import type { DesktopBridge } from "@t3tools/contracts";
import { ClientCapabilities } from "@t3tools/client-runtime/platform";
import { afterEach, describe, expect, it, vi } from "@effect/vitest";
import * as Effect from "effect/Effect";
import * as Fiber from "effect/Fiber";
import * as Option from "effect/Option";
import { HttpClient } from "effect/http";

import { layer as connectionPlatformLayer } from "../../connection/platform";
import { __resetDesktopPrimaryAuthForTests } from "./desktopAuth";
import * as PrimaryEnvironmentHttpLayer from "./httpLayer";

function createSignal<A>() {
  let resolve!: (value: A) => void;
  const promise = new Promise<A>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

describe("primary environment HTTP layer", { concurrent: false }, () => {
  afterEach(() => {
    __resetDesktopPrimaryAuthForTests();
    Reflect.deleteProperty(globalThis, "window");
    vi.unstubAllGlobals();
  });

  it.effect("uses cookie credentials for browser primary environments", () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        location: {
          href: "http://127.0.0.1:3773/settings",
          origin: "http://127.0.0.1:3773",
        },
      },
    });

    return Effect.gen(function* () {
      yield* HttpClient.get("http://127.0.0.1:3773/api/auth/session");

      const request = new Request(fetchMock.mock.calls[0]?.[0], fetchMock.mock.calls[0]?.[1]);
      expect(request.credentials).toBe("include");
      expect(request.headers.get("authorization")).toBeNull();
    }).pipe(Effect.provide(PrimaryEnvironmentHttpLayer.layerForCurrentOrigin()));
  });

  it.effect("uses bearer auth without cookies for desktop-managed primaries", () => {
    const requested = createSignal<void>();
    const bearerToken = createSignal<string>();
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        location: { origin: "t3code://app" },
        desktopBridge: {
          getLocalEnvironmentBootstrap: () => ({
            label: "Local environment",
            httpBaseUrl: "http://127.0.0.1:3773",
            wsBaseUrl: "ws://127.0.0.1:3773",
            bootstrapToken: "desktop-bootstrap-token",
          }),
          getLocalEnvironmentBearerToken: () => {
            requested.resolve(undefined);
            return bearerToken.promise;
          },
        } as unknown as DesktopBridge,
      },
    });

    return Effect.gen(function* () {
      const requestFiber = yield* HttpClient.get(
        "http://127.0.0.1:3773/api/connect/link-state",
      ).pipe(Effect.forkChild);
      yield* Effect.promise(() => requested.promise);
      expect(fetchMock).not.toHaveBeenCalled();
      bearerToken.resolve("desktop-bearer-token");
      yield* Fiber.join(requestFiber);

      const request = new Request(fetchMock.mock.calls[0]?.[0], fetchMock.mock.calls[0]?.[1]);
      expect(request.credentials).not.toBe("include");
      expect(request.headers.get("authorization")).toBe("Bearer desktop-bearer-token");
    }).pipe(Effect.provide(PrimaryEnvironmentHttpLayer.layerForCurrentOrigin()));
  });

  it.effect("invalidates and retries once with forceRefresh on HTTP 401", () => {
    const getLocalEnvironmentBearerToken = vi
      .fn<(options?: { readonly forceRefresh?: boolean }) => Promise<string>>()
      .mockResolvedValueOnce("stale-bearer-token")
      .mockResolvedValueOnce("fresh-bearer-token")
      .mockResolvedValueOnce("third-bearer-token");
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(null, { status: 401 }))
      .mockResolvedValueOnce(new Response("ok", { status: 200 }))
      .mockResolvedValueOnce(new Response(null, { status: 401 }))
      .mockResolvedValueOnce(new Response(null, { status: 401 }));
    vi.stubGlobal("fetch", fetchMock);
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        location: { href: "t3code://app/", origin: "t3code://app" },
        desktopBridge: {
          getClientPlatform: () => "linux",
          getLocalEnvironmentEnabled: () => true,
          getLocalEnvironmentBearerToken,
        } as unknown as DesktopBridge,
      },
    });

    return Effect.gen(function* () {
      const recovered = yield* HttpClient.get("http://127.0.0.1:3773/api/connect/link-state");
      expect(recovered.status).toBe(200);
      expect(getLocalEnvironmentBearerToken).toHaveBeenCalledTimes(2);
      expect(getLocalEnvironmentBearerToken.mock.calls[0]).toEqual([]);
      expect(getLocalEnvironmentBearerToken.mock.calls[1]).toEqual([{ forceRefresh: true }]);
      expect(
        fetchMock.mock.calls.map(([, init]) => new Headers(init?.headers).get("authorization")),
      ).toEqual(["Bearer stale-bearer-token", "Bearer fresh-bearer-token"]);

      // A persistent 401 retries only once per request.
      const rejected = yield* HttpClient.get("http://127.0.0.1:3773/api/connect/link-state");
      expect(rejected.status).toBe(401);
      expect(getLocalEnvironmentBearerToken).toHaveBeenCalledTimes(3);
      expect(getLocalEnvironmentBearerToken.mock.calls[2]).toEqual([{ forceRefresh: true }]);
      expect(fetchMock).toHaveBeenCalledTimes(4);
    }).pipe(Effect.provide(PrimaryEnvironmentHttpLayer.layerForCurrentOrigin()));
  });

  it.effect("clears cached bearer token on primary WS auth refresh", () => {
    const getLocalEnvironmentBearerToken = vi
      .fn<(options?: { readonly forceRefresh?: boolean }) => Promise<string>>()
      .mockResolvedValueOnce("http-cached-token")
      .mockResolvedValueOnce("ws-initial-token")
      .mockResolvedValueOnce("ws-refreshed-token");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 200 })));
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        location: { href: "t3code://app/", origin: "t3code://app" },
        desktopBridge: {
          getClientPlatform: () => "linux",
          getLocalEnvironmentEnabled: () => true,
          getLocalEnvironmentBearerToken,
        } as unknown as DesktopBridge,
      },
    });

    return Effect.gen(function* () {
      yield* HttpClient.get("http://127.0.0.1:3773/api/connect/link-state").pipe(
        Effect.provide(PrimaryEnvironmentHttpLayer.layerForCurrentOrigin()),
      );

      const primaryAuth = yield* ClientCapabilities.PrimaryEnvironmentAuth.pipe(
        Effect.provide(connectionPlatformLayer),
      );
      expect(yield* primaryAuth.bearerToken).toEqual(Option.some("ws-initial-token"));
      expect(yield* primaryAuth.bearerToken).toEqual(Option.some("ws-refreshed-token"));
      expect(getLocalEnvironmentBearerToken).toHaveBeenCalledTimes(3);
      expect(getLocalEnvironmentBearerToken.mock.calls[2]).toEqual([{ forceRefresh: true }]);
    });
  });
});
