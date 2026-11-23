import { layerRemoteHttpClient } from "@legioncode/client-runtime/rpc";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import { FetchHttpClient, HttpClient, HttpClientError, HttpClientRequest } from "effect/http";

import {
  readDesktopPrimaryBearerToken,
  type ReadDesktopPrimaryBearerTokenOptions,
} from "./desktopAuth";
import { resolvePrimaryEnvironmentHttpUrl } from "./target";

function isSameOriginBrowserPrimary(): boolean {
  if (
    typeof window === "undefined" ||
    window.desktopBridge !== undefined ||
    !window.location.origin.startsWith("http")
  ) {
    return false;
  }

  return new URL(resolvePrimaryEnvironmentHttpUrl("/")).origin === window.location.origin;
}

function readBearerTokenForRequest(
  request: HttpClientRequest.HttpClientRequest,
  options?: ReadDesktopPrimaryBearerTokenOptions,
): Effect.Effect<string | null, HttpClientError.HttpClientError> {
  return Effect.tryPromise({
    try: () => readDesktopPrimaryBearerToken(options),
    catch: (cause) =>
      new HttpClientError.HttpClientError({
        reason: new HttpClientError.TransportError({ request, cause }),
      }),
  });
}

function withPrimaryBearerToken(client: HttpClient.HttpClient): HttpClient.HttpClient {
  return HttpClient.makeWith(
    (requestEffect) =>
      Effect.gen(function* () {
        const baseRequest = yield* requestEffect;
        const initialToken = yield* readBearerTokenForRequest(baseRequest);
        const firstRequest = initialToken
          ? HttpClientRequest.bearerToken(baseRequest, initialToken)
          : baseRequest;
        const response = yield* client.postprocess(Effect.succeed(firstRequest));
        if (response.status !== 401 || !initialToken) {
          return response;
        }
        const refreshedToken = yield* readBearerTokenForRequest(baseRequest, {
          forceRefresh: true,
          invalidatedToken: initialToken,
        });
        if (!refreshedToken) {
          return response;
        }
        return yield* client.postprocess(
          Effect.succeed(HttpClientRequest.bearerToken(baseRequest, refreshedToken)),
        );
      }),
    client.preprocess,
  );
}

export function layerForCurrentOrigin() {
  return Layer.unwrap(
    Effect.sync(() => {
      const layerBase = layerRemoteHttpClient(globalThis.fetch);
      if (isSameOriginBrowserPrimary()) {
        return Layer.merge(
          layerBase,
          Layer.succeed(FetchHttpClient.RequestInit, { credentials: "include" }),
        );
      }

      const layerBearerClient = Layer.effect(
        HttpClient.HttpClient,
        Effect.map(HttpClient.HttpClient, withPrimaryBearerToken),
      ).pipe(Layer.provide(layerBase));

      return Layer.merge(
        layerBearerClient,
        Layer.succeed(FetchHttpClient.RequestInit, { credentials: "omit" }),
      );
    }),
  );
}

export const layer = layerForCurrentOrigin();
