import { nativeLanguageName, resolveLocaleSelection } from "@legioncode/client-runtime/locales";
import {
  EnvironmentId,
  IsoDateTime,
  PositiveInt,
  ProjectId,
  ThreadId,
  WorkflowBoardArtifactKey,
  WorkflowCardId,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { AsyncResult } from "effect/reactivity";
import { act, type ReactNode } from "react";
import { describe, expect, it, vi } from "vite-plus/test";

import { DEFAULT_LOCALE, SUPPORTED_LOCALES } from "../../i18n/locales";
import {
  createMobileCommandRecorder,
  createMobileNavigationStub,
  hasMobileProjectWorkflowBoardCapability,
  makeMobileBoardEnvironment,
  MobileBoardTestTextInput,
  mountMobileTestTree,
  type MobileBoardEnvironmentFixture,
  type MobileCommandRecorder,
} from "./workflowBoardE2eHarness";

const atomSupport = vi.hoisted(() => {
  const ATOM_DESCRIPTOR = Symbol.for("legioncode.e2e.mobileAtomDescriptor");
  type AtomDescriptor =
    | { readonly kind: "liveAcross"; readonly prop: string; readonly arg: unknown }
    | { readonly kind: "permission"; readonly method: string; readonly environmentId: unknown }
    | { readonly kind: "readPermission"; readonly environmentId: unknown }
    | { readonly kind: "configValue"; readonly environmentId: unknown }
    | { readonly kind: "settingsValue"; readonly environmentId: unknown }
    | { readonly kind: "serverConfigsMap" }
    | {
        readonly kind: "environmentSubscription";
        readonly prop: string;
        readonly method: string;
        readonly arg: unknown;
      }
    | {
        readonly kind: "environmentQuery";
        readonly prop: string;
        readonly method: string;
        readonly arg: unknown;
      };

  const tagAtom = <T extends AtomDescriptor>(descriptor: T) => ({
    [ATOM_DESCRIPTOR]: descriptor,
  });

  const readDescriptor = (atom: unknown): AtomDescriptor | null => {
    if (typeof atom === "object" && atom !== null && ATOM_DESCRIPTOR in atom) {
      return (atom as Record<symbol, AtomDescriptor>)[ATOM_DESCRIPTOR] ?? null;
    }
    return null;
  };

  return { tagAtom, readDescriptor };
});

const navHolder = vi.hoisted(() => ({
  navigation: null as ReturnType<typeof createMobileNavigationStub>["navigation"] | null,
  initialParams: {} as Record<string, unknown>,
}));

const envHolder = vi.hoisted(() => ({
  environments: [] as ReadonlyArray<MobileBoardEnvironmentFixture>,
  liveEntries: [] as ReadonlyArray<{
    readonly environmentId: EnvironmentId;
    readonly result: AsyncResult.AsyncResult<unknown, unknown>;
  }>,
  queryData: null as unknown,
  canOperate: true,
  recorder: null as MobileCommandRecorder | null,
}));

vi.mock("react-native", () => ({
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Pressable: ({
    children,
    onPress,
    accessibilityLabel,
    disabled,
    accessibilityState,
  }: {
    children?: ReactNode;
    onPress?: () => void;
    accessibilityLabel?: string;
    disabled?: boolean;
    accessibilityState?: { readonly disabled?: boolean };
  }) => {
    const isDisabled = Boolean(disabled || accessibilityState?.disabled);
    return (
      <button
        type="button"
        disabled={isDisabled}
        aria-disabled={isDisabled ? "true" : undefined}
        aria-label={accessibilityLabel}
        onClick={isDisabled ? undefined : onPress}
      >
        {children}
      </button>
    );
  },
}));

vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ top: 0, right: 0, bottom: 0, left: 0 }),
}));

vi.mock("@react-navigation/native", () => ({
  StackActions: { push: (name: string, params?: unknown) => ({ type: "PUSH", name, params }) },
  useNavigation: () => navHolder.navigation ?? createMobileNavigationStub().navigation,
  useRoute: () => ({ params: navHolder.initialParams }),
}));

