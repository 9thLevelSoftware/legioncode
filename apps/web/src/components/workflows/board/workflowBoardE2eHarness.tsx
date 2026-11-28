import { EnvironmentId, ORCHESTRATION_PROTOCOL_VERSION, ServerConfig } from "@legioncode/contracts";
import { DEFAULT_RESOLVED_KEYBINDINGS } from "@legioncode/shared/keybindings";
import * as Schema from "effect/Schema";
import type { ReactNode } from "react";
import { page as browserPage } from "vite-plus/test/browser";

import { act, render, type Screen } from "../../../../test/render";

export interface BoardCapabilityExtensions {
  readonly projectWorkflowBoard?: boolean | undefined;
}

export type BoardServerConfig = Omit<ServerConfig, "environment"> & {
  readonly environment: Omit<ServerConfig["environment"], "capabilities"> & {
    readonly capabilities: ServerConfig["environment"]["capabilities"] & BoardCapabilityExtensions;
  };
};

export interface BoardServerConfigFixtureOptions {
  readonly environmentId?: EnvironmentId | undefined;
  readonly label?: string | undefined;
  readonly workflowsEnabled?: boolean | undefined;
  readonly projectWorkflowBoard?: boolean | undefined;
}

export interface BoardRpcCallRecord {
  readonly method: string;
  readonly input: unknown;
}

export interface BoardRpcStub {
  readonly calls: Array<BoardRpcCallRecord>;
  on(method: string, handler: (input: unknown) => unknown): void;
  defer(method: string): {
    readonly promise: Promise<unknown>;
    resolve(value: unknown): void;
    reject(reason?: unknown): void;
  };
  dispatch(method: string, input: unknown): Promise<unknown>;
  clear(): void;
}

export interface RenderWithBoardRouterOptions {
  readonly route?: string | undefined;
  readonly width?: number | undefined;
  readonly height?: number | undefined;
}

const decodeServerConfig = Schema.decodeUnknownSync(ServerConfig);

export function withProjectWorkflowBoardCapability(
  config: BoardServerConfig,
  enabled: boolean,
): BoardServerConfig {
  return {
    ...config,
    environment: {
      ...config.environment,
      capabilities: {
        ...config.environment.capabilities,
        projectWorkflowBoard: enabled,
      },
    },
  };
}

export function hasProjectWorkflowBoardCapability(
  config: BoardServerConfig | ServerConfig | null | undefined,
): boolean {
  const caps = config?.environment.capabilities as BoardCapabilityExtensions | undefined;
  return caps?.projectWorkflowBoard === true;
}

export function makeBoardServerConfigFixture(
  options: BoardServerConfigFixtureOptions = {},
): BoardServerConfig {
  const environmentId = options.environmentId ?? EnvironmentId.make("env-board-e2e");
  const base = decodeServerConfig({
    environment: {
      environmentId,
      label: options.label ?? "board-e2e-server",
      platform: { os: "linux", arch: "x64" },
      serverVersion: "0.1.0-e2e",
      orchestrationProtocolVersion: ORCHESTRATION_PROTOCOL_VERSION,
      capabilities: {
        repositoryIdentity: true,
        sideChats: true,
      },
    },
    auth: {
      policy: "loopback-browser",
      bootstrapMethods: ["one-time-token"],
      sessionMethods: ["browser-session-cookie"],
      sessionCookieName: "legioncode_session",
    },
    cwd: "/workspace/board-e2e",
    keybindingsConfigPath: "/workspace/.legioncode/keybindings.json",
    keybindings: [],
    issues: [],
    providers: [],
    availableEditors: [],
    observability: {
      logsDirectoryPath: "/workspace/.legioncode/logs",
      localTracingEnabled: false,
      otlpTracesEnabled: false,
      otlpMetricsEnabled: false,
      otlpLogsEnabled: false,
    },
    settings: {
      workflowsEnabled: options.workflowsEnabled ?? false,
    },
  });

  const withKeybindings: BoardServerConfig = {
    ...base,
    keybindings: DEFAULT_RESOLVED_KEYBINDINGS,
  };
  return withProjectWorkflowBoardCapability(withKeybindings, options.projectWorkflowBoard ?? false);
}

