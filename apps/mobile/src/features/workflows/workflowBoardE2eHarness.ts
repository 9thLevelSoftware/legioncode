import {
  PrimaryConnectionTarget,
  type EnvironmentConnectionPhase,
  type EnvironmentServerConfig,
} from "@legioncode/client-runtime/connection";
import { EnvironmentId, ORCHESTRATION_PROTOCOL_VERSION, ServerConfig } from "@legioncode/contracts";
import { DEFAULT_RESOLVED_KEYBINDINGS } from "@legioncode/shared/keybindings";
import * as Option from "effect/Option";
import * as Schema from "effect/Schema";
import { act, createElement, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

export interface MobileBoardCapabilitiesExtension {
  readonly projectWorkflowBoard?: boolean | undefined;
}

export type MobileBoardServerConfig = Omit<EnvironmentServerConfig, "environment"> & {
  readonly environment: Omit<EnvironmentServerConfig["environment"], "capabilities"> & {
    readonly capabilities: EnvironmentServerConfig["environment"]["capabilities"] &
      MobileBoardCapabilitiesExtension;
  };
};

export interface MobileBoardEnvironmentFixture {
  readonly environmentId: EnvironmentId;
  readonly label: string;
  readonly displayUrl: string | null;
  readonly relayManaged: boolean;
  readonly entry: {
    readonly target: PrimaryConnectionTarget;
    readonly profile: Option.Option<never>;
    readonly enabled: boolean;
  };
  readonly connection: {
    readonly phase: EnvironmentConnectionPhase;
    readonly error: string | null;
    readonly traceId: string | null;
  };
  readonly serverConfig: MobileBoardServerConfig | null;
  readonly refusedRouteIds: ReadonlyArray<string>;
}

export interface MakeMobileBoardEnvironmentOptions {
  readonly environmentId?: EnvironmentId | undefined;
  readonly label?: string | undefined;
  readonly phase?: EnvironmentConnectionPhase | undefined;
  readonly workflowsEnabled?: boolean | undefined;
  readonly projectWorkflowBoard?: boolean | undefined;
  readonly omitServerConfig?: boolean | undefined;
}

export interface MobileNavigationCall {
  readonly name: string;
  readonly params: unknown;
}

export interface MobileNavigationStub {
  readonly initialParams: Record<string, unknown>;
  readonly dispatched: Array<unknown>;
  readonly navigated: Array<MobileNavigationCall>;
  readonly getGoBackCount: () => number;
  readonly navigation: {
    navigate(name: string, params?: unknown): void;
    dispatch(action: unknown): void;
    canGoBack(): boolean;
    goBack(): void;
    isFocused(): boolean;
    addListener(event: string, listener: () => void): () => void;
  };
  emit(event: string): void;
}

export interface MobileCommandCall {
  readonly label: string;
  readonly input: unknown;
}

export interface MobileCommandRecorder {
  readonly calls: Array<MobileCommandCall>;
  deferNext(label: string): {
    readonly promise: Promise<unknown>;
    resolve(result?: unknown): void;
    reject(reason?: unknown): void;
  };
  wrap(label: string): (input: unknown) => Promise<unknown>;
  reset(): void;
}

export interface MountedMobileTree extends Disposable {
  readonly container: HTMLDivElement;
  rerender(next: ReactNode): void;
  [Symbol.dispose](): void;
}

const decodeServerConfig = Schema.decodeUnknownSync(ServerConfig);

export function MobileBoardTestTextInput({
  value,
  onChangeText,
  accessibilityLabel,
  editable,
}: {
  readonly value: string;
  readonly onChangeText: (next: string) => void;
  readonly accessibilityLabel?: string;
  readonly editable?: boolean;
}) {
  return createElement("input", {
    "aria-label": accessibilityLabel,
    value,
    disabled: editable === false,
    onChange: (event) => onChangeText(event.currentTarget.value),
  });
}

export function makeMobileBoardEnvironment(
  options: MakeMobileBoardEnvironmentOptions = {},
): MobileBoardEnvironmentFixture {
  const environmentId = options.environmentId ?? EnvironmentId.make("mobile-board-env");
  const label = options.label ?? "mobile-board-e2e";
  const phase = options.phase ?? "connected";

  const serverConfig: MobileBoardServerConfig | null = options.omitServerConfig
    ? null
    : (() => {
        const decoded = decodeServerConfig({
          environment: {
            environmentId,
            label,
            platform: { os: "darwin", arch: "arm64" },
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
          cwd: "/workspace/mobile-board-e2e",
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
            workflowsEnabled: options.workflowsEnabled ?? true,
          },
        });
        const { providers: _providers, ...rest } = decoded;
        return {
          ...rest,
          keybindings: DEFAULT_RESOLVED_KEYBINDINGS,
          environment: {
            ...rest.environment,
            capabilities: {
              ...rest.environment.capabilities,
              projectWorkflowBoard: options.projectWorkflowBoard ?? false,
            },
          },
        };
      })();

  return {
    environmentId,
    label,
    displayUrl: null,
    relayManaged: false,
    entry: {
      target: new PrimaryConnectionTarget({
        environmentId,
        label,
        httpBaseUrl: "http://127.0.0.1:3773",
        wsBaseUrl: "ws://127.0.0.1:3773",
      }),
      profile: Option.none(),
      enabled: true,
    },
    connection: {
      phase,
      error: null,
      traceId: null,
    },
    serverConfig,
    refusedRouteIds: [],
  };
}

export function hasMobileProjectWorkflowBoardCapability(
  environments: ReadonlyArray<Pick<MobileBoardEnvironmentFixture, "connection" | "serverConfig">>,
): boolean {
  return environments.some(
    (env) =>
      env.connection.phase === "connected" &&
      env.serverConfig?.environment.capabilities.projectWorkflowBoard === true,
  );
}

export function createMobileNavigationStub(
  initialParams: Record<string, unknown> = {},
): MobileNavigationStub {
  const dispatched: Array<unknown> = [];
  const navigated: Array<MobileNavigationCall> = [];
  const listeners = new Map<string, Set<() => void>>();
  let goBackCount = 0;

  return {
    initialParams,
    dispatched,
    navigated,
    getGoBackCount: () => goBackCount,
    navigation: {
      navigate(name, params) {
        navigated.push({ name, params });
      },
      dispatch(action) {
        dispatched.push(action);
      },
      canGoBack: () => true,
      goBack() {
        goBackCount += 1;
      },
      isFocused: () => true,
      addListener(event, listener) {
        let bucket = listeners.get(event);
        if (!bucket) {
          bucket = new Set();
          listeners.set(event, bucket);
        }
        bucket.add(listener);
        return () => {
          bucket.delete(listener);
        };
      },
    },
    emit(event) {
      const bucket = listeners.get(event);
      if (!bucket) return;
      for (const listener of bucket) {
        listener();
      }
    },
  };
}

export function createMobileCommandRecorder(): MobileCommandRecorder {
  const calls: Array<MobileCommandCall> = [];
  const gates = new Map<string, PromiseWithResolvers<unknown>>();

  return {
    calls,
    deferNext(label) {
      const resolver = Promise.withResolvers<unknown>();
      gates.set(label, resolver);
      return {
        promise: resolver.promise,
        resolve(result = { _tag: "Success", value: undefined }) {
          resolver.resolve(result);
        },
        reject(reason) {
          resolver.reject(reason);
        },
      };
    },
    wrap(label) {
      return async (input: unknown) => {
        calls.push({ label, input });
        const pending = gates.get(label);
        if (pending) {
          gates.delete(label);
          return await pending.promise;
        }
        return { _tag: "Success", value: undefined };
      };
    },
    reset() {
      calls.length = 0;
      gates.clear();
    },
  };
}

export function mountMobileTestTree(element: ReactNode): MountedMobileTree {
  const actEnv = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
  const prevAct = actEnv.IS_REACT_ACT_ENVIRONMENT;
  actEnv.IS_REACT_ACT_ENVIRONMENT = true;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    act(() => {
      root.render(element);
    });
  } catch (err) {
    try {
      act(() => {
        root.unmount();
      });
    } finally {
      actEnv.IS_REACT_ACT_ENVIRONMENT = prevAct;
    }
    throw err;
  }

  return {
    container,
    rerender(next) {
      act(() => {
        root.render(next);
      });
    },
    [Symbol.dispose]() {
      try {
        act(() => {
          root.unmount();
        });
      } finally {
        actEnv.IS_REACT_ACT_ENVIRONMENT = prevAct;
      }
    },
  };
}