vi.mock("../../state/server", () => {
  const toMethodName = (prop: string): string => {
    if (!prop.startsWith("workflowBoard")) return prop;
    const rest = prop.slice("workflowBoard".length);
    return rest.length === 0
      ? "workflowBoard"
      : `workflowBoard.${rest.replace(/^[A-Z]/, (c) => c.toLowerCase())}`;
  };

  const cache = new Map<string, unknown>();
  const serverEnvironment = new Proxy(
    {},
    {
      get(_target, prop) {
        if (typeof prop === "symbol" || prop === "then") return undefined;
        const cached = cache.get(prop);
        if (cached !== undefined) return cached;

        const method = toMethodName(prop);
        const permissionAtom = (environmentId: EnvironmentId | null) =>
          atomSupport.tagAtom({ kind: "permission", method, environmentId });

        const fn = (arg?: unknown) => {
          if (prop === "workflowsLiveAcross" || prop.endsWith("LiveAcross")) {
            return atomSupport.tagAtom({ kind: "liveAcross", prop, arg });
          }
          if (prop === "configValueAtom") {
            return atomSupport.tagAtom({ kind: "configValue", environmentId: arg });
          }
          if (prop === "settingsValueAtom") {
            return atomSupport.tagAtom({ kind: "settingsValue", environmentId: arg });
          }
          const isSubscription =
            prop === "workflowsLive" || prop.endsWith("Live") || prop.endsWith("Subscribe");
          return atomSupport.tagAtom(
            isSubscription
              ? { kind: "environmentSubscription", prop, method, arg }
              : { kind: "environmentQuery", prop, method, arg },
          );
        };

        const stub = Object.assign(fn, {
          method,
          label: method,
          requiredScopes: () => [] as const,
          permissionAtom,
        });
        cache.set(prop, stub);
        return stub;
      },
    },
  );

  return {
    serverEnvironment,
    environmentServerConfigsAtom: atomSupport.tagAtom({ kind: "serverConfigsMap" }),
    describeOutdatedServer: { label: "describeOutdatedServer", run: vi.fn() },
    updateOutdatedServer: { label: "updateOutdatedServer", run: vi.fn() },
  };
});

vi.mock("../../state/use-atom-command", () => ({
  useAtomCommand:
    (
      command: { readonly method?: string; readonly label?: string } | undefined,
      options?: string | { readonly label?: string },
    ) =>
    (arg: { readonly input?: unknown } | unknown) => {
      const recorder = envHolder.recorder;
      const optionLabel = typeof options === "string" ? options : options?.label;
      const label = command?.method ?? optionLabel ?? command?.label ?? "unknown";
      const payload =
        arg !== null && typeof arg === "object" && "input" in arg
          ? (arg as { readonly input: unknown }).input
          : arg;
      return recorder
        ? recorder.wrap(label)(payload)
        : Promise.resolve({ _tag: "Success", value: undefined });
    },
}));

vi.mock("../../state/query", () => ({
  useEnvironmentQuery: (atom: unknown) => {
    if (atom === null || atom === undefined) {
      return {
        data: null,
        successResult: null,
        error: null,
        failure: null,
        isPending: false,
        isSuccess: false,
        refresh: vi.fn(),
      };
    }
    const descriptor = atomSupport.readDescriptor(atom);
    if (
      descriptor?.kind === "environmentSubscription" &&
      typeof descriptor.arg === "object" &&
      descriptor.arg !== null &&
      "environmentId" in descriptor.arg
    ) {
      const envId = (descriptor.arg as { readonly environmentId: EnvironmentId }).environmentId;
      const entry = envHolder.liveEntries.find((item) => item.environmentId === envId);
      if (entry) {
        if (entry.result._tag === "Success") {
          return {
            data: entry.result.value,
            successResult: !entry.result.waiting ? entry.result : null,
            error: null,
            failure: null,
            isPending: entry.result.waiting,
            isSuccess: true,
            refresh: vi.fn(),
          };
        }
        if (entry.result._tag === "Failure") {
          return {
            data: null,
            successResult: null,
            error: String(entry.result.cause),
            failure: entry.result.cause,
            isPending: entry.result.waiting,
            isSuccess: false,
            refresh: vi.fn(),
          };
        }
      }
    }
    return {
      data: envHolder.queryData,
      successResult: envHolder.queryData !== null ? AsyncResult.success(envHolder.queryData) : null,
      error: null,
      failure: null,
      isPending: false,
      isSuccess: envHolder.queryData !== null,
      refresh: vi.fn(),
    };
  },
  formatEnvironmentQueryError: (cause: unknown) => String(cause),
}));