export async function withDesktopViewport(width = 1280, height = 800): Promise<AsyncDisposable> {
  const prevWidth = window.innerWidth;
  const prevHeight = window.innerHeight;
  await browserPage.viewport(width, height);
  return {
    [Symbol.asyncDispose]: () => browserPage.viewport(prevWidth, prevHeight),
  };
}

export function createBoardRpcStub(): BoardRpcStub {
  const calls: Array<BoardRpcCallRecord> = [];
  const handlers = new Map<string, (input: unknown) => unknown>();
  const deferred = new Map<string, Array<PromiseWithResolvers<unknown>>>();

  return {
    calls,
    on(method, handler) {
      handlers.set(method, handler);
    },
    defer(method) {
      const gate = Promise.withResolvers<unknown>();
      const queue = deferred.get(method) ?? [];
      queue.push(gate);
      deferred.set(method, queue);
      return {
        promise: gate.promise,
        resolve(value) {
          gate.resolve(value);
        },
        reject(reason) {
          gate.reject(reason);
        },
      };
    },
    async dispatch(method, input) {
      calls.push({ method, input });
      const queue = deferred.get(method);
      const pending = queue?.shift();
      if (queue && queue.length === 0) {
        deferred.delete(method);
      }
      if (pending) {
        return await pending.promise;
      }
      const handler = handlers.get(method);
      if (handler) {
        return await handler(input);
      }
      return { _tag: "Success", value: undefined };
    },
    clear() {
      calls.length = 0;
      deferred.clear();
    },
  };
}

function makePointerEvent(type: string, x: number, y: number): PointerEvent {
  return new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    pointerId: 1,
    isPrimary: true,
    button: 0,
    buttons: type === "pointerup" || type === "pointercancel" ? 0 : 1,
    clientX: x,
    clientY: y,
  });
}

export async function dispatchPointerDrag(
  sourceEl: Element,
  targetEl: Element,
): Promise<Disposable> {
  const srcRect = sourceEl.getBoundingClientRect();
  const dstRect = targetEl.getBoundingClientRect();
  const srcX = srcRect.left + srcRect.width / 2;
  const srcY = srcRect.top + srcRect.height / 2;
  const dstX = dstRect.left + dstRect.width / 2;
  const dstY = dstRect.top + dstRect.height / 2;
  const dx = dstX - srcX;
  const dy = dstY - srcY;
  const dist = Math.hypot(dx, dy);
  const actX = dist >= 20 ? (srcX + dstX) / 2 : dist > 0 ? srcX + (dx / dist) * 12 : srcX + 12;
  const actY = dist >= 20 ? (srcY + dstY) / 2 : dist > 0 ? srcY + (dy / dist) * 12 : srcY;

  let completed = false;
  await act(async () => {
    sourceEl.dispatchEvent(makePointerEvent("pointerdown", srcX, srcY));
  });
  try {
    await act(async () => {
      document.dispatchEvent(makePointerEvent("pointermove", actX, actY));
    });
    await act(async () => {
      document.dispatchEvent(makePointerEvent("pointermove", dstX, dstY));
    });
    await act(async () => {
      document.dispatchEvent(makePointerEvent("pointerup", dstX, dstY));
      completed = true;
    });
  } catch (err) {
    if (!completed) {
      document.dispatchEvent(makePointerEvent("pointercancel", dstX, dstY));
    }
    throw err;
  }

  return {
    [Symbol.dispose]() {
      if (!completed) {
        document.dispatchEvent(makePointerEvent("pointercancel", dstX, dstY));
      }
    },
  };
}

export async function renderWithBoardRouter(
  ui: ReactNode,
  options: RenderWithBoardRouterOptions = {},
): Promise<Screen> {
  await using stack = new AsyncDisposableStack();
  if (options.width !== undefined || options.height !== undefined) {
    stack.use(await withDesktopViewport(options.width ?? 1280, options.height ?? 800));
  }
  const screen = stack.use(await render(ui, { router: options.route ?? "/" }));
  const owned = stack.move();
  return {
    ...screen,
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}
