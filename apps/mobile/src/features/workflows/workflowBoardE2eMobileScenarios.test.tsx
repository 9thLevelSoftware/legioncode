import { nativeLanguageName, resolveLocaleSelection } from "@legioncode/client-runtime/locales";
import {
  EnvironmentId,
  IsoDateTime,
  PositiveInt,
  ProjectId,
  ThreadId,
  WorkflowBoardApprovalId,
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

function resetMobileScenarioHolders(): void {
  navHolder.navigation = null;
  navHolder.initialParams = {};
  envHolder.environments = [];
  envHolder.liveEntries = [];
  envHolder.queryData = null;
  envHolder.canOperate = true;
  envHolder.recorder?.reset();
  envHolder.recorder = null;
}

function withIsolatedMobileHolders(): Disposable {
  resetMobileScenarioHolders();
  return {
    [Symbol.dispose]() {
      resetMobileScenarioHolders();
    },
  };
}

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

// The card screens read board RPCs through the client-runtime atoms in state/workflowBoard.
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

const SCENARIO_PROJECT_ID = ProjectId.make("mobile-board-project");
const SCENARIO_SNAPSHOT_DIGEST = "sha256:mobile-scenario-snapshot-1";

function makeMobileScenarioCardFixture(
  overrides: Partial<WorkflowCardDetail> = {},
): WorkflowCardDetail {
  return {
    cardId: WorkflowCardId.make("card-m5-mobile-1"),
    projectId: SCENARIO_PROJECT_ID,
    title: "Cross-Surface Mobile Card",
    stage: "ship",
    status: "active",
    revision: PositiveInt.make(4),
    currentTask: null,
    blockerSummary: null,
    nextAction: "approve_delivery",
    specApprovedRevision: PositiveInt.make(2),
    specApprovalStatus: "valid",
    activeThreadId: ThreadId.make("thr-m5-mobile-1"),
    createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
    updatedAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
    workspaceStrategy: { type: "root" },
    modelSelection: null,
    runtimeMode: null,
    interactionMode: null,
    spec: {
      title: "Cross-Surface Mobile Card",
      brief: "Verify mobile inspection and approval.",
      sourceType: "idea",
      assistanceMode: "guided",
      acceptanceCriteria: [{ id: "AC-1", text: "Verify mobile inspection and approval." }],
      constraints: ["Never auto-commit deliverables"],
      sourceRefs: ["apps/mobile/src/features/workflows/WorkflowsScreen.tsx"],
      wireframesIndex: null,
      specDigest: "sha256:mobile-scenario-spec-1",
      updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
    },
    specApproval: {
      id: WorkflowBoardApprovalId.make("appr-m5-mobile-1"),
      gate: "spec",
      status: "valid",
      revision: PositiveInt.make(2),
      artifactDigest: "sha256:mobile-scenario-spec-1",
      specDigest: "sha256:mobile-scenario-spec-1",
      wireframesDigest: null,
      approvedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      approvedBy: "client",
    },
    stageRuns: [],
    revisions: [],
    artifactKeys: [WorkflowBoardArtifactKey.make("spec.md")],
    checks: [{ id: "CHK-1", argv: ["vp", "test"] }],
    acObservations: [
      {
        criterionId: "AC-1",
        method: "manual",
        status: "pass",
        checkIds: [],
        procedure: "Inspect the card on mobile",
        observationNote: "Observed on desktop before handoff",
        evidenceRef: null,
        evidenceDigest: null,
        productSnapshotDigest: SCENARIO_SNAPSHOT_DIGEST,
        observedBy: "client",
        observedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
      },
    ],
    deliveryStatus: "ready",
    shipReadiness: {
      status: "ready",
      ready: true,
      productSnapshotDigest: SCENARIO_SNAPSHOT_DIGEST,
      blockers: [],
      deliveryReceipt: null,
    },
    ...overrides,
  };
}

function scenarioCardParams(environmentId: EnvironmentId, card: WorkflowCardDetail) {
  return { environmentId, projectId: card.projectId, cardId: card.cardId };
}

function isDisabledButton(button: HTMLButtonElement | undefined): boolean {
  return (
    button !== undefined && (button.disabled || button.getAttribute("aria-disabled") === "true")
  );
}

function findButtonMatching(
  container: HTMLElement,
  pattern: RegExp,
): HTMLButtonElement | undefined {
  return [...container.querySelectorAll("button")].find((btn) =>
    pattern.test(`${btn.getAttribute("aria-label") ?? ""} ${btn.textContent ?? ""}`),
  );
}

describe("E2E-M5 Tier 3 mobile pairwise & Tier 4 cross-surface scenarios", () => {
  it("[T3-M-01] F23 x F22: mixed capability & offline environment isolation disables Approve/Pause on read-only host", () => {
    using _holders = withIsolatedMobileHolders();
    const offlineCapable = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-offline-cap"),
      phase: "offline",
      projectWorkflowBoard: true,
    });
    const connectedLegacy = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-connected-legacy"),
      phase: "connected",
      projectWorkflowBoard: false,
    });
    expect(hasMobileProjectWorkflowBoardCapability([offlineCapable, connectedLegacy])).toBe(false);

    envHolder.environments = [connectedLegacy];
    envHolder.liveEntries = [
      {
        environmentId: connectedLegacy.environmentId,
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

    const card = makeMobileScenarioCardFixture();
    const recorder = createMobileCommandRecorder();
    const offlineNav = createMobileNavigationStub(
      scenarioCardParams(offlineCapable.environmentId, card),
    );
    navHolder.navigation = offlineNav.navigation;
    navHolder.initialParams = offlineNav.initialParams;
    envHolder.environments = [offlineCapable, connectedLegacy];
    envHolder.liveEntries = [];
    envHolder.queryData = card;
    envHolder.canOperate = true;
    envHolder.recorder = recorder;

    {
      using offlineTree = mountMobileTestTree(
        <WorkflowCardRouteScreen route={{ params: offlineNav.initialParams } as never} />,
      );
      expect(offlineTree.container.textContent).toContain("Environment is not connected");
      expect(findButtonMatching(offlineTree.container, /approve/i)).toBeUndefined();
      expect(findButtonMatching(offlineTree.container, /pause|resume/i)).toBeUndefined();
    }

    const readOnlyHost = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-read-only-host"),
      phase: "connected",
      projectWorkflowBoard: true,
    });
    const readOnlyNav = createMobileNavigationStub(
      scenarioCardParams(readOnlyHost.environmentId, card),
    );
    navHolder.navigation = readOnlyNav.navigation;
    navHolder.initialParams = readOnlyNav.initialParams;
    envHolder.environments = [offlineCapable, readOnlyHost];
    envHolder.canOperate = false;

    using roTree = mountMobileTestTree(
      <WorkflowCardRouteScreen route={{ params: readOnlyNav.initialParams } as never} />,
    );
    expect(roTree.container.textContent).toContain("Cross-Surface Mobile Card");
    const approveBtn = findButtonMatching(roTree.container, /approve delivery/i);
    const pauseBtn = findButtonMatching(roTree.container, /pause|resume/i);
    expect(isDisabledButton(approveBtn)).toBe(true);
    expect(isDisabledButton(pauseBtn)).toBe(true);
    act(() => {
      approveBtn?.click();
      pauseBtn?.click();
    });
    expect(recorder.calls).toHaveLength(0);
  });

  it("[T3-M-02] F18 x F23: mobile inspects Ship evidence/ACs, navigates to Thread, deduplicates double-tap Approve/Pause, and omits desktop authoring", async () => {
    using _holders = withIsolatedMobileHolders();
    const recorder = createMobileCommandRecorder();
    const gate = recorder.deferNext("workflowBoard.approveDelivery");
    const wrappedApprove = recorder.wrap("workflowBoard.approveDelivery");
    const p1 = wrappedApprove({ cardId: "card-m5-mobile-1" });
    gate.resolve({ _tag: "Success", value: { ok: true } });
    await p1;
    expect(recorder.calls).toHaveLength(1);
    recorder.reset();

    const capableEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-capable-env"),
      projectWorkflowBoard: true,
    });
    const card = makeMobileScenarioCardFixture();
    const boardNav = createMobileNavigationStub(scenarioCardParams(capableEnv.environmentId, card));
    const approveGate = recorder.deferNext("workflowBoard.approveDelivery");

    navHolder.navigation = boardNav.navigation;
    navHolder.initialParams = boardNav.initialParams;
    envHolder.environments = [capableEnv];
    envHolder.queryData = card;
    envHolder.canOperate = true;
    envHolder.recorder = recorder;

    try {
      using tree = mountMobileTestTree(
        <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />,
      );
      const text = tree.container.textContent ?? "";
      expect(text).toContain("Cross-Surface Mobile Card");
      expect(text).toContain("Ready to ship");
      expect(text).toContain("AC-1");
      expect(text).toContain("Observed on desktop before handoff");
      expect(text).not.toMatch(/wireframe editor|git push/i);

      const threadBtn = findButtonMatching(tree.container, /thread/i);
      expect(threadBtn).toBeDefined();
      act(() => {
        threadBtn?.click();
      });
      expect(boardNav.dispatched).toContainEqual({
        type: "PUSH",
        name: "Thread",
        params: { environmentId: capableEnv.environmentId, threadId: "thr-m5-mobile-1" },
      });

      const approveBtn = findButtonMatching(tree.container, /approve delivery/i);
      expect(isDisabledButton(approveBtn)).toBe(false);
      act(() => {
        approveBtn?.click();
      });
      const retryBtn = findButtonMatching(tree.container, /approv/i);
      expect(isDisabledButton(retryBtn)).toBe(true);
      expect(isDisabledButton(findButtonMatching(tree.container, /pause/i))).toBe(true);
      act(() => {
        retryBtn?.click();
      });
      expect(recorder.calls).toEqual([
        {
          label: "workflowBoard.approveDelivery",
          input: {
            cardId: card.cardId,
            expectedRevision: 4,
            expectedProductSnapshotDigest: SCENARIO_SNAPSHOT_DIGEST,
          },
        },
      ]);
    } finally {
      approveGate.resolve({ _tag: "Success", value: { disposed: true } });
    }
  });

  it("[T3-M-03] F1 x F23: resolves 10-locale boundaries and restores act state on unmount across WorkflowsScreen and board", () => {
    using _holders = withIsolatedMobileHolders();
    expect(SUPPORTED_LOCALES).toHaveLength(10);
    expect(new Set(SUPPORTED_LOCALES.map((loc) => nativeLanguageName(loc))).size).toBe(10);
    expect(resolveLocaleSelection("system", ["zh-TW"], SUPPORTED_LOCALES, DEFAULT_LOCALE)).toEqual({
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
      using _wfTree = mountMobileTestTree(<WorkflowsScreen />);
    }
    expect(actEnv.IS_REACT_ACT_ENVIRONMENT).toBe(beforeAct);

    const capableEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-locale-env"),
      projectWorkflowBoard: true,
    });
    const card = makeMobileScenarioCardFixture();
    const boardNav = createMobileNavigationStub(scenarioCardParams(capableEnv.environmentId, card));
    navHolder.navigation = boardNav.navigation;
    navHolder.initialParams = boardNav.initialParams;
    envHolder.environments = [capableEnv];
    envHolder.queryData = card;
    envHolder.canOperate = true;

    {
      using boardTree = mountMobileTestTree(
        <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />,
      );
      expect(boardTree.container.textContent).toContain("Cross-Surface Mobile Card");
    }
    expect(actEnv.IS_REACT_ACT_ENVIRONMENT).toBe(beforeAct);
  });

  it("[T4-M-11] Scenario 11 (Mobile): inspects desktop-created card evidence, toggles pause/resume, navigates Thread & approves delivery", async () => {
    using _holders = withIsolatedMobileHolders();
    const capableEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-s11-env"),
      projectWorkflowBoard: true,
    });
    expect(hasMobileProjectWorkflowBoardCapability([capableEnv])).toBe(true);

    // Desktop paused the card before handing it to mobile.
    const pausedCard = makeMobileScenarioCardFixture({ status: "paused" });
    const boardNav = createMobileNavigationStub(
      scenarioCardParams(capableEnv.environmentId, pausedCard),
    );
    const recorder = createMobileCommandRecorder();
    navHolder.navigation = boardNav.navigation;
    navHolder.initialParams = boardNav.initialParams;
    envHolder.environments = [capableEnv];
    envHolder.queryData = pausedCard;
    envHolder.canOperate = true;
    envHolder.recorder = recorder;

    const renderCard = () => (
      <WorkflowCardRouteScreen route={{ params: boardNav.initialParams } as never} />
    );
    using tree = mountMobileTestTree(renderCard());
    expect(tree.container.textContent).toContain("Observed on desktop before handoff");
    // A paused card cannot ship until it is resumed.
    expect(isDisabledButton(findButtonMatching(tree.container, /approve delivery/i))).toBe(true);

    const resumeBtn = findButtonMatching(tree.container, /resume/i);
    expect(isDisabledButton(resumeBtn)).toBe(false);
    await act(async () => {
      resumeBtn?.click();
    });

    // The server pushes the resumed card at the next revision.
    const resumedCard = makeMobileScenarioCardFixture({
      status: "active",
      revision: PositiveInt.make(5),
    });
    envHolder.queryData = resumedCard;
    tree.rerender(renderCard());
    expect(findButtonMatching(tree.container, /resume/i)).toBeUndefined();
    expect(isDisabledButton(findButtonMatching(tree.container, /pause/i))).toBe(false);

    const threadBtn = findButtonMatching(tree.container, /thread/i);
    act(() => {
      threadBtn?.click();
    });
    expect(boardNav.dispatched).toContainEqual({
      type: "PUSH",
      name: "Thread",
      params: { environmentId: capableEnv.environmentId, threadId: "thr-m5-mobile-1" },
    });

    const approveBtn = findButtonMatching(tree.container, /approve delivery/i);
    expect(isDisabledButton(approveBtn)).toBe(false);
    await act(async () => {
      approveBtn?.click();
    });

    expect(recorder.calls).toEqual([
      {
        label: "workflowBoard.resumeCard",
        input: { cardId: pausedCard.cardId, expectedRevision: 4 },
      },
      {
        label: "workflowBoard.approveDelivery",
        input: {
          cardId: resumedCard.cardId,
          expectedRevision: 5,
          expectedProductSnapshotDigest: SCENARIO_SNAPSHOT_DIGEST,
        },
      },
    ]);
  });

  it("[T4-M-12] Scenario 12 (Mobile): legacy server without projectWorkflowBoard keeps WorkflowsScreen usable & disables board entry", () => {
    using _holders = withIsolatedMobileHolders();
    const legacyEnv = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("m5-legacy-env"),
      projectWorkflowBoard: false,
    });
    expect(hasMobileProjectWorkflowBoardCapability([legacyEnv])).toBe(false);

    const navStub = createMobileNavigationStub();
    const recorder = createMobileCommandRecorder();
    navHolder.navigation = navStub.navigation;
    navHolder.initialParams = {};
    envHolder.environments = [legacyEnv];
    envHolder.liveEntries = [
      {
        environmentId: legacyEnv.environmentId,
        result: AsyncResult.success({ workflows: [], runs: [], questions: [] }),
      },
    ];
    envHolder.queryData = makeMobileScenarioCardFixture();
    envHolder.canOperate = true;
    envHolder.recorder = recorder;

    {
      using tree = mountMobileTestTree(<WorkflowsScreen />);
      expect(tree.container.textContent).toContain("No workflow runs yet");
    }

    const card = makeMobileScenarioCardFixture();
    using legacyBoard = mountMobileTestTree(
      <WorkflowCardRouteScreen
        route={{ params: scenarioCardParams(legacyEnv.environmentId, card) } as never}
      />,
    );
    const text = legacyBoard.container.textContent ?? "";
    expect(text).toContain("Workflow board is not supported");
    expect(text).not.toContain("Cross-Surface Mobile Card");
    expect(findButtonMatching(legacyBoard.container, /approve|pause|resume/i)).toBeUndefined();
    expect(recorder.calls).toHaveLength(0);
  });
});