vi.mock("@effect/atom-react", () => {
  const resolveAtomValue = (atom: unknown): unknown => {
    const descriptor = atomSupport.readDescriptor(atom);
    if (descriptor === null) return undefined;
    switch (descriptor.kind) {
      case "liveAcross":
        return envHolder.liveEntries;
      case "permission":
        return descriptor.environmentId === null || descriptor.environmentId === undefined
          ? false
          : envHolder.canOperate;
      case "readPermission":
        return descriptor.environmentId !== null && descriptor.environmentId !== undefined;
      case "configValue":
        return (
          envHolder.environments.find((env) => env.environmentId === descriptor.environmentId)
            ?.serverConfig ?? null
        );
      case "settingsValue":
        return (
          envHolder.environments.find((env) => env.environmentId === descriptor.environmentId)
            ?.serverConfig?.settings ?? null
        );
      case "serverConfigsMap":
        return new Map(
          envHolder.environments
            .filter((env) => env.serverConfig !== null)
            .map((env) => [env.environmentId, env.serverConfig]),
        );
      case "environmentSubscription": {
        if (
          typeof descriptor.arg === "object" &&
          descriptor.arg !== null &&
          "environmentId" in descriptor.arg
        ) {
          const envId = (descriptor.arg as { readonly environmentId: EnvironmentId }).environmentId;
          const entry = envHolder.liveEntries.find((item) => item.environmentId === envId);
          if (entry) return entry.result;
        }
        return AsyncResult.success(envHolder.queryData);
      }
      case "environmentQuery":
        return AsyncResult.success(envHolder.queryData);
    }
  };
  return {
    useAtomValue: (atom: unknown, selector?: (value: unknown) => unknown) => {
      const value = resolveAtomValue(atom);
      return typeof selector === "function" ? selector(value) : value;
    },
    useAtomRefresh: () => vi.fn(),
    useAtomMount: () => undefined,
    useAtomSubscribe: () => undefined,
  };
});

vi.mock("../../components/RelativeTime", () => ({
  RelativeTime: ({ timestamp }: { readonly timestamp: string }) => <span>{timestamp}</span>,
}));
vi.mock("../../components/ui/AppBar", () => ({
  AppBar: ({ title, children }: { readonly title?: string; readonly children?: ReactNode }) => (
    <header>
      {title ? <h1>{title}</h1> : null}
      {children}
    </header>
  ),
}));
vi.mock("../../components/ui/AppText", () => ({
  AppText: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  AppTextInput: MobileBoardTestTextInput,
}));
vi.mock("../../components/ui/Button", () => ({
  Button: ({
    children,
    label,
    title,
    onPress,
    disabled,
    loading,
    accessibilityLabel,
  }: {
    readonly children?: ReactNode;
    readonly label?: string;
    readonly title?: string;
    readonly onPress?: () => void;
    readonly disabled?: boolean;
    readonly loading?: boolean;
    readonly accessibilityLabel?: string;
  }) => {
    const isDisabled = Boolean(disabled || loading);
    return (
      <button
        type="button"
        disabled={isDisabled}
        aria-disabled={isDisabled ? "true" : undefined}
        aria-label={accessibilityLabel ?? label}
        onClick={isDisabled ? undefined : onPress}
      >
        {children ?? label ?? title}
      </button>
    );
  },
}));
vi.mock("../../components/ui/EmptyState", () => ({
  EmptyState: ({ title, detail }: { readonly title: string; readonly detail: string }) => (
    <div data-testid="empty-state">
      <h2>{title}</h2>
      <p>{detail}</p>
    </div>
  ),
}));
vi.mock("../../components/ui/ScreenScrollView", () => ({
  ScreenScrollView: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));
vi.mock("../../components/ui/SegmentedControl", () => ({
  SegmentedControl: ({
    options,
    onSelect,
  }: {
    readonly options: ReadonlyArray<{ readonly value: string; readonly label: string }>;
    readonly onSelect: (value: string) => void;
  }) => (
    <div>
      {options.map((opt) => (
        <button key={opt.value} type="button" onClick={() => onSelect(opt.value)}>
          {opt.label}
        </button>
      ))}
    </div>
  ),
}));
vi.mock("../../state/environments", () => ({
  useEnvironments: () => ({
    isReady: true,
    networkStatus: "online",
    environments: envHolder.environments,
    presentationById: new Map(),
  }),
}));

// The card screen reads board RPCs through the client-runtime atoms in state/workflowBoard.
// Commands carry their WS method so the recorder logs the wire method name.
vi.mock("../../state/workflowBoard", () => {
  const command = (method: string) => ({
    method,
    label: method,
    permissionAtom: (environmentId: EnvironmentId | null) =>
      atomSupport.tagAtom({ kind: "permission", method, environmentId }),
  });
  return {
    workflowBoard: {
      readPermissionAtom: (environmentId: EnvironmentId | null) =>
        atomSupport.tagAtom({ kind: "readPermission", environmentId }),
      cardDetail: (arg: unknown) =>
        atomSupport.tagAtom({
          kind: "environmentQuery",
          prop: "cardDetail",
          method: "workflowBoard.getCard",
          arg,
        }),
      // Artifact and diff reads are not under test; an absent atom yields an idle query.
      artifact: () => null,
      cardDiff: () => null,
      approveSpec: command("workflowBoard.approveSpec"),
      approvePlan: command("workflowBoard.approvePlan"),
      approveDelivery: command("workflowBoard.approveDelivery"),
      pauseCard: command("workflowBoard.pauseCard"),
      resumeCard: command("workflowBoard.resumeCard"),
      retryStep: command("workflowBoard.retryStep"),
      returnForRevision: command("workflowBoard.returnForRevision"),
      recordManualAcceptance: command("workflowBoard.recordManualAcceptance"),
      resolveFinding: command("workflowBoard.resolveFinding"),
    },
  };
});
vi.mock("../../state/entities", () => ({
  useProjects: () =>
    envHolder.environments.map((env) => ({
      environmentId: env.environmentId,
      id: "mobile-board-project",
      title: "Mobile Board Project",
      workspaceRoot: "/workspace/mobile-board-e2e",
      archivedAt: null,
    })),
}));
vi.mock("../../state/threads", () => ({
  startOpeningEnvironmentThread: () => undefined,
}));
vi.mock("../settings/settings-navigation", () => ({
  openSettings: () => undefined,
}));
vi.mock("../../components/ui/Dialog", () => ({
  confirm: (request: { readonly onConfirm: () => void }) => {
    request.onConfirm();
  },
}));
vi.mock("../../components/ui/Notice", () => ({
  Notice: ({ message }: { readonly message: string }) => <div role="status">{message}</div>,
}));
vi.mock("../../components/ui/ScreenContent", () => ({
  ScreenContent: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));
vi.mock("../../components/ui/ListSection", () => ({
  ListSection: ({ children }: { children?: ReactNode }) => <section>{children}</section>,
}));
vi.mock("../../components/ui/Chip", () => ({
  Chip: ({ label, onPress }: { readonly label: string; readonly onPress?: () => void }) => (
    <button type="button" onClick={onPress}>
      {label}
    </button>
  ),
}));
vi.mock("../../components/StatusPill", () => ({
  StatusPill: ({ label }: { readonly label: string }) => <span>{label}</span>,
}));

import { WorkflowCardRouteScreen } from "../workflow-board/WorkflowCardRouteScreen";
import { WorkflowsScreen } from "./WorkflowsScreen";

const MOBILE_BOARD_PROJECT_ID = "mobile-board-project";

function makeMobileCardFixture(): WorkflowCardDetail {
  const cardId = WorkflowCardId.make("card-mobile-1");
  return {
    cardId,
    projectId: ProjectId.make(MOBILE_BOARD_PROJECT_ID),
    title: "Mobile Qualification Card",
    stage: "ship",
    status: "active",
    revision: PositiveInt.make(3),
    currentTask: null,
    blockerSummary: null,
    nextAction: "approve_delivery",
    specApprovedRevision: PositiveInt.make(1),
    specApprovalStatus: "valid",
    activeThreadId: ThreadId.make("thr-mobile-exec-1"),
    createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
    updatedAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
    workspaceStrategy: { type: "root" },
    modelSelection: null,
    runtimeMode: null,
    interactionMode: null,
    spec: {
      title: "Mobile Qualification Card",
      brief: "Verify mobile approval.",
      sourceType: "idea",
      assistanceMode: "balanced",
      acceptanceCriteria: [{ id: "AC-1", text: "Verify mobile approval" }],
      constraints: [],
      sourceRefs: [],
      wireframesIndex: null,
      specDigest: "sha256:mobile-spec-1",
      updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
    },
    specApproval: null,
    stageRuns: [],
    revisions: [],
    artifactKeys: [WorkflowBoardArtifactKey.make("spec.md")],
    deliveryStatus: "ready",
    shipReadiness: {
      status: "ready",
      ready: true,
      productSnapshotDigest: "sha256:mobile-snapshot-1",
      blockers: [],
      deliveryReceipt: null,
    },
  };
}

function findButtonMatching(
  container: HTMLElement,
  pattern: RegExp,
): HTMLButtonElement | undefined {
  return [...container.querySelectorAll("button")].find((btn) =>
    pattern.test(`${btn.getAttribute("aria-label") ?? ""} ${btn.textContent ?? ""}`),
  );
}

describe("F23 mobile capability gating, error isolation & 10-locale resolution", () => {
  it("[F23-T1-04] renders legacy WorkflowsScreen cleanly and verifies mobile board evidence inspection, Thread navigation & approve/pause commands", async () => {
    const legacyEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("mobile-legacy-env"),
      projectWorkflowBoard: false,
    });
    expect(hasMobileProjectWorkflowBoardCapability([legacyEnv])).toBe(false);

    const navStub = createMobileNavigationStub();
    navHolder.navigation = navStub.navigation;
    navHolder.initialParams = {};
    envHolder.environments = [legacyEnv];
    envHolder.liveEntries = [
      {
        environmentId: legacyEnv.environmentId,
        result: AsyncResult.success({ workflows: [], runs: [], questions: [] }),
      },
    ];
    envHolder.queryData = null;
    envHolder.canOperate = true;
    envHolder.recorder = null;

    {
      using mounted = mountMobileTestTree(<WorkflowsScreen />);
      expect(mounted.container.textContent).toContain("No workflow runs yet");
    }

    const capableEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("mobile-board-env"),
      projectWorkflowBoard: true,
    });
    expect(hasMobileProjectWorkflowBoardCapability([capableEnv])).toBe(true);
    const cardFixture = makeMobileCardFixture();
    const boardNav = createMobileNavigationStub({
      environmentId: capableEnv.environmentId,
      projectId: cardFixture.projectId,
      cardId: cardFixture.cardId,
    });
    const recorder = createMobileCommandRecorder();
    navHolder.navigation = boardNav.navigation;
    navHolder.initialParams = boardNav.initialParams;
    envHolder.environments = [capableEnv];
    envHolder.liveEntries = [];
    envHolder.queryData = cardFixture;
    envHolder.canOperate = true;
    envHolder.recorder = recorder;

    using boardTree = mountMobileTestTree(
      <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />,
    );
    const text = boardTree.container.textContent ?? "";
    expect(text).toContain("Mobile Qualification Card");
    expect(text).toContain("AC-1");
    expect(text).toContain("Ready to ship");

    const threadBtn = findButtonMatching(boardTree.container, /thread/i);
    expect(threadBtn).toBeDefined();
    act(() => {
      threadBtn?.click();
    });
    expect(boardNav.dispatched).toContainEqual({
      type: "PUSH",
      name: "Thread",
      params: { environmentId: capableEnv.environmentId, threadId: "thr-mobile-exec-1" },
    });

    const approveBtn = findButtonMatching(boardTree.container, /approve delivery/i);
    expect(approveBtn).toBeDefined();
    await act(async () => {
      approveBtn?.click();
    });

    const pauseOrResumeBtn = findButtonMatching(boardTree.container, /pause|resume/i);
    expect(pauseOrResumeBtn).toBeDefined();
    await act(async () => {
      pauseOrResumeBtn?.click();
    });

    // The useAtomCommand mock records the command's `input` payload.
    expect(recorder.calls).toEqual([
      {
        label: "workflowBoard.approveDelivery",
        input: {
          cardId: cardFixture.cardId,
          expectedRevision: 3,
          expectedProductSnapshotDigest: "sha256:mobile-snapshot-1",
        },
      },
      {
        label: "workflowBoard.pauseCard",
        input: { cardId: cardFixture.cardId, expectedRevision: 3 },
      },
    ]);
  });

  it("[F23-T2-03] enforces mixed-capability/offline isolation, disables controls on disconnected/read-only board, and omits desktop-only authoring", () => {
    const offlineCapable = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("offline-cap"),
      phase: "offline",
      projectWorkflowBoard: true,
    });
    const connectedLegacy = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("connected-legacy"),
      phase: "connected",
      projectWorkflowBoard: false,
    });
    const connectedNoConfig = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("connected-noconfig"),
      phase: "connected",
      omitServerConfig: true,
    });
    expect(
      hasMobileProjectWorkflowBoardCapability([offlineCapable, connectedLegacy, connectedNoConfig]),
    ).toBe(false);

    envHolder.environments = [];
    envHolder.liveEntries = [];
    {
      using noEnvTree = mountMobileTestTree(<WorkflowsScreen />);
      expect(noEnvTree.container.textContent).toContain("No environment connected");
    }

    const failingEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("failing-env"),
      label: "failing-mac",
      projectWorkflowBoard: true,
    });
    const healthyEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("healthy-env"),
      label: "healthy-mac",
      projectWorkflowBoard: true,
    });
    envHolder.environments = [failingEnv, healthyEnv];
    envHolder.liveEntries = [
      {
        environmentId: failingEnv.environmentId,
        result: AsyncResult.failure("connection refused" as never),
      },
      {
        environmentId: healthyEnv.environmentId,
        result: AsyncResult.success({ workflows: [], runs: [], questions: [] }),
      },
    ];
    {
      using multiEnvTree = mountMobileTestTree(<WorkflowsScreen />);
      expect(multiEnvTree.container.textContent).toContain("failing-mac:");
      expect(multiEnvTree.container.textContent).toContain("No workflow runs yet");
    }

    const cardFixture = makeMobileCardFixture();
    const boardNav = createMobileNavigationStub({
      environmentId: offlineCapable.environmentId,
      projectId: cardFixture.projectId,
      cardId: cardFixture.cardId,
    });
    const recorder = createMobileCommandRecorder();
    navHolder.navigation = boardNav.navigation;
    navHolder.initialParams = boardNav.initialParams;
    envHolder.environments = [offlineCapable];
    envHolder.liveEntries = [
      {
        environmentId: offlineCapable.environmentId,
        result: AsyncResult.success({ cards: [cardFixture] }),
      },
    ];
    envHolder.queryData = cardFixture;
    envHolder.canOperate = false;
    envHolder.recorder = recorder;

    {
      using offlineTree = mountMobileTestTree(
        <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />,
      );
      expect(offlineTree.container.textContent).toContain("Environment is not connected");
      expect(findButtonMatching(offlineTree.container, /approve|pause|resume/i)).toBeUndefined();
    }

    envHolder.environments = [
      makeMobileBoardEnvironment({
        environmentId: offlineCapable.environmentId,
        projectWorkflowBoard: true,
      }),
    ];
    using roTree = mountMobileTestTree(
      <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />,
    );
    expect(findButtonMatching(roTree.container, /approve delivery/i)?.disabled).toBe(true);
    expect(findButtonMatching(roTree.container, /pause|resume/i)?.disabled).toBe(true);
    expect(roTree.container.textContent).not.toMatch(/wireframe editor|git push/i);
    expect(findButtonMatching(roTree.container, /wireframe editor|git push/i)).toBeUndefined();
    expect(recorder.calls).toHaveLength(0);
  });

  it("[F23-T2-04] resolves 10-locale boundaries, deduplicates rapid double-taps on Approve/Pause, and restores act state on unmount", () => {
    expect(SUPPORTED_LOCALES).toHaveLength(10);
    const nativeNames = new Set(SUPPORTED_LOCALES.map((loc) => nativeLanguageName(loc)));
    expect(nativeNames.size).toBe(10);

    expect(resolveLocaleSelection("system", ["zh-TW"], SUPPORTED_LOCALES, DEFAULT_LOCALE)).toEqual({
      catalogLocale: "en",
      formattingLocale: "en",
    });
    expect(
      resolveLocaleSelection("system", ["zh-Hant"], SUPPORTED_LOCALES, DEFAULT_LOCALE),
    ).toEqual({
      catalogLocale: "en",
      formattingLocale: "en",
    });
    expect(resolveLocaleSelection("system", ["pt-BR"], SUPPORTED_LOCALES, DEFAULT_LOCALE)).toEqual({
      catalogLocale: "pt-BR",
      formattingLocale: "pt-BR",
    });
    expect(resolveLocaleSelection("system", ["en-GB"], SUPPORTED_LOCALES, DEFAULT_LOCALE)).toEqual({
      catalogLocale: "en",
      formattingLocale: "en-GB",
    });
    expect(resolveLocaleSelection("xx-ZZ", [], SUPPORTED_LOCALES, DEFAULT_LOCALE)).toEqual({
      catalogLocale: "en",
      formattingLocale: "en",
    });

    const actEnv = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    const beforeAct = actEnv.IS_REACT_ACT_ENVIRONMENT;
    {
      const baselineRecorder = createMobileCommandRecorder();
      const approveBaseline = baselineRecorder.wrap("workflowBoard.approveDelivery");
      const baselineGate = baselineRecorder.deferNext("workflowBoard.approveDelivery");
      const pendingBaseline = approveBaseline({ cardId: "card-1", revision: 2 });
      try {
        {
          using _mounted = mountMobileTestTree(<span>inflight-command</span>);
          expect(baselineRecorder.calls).toHaveLength(1);
        }
        expect(actEnv.IS_REACT_ACT_ENVIRONMENT).toBe(beforeAct);
      } finally {
        baselineGate.resolve({ _tag: "Success", value: { disposed: true } });
      }
      void pendingBaseline;
    }

    const capableEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("mobile-dedup-env"),
      projectWorkflowBoard: true,
    });
    const cardFixture = makeMobileCardFixture();
    const boardNav = createMobileNavigationStub({
      environmentId: capableEnv.environmentId,
      projectId: cardFixture.projectId,
      cardId: cardFixture.cardId,
    });
    const recorder = createMobileCommandRecorder();
    const approveGate = recorder.deferNext("workflowBoard.approveDelivery");
    const pauseGate = recorder.deferNext("workflowBoard.pauseCard");
    const resumeGate = recorder.deferNext("workflowBoard.resumeCard");

    navHolder.navigation = boardNav.navigation;
    navHolder.initialParams = boardNav.initialParams;
    envHolder.environments = [capableEnv];
    envHolder.liveEntries = [
      {
        environmentId: capableEnv.environmentId,
        result: AsyncResult.success({ cards: [cardFixture] }),
      },
    ];
    envHolder.queryData = cardFixture;
    envHolder.canOperate = true;
    envHolder.recorder = recorder;

    try {
      {
        using mounted = mountMobileTestTree(
          <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />,
        );
        const targetBtn =
          findButtonMatching(mounted.container, /approve/i) ??
          findButtonMatching(mounted.container, /pause|resume/i);
        expect(targetBtn).toBeDefined();
        act(() => {
          targetBtn?.click();
        });
        const retryBtn =
          findButtonMatching(mounted.container, /approve/i) ??
          findButtonMatching(mounted.container, /pause|resume/i);
        act(() => {
          retryBtn?.click();
        });
        expect(recorder.calls).toHaveLength(1);
      }
      expect(actEnv.IS_REACT_ACT_ENVIRONMENT).toBe(beforeAct);
    } finally {
      approveGate.resolve({ _tag: "Success", value: { disposed: true } });
      pauseGate.resolve({ _tag: "Success", value: { disposed: true } });
      resumeGate.resolve({ _tag: "Success", value: { disposed: true } });
    }
  });
});
