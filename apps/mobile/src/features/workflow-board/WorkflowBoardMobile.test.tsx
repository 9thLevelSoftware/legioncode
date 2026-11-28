import {
  DEFAULT_SERVER_SETTINGS,
  EnvironmentId,
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProjectId,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WorkflowBoardNonGitArtifactKey,
  WorkflowBoardRequestError,
  WorkflowCardId,
  type ProjectWorkflowBoardSnapshot,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

const { Children, mocks } = vi.hoisted(() => ({
  Children: ({ children }: { children?: ReactNode }) => children,
  mocks: {
    isReady: true,
    environments: [] as Array<{
      environmentId: EnvironmentId;
      label: string;
      connection: { phase: "connected" | "connecting" | "disconnected" };
      serverConfig: {
        settings: typeof DEFAULT_SERVER_SETTINGS;
        environment: { capabilities: Record<string, boolean | undefined> };
      } | null;
    }>,
    projects: [] as Array<{
      environmentId: EnvironmentId;
      id: ProjectId;
      title: string;
      workspaceRoot: string;
      archivedAt: null;
    }>,
    canReadBoard: true,
    canOperate: true,
    snapshotData: null as ProjectWorkflowBoardSnapshot | null,
    snapshotFailure: null as unknown,
    snapshotPending: false,
    cardData: null as WorkflowCardDetail | null,
    cardFailure: null as unknown,
    cardDiffData: null as unknown,
    artifactData: null as unknown,
    navigationDispatch: vi.fn(),
    openSettings: vi.fn(),
    startOpeningEnvironmentThread: vi.fn(),
    confirmAutoApprove: true,
    pendingConfirmCallbacks: null as {
      readonly onConfirm: () => void;
      readonly onCancel?: () => void;
    } | null,
    deferConfirm: false,
    confirmCalls: [] as Array<{ title: string }>,
    commandMocks: {
      approveSpec: vi.fn(),
      approvePlan: vi.fn(),
      approveDelivery: vi.fn(),
      pauseCard: vi.fn(),
      resumeCard: vi.fn(),
      retryStep: vi.fn(),
      returnForRevision: vi.fn(),
      recordManualAcceptance: vi.fn(),
      resolveFinding: vi.fn(),
      updateSettings: vi.fn(),
    },
  },
}));

vi.mock("react-native", () => ({
  Platform: { OS: "ios", Version: 18 },
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  ScrollView: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Pressable: ({
    children,
    disabled,
    onPress,
    accessibilityLabel,
  }: {
    children?: ReactNode;
    disabled?: boolean;
    onPress?: () => void;
    accessibilityLabel?: string;
  }) => (
    <button type="button" disabled={disabled} aria-label={accessibilityLabel} onClick={onPress}>
      {children}
    </button>
  ),
}));

vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ top: 0, right: 0, bottom: 0, left: 0 }),
}));

vi.mock("@react-navigation/native", () => ({
  StackActions: {
    push: (name: string, params?: unknown) => ({ type: "PUSH", payload: { name, params } }),
  },
  useNavigation: () => ({
    canGoBack: () => true,
    goBack: vi.fn(),
    dispatch: mocks.navigationDispatch,
  }),
}));

vi.mock("@effect/atom-react", () => ({
  useAtomValue: (atom: { readonly _tag?: string; readonly kind?: string }) => {
    if (atom?.kind === "read") return mocks.canReadBoard;
    if (atom?.kind === "operate") return mocks.canOperate;
    return new Set(mocks.environments.map((e) => e.environmentId));
  },
  useAtomSet: () => () => {},
}));

vi.mock("../../state/environments", () => ({
  useEnvironments: () => ({
    isReady: mocks.isReady,
    networkStatus: "online",
    environments: mocks.environments,
    presentationById: new Map(mocks.environments.map((e) => [e.environmentId, e])),
  }),
}));

vi.mock("../../state/entities", () => ({
  useProjects: () => mocks.projects,
}));

vi.mock("../../state/threads", () => ({
  startOpeningEnvironmentThread: mocks.startOpeningEnvironmentThread,
}));

vi.mock("../../state/session", () => ({
  useEnvironmentsWithScope: (envs: readonly { environmentId: EnvironmentId }[]) =>
    new Set(envs.map((e) => e.environmentId)),
}));

vi.mock("../../state/preferences", () => ({
  mobilePreferencesAtom: {},
  saveMobilePreferences: {},
}));

vi.mock("../../state/server", () => ({
  serverEnvironment: {
    updateSettings: { kind: "updateSettings" },
  },
}));

vi.mock("../../state/workflowBoard", () => ({
  workflowBoard: {
    readPermissionAtom: () => ({ kind: "read" }),
    boardSnapshot: () => ({ kind: "boardSnapshot" }),
    cardDetail: () => ({ kind: "cardDetail" }),
    artifact: () => ({ kind: "artifact" }),
    cardDiff: () => ({ kind: "cardDiff" }),
    approveSpec: { kind: "approveSpec", permissionAtom: () => ({ kind: "operate" }) },
    approvePlan: { kind: "approvePlan", permissionAtom: () => ({ kind: "operate" }) },
    approveDelivery: { kind: "approveDelivery", permissionAtom: () => ({ kind: "operate" }) },
    pauseCard: { kind: "pauseCard", permissionAtom: () => ({ kind: "operate" }) },
    resumeCard: { kind: "resumeCard", permissionAtom: () => ({ kind: "operate" }) },
    retryStep: { kind: "retryStep", permissionAtom: () => ({ kind: "operate" }) },
    returnForRevision: { kind: "returnForRevision", permissionAtom: () => ({ kind: "operate" }) },
    recordManualAcceptance: {
      kind: "recordManualAcceptance",
      permissionAtom: () => ({ kind: "operate" }),
    },
    resolveFinding: { kind: "resolveFinding", permissionAtom: () => ({ kind: "operate" }) },
  },
}));

vi.mock("../../state/query", () => ({
  useEnvironmentQuery: (atom: { readonly kind?: string } | null) => {
    if (atom === null) {
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
    if (atom.kind === "boardSnapshot") {
      return {
        data: mocks.snapshotData,
        successResult: null,
        error: null,
        failure: mocks.snapshotFailure,
        isPending: mocks.snapshotPending,
        isSuccess: mocks.snapshotData !== null,
        refresh: vi.fn(),
      };
    }
    if (atom.kind === "cardDetail") {
      return {
        data: mocks.cardData,
        successResult: null,
        error: null,
        failure: mocks.cardFailure,
        isPending: false,
        isSuccess: mocks.cardData !== null,
        refresh: vi.fn(),
      };
    }
    if (atom.kind === "cardDiff") {
      return {
        data: mocks.cardDiffData,
        successResult: null,
        error: null,
        failure: null,
        isPending: false,
        isSuccess: mocks.cardDiffData !== null,
        refresh: vi.fn(),
      };
    }
    return {
      data: mocks.artifactData,
      successResult: null,
      error: null,
      failure: null,
      isPending: false,
      isSuccess: mocks.artifactData !== null,
      refresh: vi.fn(),
    };
  },
}));

vi.mock("../../state/use-atom-command", () => ({
  useAtomCommand: (cmd: { readonly kind?: string }) => {
    const key = cmd?.kind as keyof typeof mocks.commandMocks | undefined;
    return (key && mocks.commandMocks[key]) || vi.fn();
  },
}));

vi.mock("../settings/settings-navigation", () => ({
  openSettings: mocks.openSettings,
}));

vi.mock("../settings/settings-environment-filter", () => ({
  useSettingsEnvironmentFilter: () => ({
    selectedTargets: [],
    projectGroups: [],
    selectedProjectKey: null,
  }),
}));

vi.mock("../settings/components/AutoSettleDaysField", () => ({ AutoSettleDaysField: () => null }));
vi.mock("../settings/components/SettingsScreen", () => ({ SettingsScreen: Children }));
vi.mock("../settings/components/SettingsSection", () => ({
  SettingsSection: ({ title, children }: { title?: string; children?: ReactNode }) => (
    <section>
      {title ? <h2>{title}</h2> : null}
      {children}
    </section>
  ),
}));
vi.mock("../settings/components/SettingsSwitchRow", () => ({
  SettingsSwitchRow: ({
    label,
    value,
    disabled,
    onValueChange,
  }: {
    label: string;
    value: boolean;
    disabled?: boolean;
    onValueChange: (next: boolean) => void;
  }) => (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      disabled={disabled}
      onClick={() => onValueChange(!value)}
    >
      {label}
    </button>
  ),
}));

vi.mock("../../components/ui/AppBar", () => ({
  AppBar: ({ title, subtitle }: { title: string; subtitle?: string | null }) => (
    <header>
      <h1>{title}</h1>
      {subtitle ? <span>{subtitle}</span> : null}
    </header>
  ),
}));
vi.mock("../../components/ui/AppSymbol", () => ({ SymbolView: () => null }));
vi.mock("../../components/ui/AppText", () => ({
  AppText: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  AppTextInput: ({
    value,
    onChangeText,
    placeholder,
    accessibilityLabel,
    editable,
  }: {
    value: string;
    onChangeText: (v: string) => void;
    placeholder?: string;
    accessibilityLabel?: string;
    editable?: boolean;
  }) => (
    <input
      aria-label={accessibilityLabel}
      placeholder={placeholder}
      value={value}
      disabled={editable === false}
      onChange={(e) => onChangeText(e.target.value)}
    />
  ),
}));
vi.mock("../../components/ui/Button", () => ({
  Button: ({
    label,
    disabled,
    onPress,
  }: {
    label: string;
    disabled?: boolean;
    onPress: () => void;
  }) => (
    <button type="button" disabled={disabled} onClick={onPress}>
      {label}
    </button>
  ),
}));
vi.mock("../../components/ui/Chip", () => ({
  Chip: ({
    label,
    selected,
    onPress,
  }: {
    label: string;
    selected?: boolean;
    onPress?: () => void;
  }) => (
    <button type="button" aria-pressed={selected} onClick={onPress}>
      {label}
    </button>
  ),
}));
vi.mock("../../components/ui/Dialog", () => ({
  confirm: (req: { title: string; onConfirm: () => void; onCancel?: () => void }) => {
    mocks.confirmCalls.push({ title: req.title });
    if (mocks.deferConfirm) {
      mocks.pendingConfirmCallbacks = {
        onConfirm: req.onConfirm,
        ...(req.onCancel ? { onCancel: req.onCancel } : {}),
      };
      return;
    }
    if (mocks.confirmAutoApprove) req.onConfirm();
    else req.onCancel?.();
  },
}));
vi.mock("../../components/ui/EmptyState", () => ({
  EmptyState: ({
    title,
    detail,
    actionLabel,
    onAction,
  }: {
    title: string;
    detail: string;
    actionLabel?: string;
    onAction?: () => void;
  }) => (
    <div>
      <h2>{title}</h2>
      <p>{detail}</p>
      {actionLabel && onAction ? (
        <button type="button" onClick={onAction}>
          {actionLabel}
        </button>
      ) : null}
    </div>
  ),
}));
vi.mock("../../components/ui/ListChoiceRow", () => ({ ListChoiceRow: () => null }));
vi.mock("../../components/ui/ListSection", () => ({
  ListSection: ({ title, children }: { title?: string; children?: ReactNode }) => (
    <section>
      {title ? <h3>{title}</h3> : null}
      {children}
    </section>
  ),
}));
vi.mock("../../components/ui/Notice", () => ({
  Notice: ({ message }: { message: string }) => <div role="status">{message}</div>,
}));
vi.mock("../../components/ui/ScreenContent", () => ({ ScreenContent: Children }));
vi.mock("../../components/ui/ScreenScrollView", () => ({ ScreenScrollView: Children }));
vi.mock("../../components/ui/SegmentedControl", () => ({
  SegmentedControl: ({
    options,
    selected,
    onSelect,
  }: {
    options: readonly { value: string; label: string }[];
    selected: string;
    onSelect: (value: string) => void;
  }) => (
    <div role="tablist">
      {options.map((opt) => (
        <button
          key={opt.value}
          type="button"
          role="tab"
          aria-selected={selected === opt.value}
          onClick={() => onSelect(opt.value)}
        >
          {opt.label}
        </button>
      ))}
    </div>
  ),
}));
vi.mock("../../components/StatusPill", () => ({
  StatusPill: ({ label }: { label: string }) => <span>{label}</span>,
}));

import { SettingsLabsRouteScreen } from "../settings/ThreadSettingsSections";
import { WorkflowBoardRouteScreen } from "./WorkflowBoardRouteScreen";
import { WorkflowCardRouteScreen } from "./WorkflowCardRouteScreen";

const ENV_1 = EnvironmentId.make("env-1");
const PROJ_1 = ProjectId.make("proj-1");
const CARD_1 = WorkflowCardId.make("card-1");

function makeDetail(overrides: Partial<WorkflowCardDetail> = {}): WorkflowCardDetail {
  return {
    cardId: CARD_1,
    projectId: PROJ_1,
    title: "Mobile feature delivery",
    stage: "spec",
    status: "active",
    revision: PositiveInt.make(2),
    currentTask: null,
    blockerSummary: null,
    nextAction: "approve_spec",
    specApprovedRevision: null,
    specApprovalStatus: null,
    activeThreadId: ThreadId.make("thread-spec"),
    createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
    updatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
    workspaceStrategy: { type: "root" },
    modelSelection: null,
    runtimeMode: null,
    interactionMode: null,
    spec: {
      title: "Mobile feature delivery",
      brief: "Inspect cards and approve from mobile.",
      sourceType: "idea",
      assistanceMode: "balanced",
      acceptanceCriteria: [{ id: "AC-1", text: "Mobile shows cards and approves spec." }],
      constraints: ["Never bypass auth"],
      sourceRefs: ["apps/mobile/src/Stack.tsx"],
      wireframesIndex: null,
      specDigest: "sha256:spec",
      updatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
    },
    specApproval: null,
    stageRuns: [],
    revisions: [
      {
        revision: PositiveInt.make(1),
        stage: "spec",
        kind: "card_created",
        summary: "Created card",
        specDigest: "sha256:spec",
        createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
      },
    ],
    artifactKeys: [WorkflowBoardArtifactKey.make("spec.md")],
    ...overrides,
  };
}

let container: HTMLDivElement;
let root: Root;
const previousActEnv = (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean })
  .IS_REACT_ACT_ENVIRONMENT;

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);

  mocks.isReady = true;
  mocks.environments = [
    {
      environmentId: ENV_1,
      label: "Local Host",
      connection: { phase: "connected" },
      serverConfig: {
        settings: { ...DEFAULT_SERVER_SETTINGS, workflowsEnabled: true },
        environment: { capabilities: { projectWorkflowBoard: true } },
      },
    },
  ];
  mocks.projects = [
    {
      environmentId: ENV_1,
      id: PROJ_1,
      title: "t3code",
      workspaceRoot: "/repo/t3code",
      archivedAt: null,
    },
  ];
  mocks.canReadBoard = true;
  mocks.canOperate = true;
  mocks.snapshotData = null;
  mocks.snapshotFailure = null;
  mocks.snapshotPending = false;
  mocks.cardData = null;
  mocks.cardFailure = null;
  mocks.cardDiffData = null;
  mocks.artifactData = null;
  mocks.confirmAutoApprove = true;
  mocks.deferConfirm = false;
  mocks.pendingConfirmCallbacks = null;
  mocks.confirmCalls = [];
  vi.clearAllMocks();
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = previousActEnv;
});

function clickButton(label: string) {
  const button = [...container.querySelectorAll("button")].find((btn) =>
    btn.textContent?.includes(label),
  );
  if (!button) throw new Error(`Button "${label}" not found in: ${container.textContent}`);
  act(() => {
    button.click();
  });
  return button;
}

describe("WorkflowBoard mobile screens", () => {
  it("gates WorkflowBoardRouteScreen across loading, unsupported, labs-disabled, read-only, and enabled states", () => {
    // 1. Connecting / cold load holds loading without flashing Project not found
    mocks.isReady = false;
    mocks.projects = [];
    act(() => {
      root.render(
        <WorkflowBoardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1 },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Loading workflow board…");
    expect(container.textContent).not.toContain("Project not found");

    // 2. Unsupported legacy server
    mocks.isReady = true;
    mocks.projects = [
      {
        environmentId: ENV_1,
        id: PROJ_1,
        title: "t3code",
        workspaceRoot: "/repo/t3code",
        archivedAt: null,
      },
    ];
    mocks.environments[0]!.serverConfig = {
      settings: { ...DEFAULT_SERVER_SETTINGS, workflowsEnabled: true },
      environment: { capabilities: {} },
    };
    act(() => {
      root.render(
        <WorkflowBoardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1 },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Workflow board is not supported");

    // 3. Labs disabled -> Open Labs settings CTA
    mocks.environments[0]!.serverConfig = {
      settings: { ...DEFAULT_SERVER_SETTINGS, workflowsEnabled: false },
      environment: { capabilities: { projectWorkflowBoard: true } },
    };
    act(() => {
      root.render(
        <WorkflowBoardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1 },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Workflows are off");
    clickButton("Open Labs settings");
    expect(mocks.openSettings).toHaveBeenCalledWith(expect.anything(), "SettingsLabs");

    // 4. Read permission denied
    mocks.environments[0]!.serverConfig = {
      settings: { ...DEFAULT_SERVER_SETTINGS, workflowsEnabled: true },
      environment: { capabilities: { projectWorkflowBoard: true } },
    };
    mocks.canReadBoard = false;
    act(() => {
      root.render(
        <WorkflowBoardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1 },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Read permission required");

    // 5. Enabled board renders cards and pushes WorkflowCard on tap
    mocks.canReadBoard = true;
    mocks.snapshotData = {
      projectId: PROJ_1,
      revision: NonNegativeInt.make(2),
      generatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
      cards: [
        {
          cardId: CARD_1,
          projectId: PROJ_1,
          title: "Mobile feature delivery",
          stage: "execute",
          status: "blocked",
          revision: PositiveInt.make(4),
          currentTask: "TSK-2",
          blockerSummary: "Check failed",
          nextAction: "retry_check",
          specApprovedRevision: PositiveInt.make(2),
          specApprovalStatus: "valid",
          planApprovedRevision: PositiveInt.make(3),
          planApprovalStatus: "valid",
          activeThreadId: ThreadId.make("thread-1"),
          createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
          hasUnreconciledEdits: true,
        },
      ],
    };
    act(() => {
      root.render(
        <WorkflowBoardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1 },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Mobile feature delivery");
    expect(container.textContent).toContain("Check failed");
    expect(container.textContent).toContain("Needs reconciliation");
    clickButton("Mobile feature delivery");
    expect(mocks.navigationDispatch).toHaveBeenCalledWith({
      type: "PUSH",
      payload: {
        name: "WorkflowCard",
        params: {
          environmentId: ENV_1,
          projectId: PROJ_1,
          cardId: CARD_1,
          stage: "execute",
        },
      },
    });
  });

  it("renders SettingsLabsRouteScreen Workflows toggle and updates settings", async () => {
    mocks.commandMocks.updateSettings.mockResolvedValue({ _tag: "Success", value: {} });
    act(() => {
      root.render(<SettingsLabsRouteScreen />);
    });
    const switches = [...container.querySelectorAll('[role="switch"]')];
    expect(switches.length).toBeGreaterThan(0);
    await act(async () => {
      (switches.at(-1) as HTMLButtonElement).click();
    });
    expect(mocks.commandMocks.updateSettings).toHaveBeenCalledWith({
      environmentId: ENV_1,
      input: { patch: { workflowsEnabled: false } },
    });
  });

  it("navigates to linked Thread, enforces double-tap guard on approveSpec, and surfaces RPC errors", async () => {
    const deferred = Promise.withResolvers<{
      _tag: "Success" | "Failure";
      cause?: Cause.Cause<unknown>;
    }>();
    mocks.commandMocks.approveSpec.mockReturnValue(deferred.promise);
    mocks.cardData = makeDetail();

    act(() => {
      root.render(
        <WorkflowCardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1, cardId: CARD_1 },
          }}
        />,
      );
    });

    // Linked thread navigation uses StackActions.push("Thread", ...)
    clickButton("Open active thread");
    expect(mocks.startOpeningEnvironmentThread).toHaveBeenCalledWith(ENV_1, "thread-spec");
    expect(mocks.navigationDispatch).toHaveBeenCalledWith({
      type: "PUSH",
      payload: {
        name: "Thread",
        params: { environmentId: ENV_1, threadId: "thread-spec" },
      },
    });

    // Double-tap guard on approveSpec
    const approveBtn = clickButton("Approve spec");
    act(() => {
      approveBtn.click();
    });
    expect(mocks.commandMocks.approveSpec).toHaveBeenCalledTimes(1);
    expect(mocks.commandMocks.approveSpec).toHaveBeenCalledWith({
      environmentId: ENV_1,
      input: {
        cardId: CARD_1,
        expectedRevision: 2,
        expectedDigest: "sha256:spec",
      },
    });

    // Surface revision_conflict error via workflowBoardErrorMessage
    await act(async () => {
      deferred.resolve({
        _tag: "Failure",
        cause: Cause.fail(
          new WorkflowBoardRequestError({
            code: "revision_conflict",
            message: "stale rev",
            currentRevision: PositiveInt.make(3),
          }),
        ),
      });
    });
    expect(container.textContent).toContain("revision 3");
  });

  it("supports Execute pause/reconcile-resume/retry and Ship manual acceptance + delivery approval", async () => {
    mocks.commandMocks.resumeCard.mockResolvedValue({
      _tag: "Success",
      value: { reconciledCount: 1 },
    });
    mocks.commandMocks.retryStep.mockResolvedValue({ _tag: "Success", value: {} });
    mocks.commandMocks.recordManualAcceptance.mockResolvedValue({ _tag: "Success", value: {} });
    mocks.commandMocks.approveDelivery.mockResolvedValue({ _tag: "Success", value: {} });

    mocks.cardData = makeDetail({
      stage: "ship",
      status: "paused",
      hasUnreconciledEdits: true,
      specApprovalStatus: "valid",
      specApproval: {
        id: WorkflowBoardApprovalId.make("a-spec"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(2),
        artifactDigest: "sha256:spec",
        specDigest: "sha256:spec",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-27T00:10:00.000Z"),
        approvedBy: "client",
      },
      planApprovalStatus: "valid",
      tasks: [
        {
          taskId: "TSK-1",
          title: "Implement mobile",
          filesAllowed: [WorkflowBoardNonGitArtifactKey.make("apps/mobile/src/Stack.tsx")],
          testingMethod: "test-first",
          status: "failed",
        },
      ],
      taskExecutions: [
        {
          taskId: "TSK-1",
          status: "failed",
          threadId: ThreadId.make("thread-task"),
          runId: null,
          stageRunId: null,
          worktreePath: null,
          branch: null,
          attempt: PositiveInt.make(1),
          startedAt: null,
          endedAt: null,
          lastError: "Assertion failed",
        },
      ],
      shipReadiness: {
        status: "ready",
        ready: true,
        productSnapshotDigest: "snap-42",
        blockers: [],
        deliveryReceipt: null,
      },
    });

    act(() => {
      root.render(
        <WorkflowCardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1, cardId: CARD_1 },
          }}
        />,
      );
    });

    // Reconcile & resume when paused with recovery needed
    await act(async () => {
      clickButton("Reconcile & resume");
    });
    expect(mocks.commandMocks.resumeCard).toHaveBeenCalledWith({
      environmentId: ENV_1,
      input: { cardId: CARD_1, expectedRevision: 2, reconcileInterrupted: true },
    });

    // Record manual acceptance in Ship tab (card is paused? Wait: when active, Save observation shows!)
    mocks.cardData = { ...mocks.cardData, status: "active", hasUnreconciledEdits: false };
    act(() => {
      root.render(
        <WorkflowCardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1, cardId: CARD_1 },
          }}
        />,
      );
    });

    await act(async () => {
      clickButton("Save observation");
    });
    expect(mocks.commandMocks.recordManualAcceptance).toHaveBeenCalledWith({
      environmentId: ENV_1,
      input: {
        cardId: CARD_1,
        expectedRevision: 2,
        criterionId: "AC-1",
        verdict: "pass",
        expectedProductSnapshotDigest: "snap-42",
      },
    });

    await act(async () => {
      clickButton("Approve delivery");
    });
    expect(mocks.commandMocks.approveDelivery).toHaveBeenCalledWith({
      environmentId: ENV_1,
      input: {
        cardId: CARD_1,
        expectedRevision: 2,
        expectedProductSnapshotDigest: "snap-42",
      },
    });

    // Switch to Execute tab and retry failed task
    clickButton("Execute");
    await act(async () => {
      clickButton("Retry task");
    });
    expect(mocks.commandMocks.retryStep).toHaveBeenCalledWith({
      environmentId: ENV_1,
      input: {
        cardId: CARD_1,
        expectedRevision: 2,
        stepKind: "task",
        targetId: "TSK-1",
      },
    });
  });

  it("switches cleanly to an EnvironmentId containing ':' without losing the selected project", () => {
    const COLON_ENV = EnvironmentId.make("peer:env-1");
    const COLON_PROJ = ProjectId.make("proj-colon");
    mocks.environments = [
      {
        environmentId: ENV_1,
        label: "Env One",
        connection: { phase: "connected" },
        serverConfig: {
          settings: { ...DEFAULT_SERVER_SETTINGS, workflowsEnabled: true },
          environment: { capabilities: { projectWorkflowBoard: true } },
        },
      },
      {
        environmentId: COLON_ENV,
        label: "Colon Env",
        connection: { phase: "connected" },
        serverConfig: {
          settings: { ...DEFAULT_SERVER_SETTINGS, workflowsEnabled: true },
          environment: { capabilities: { projectWorkflowBoard: true } },
        },
      },
    ];
    mocks.projects = [
      {
        environmentId: ENV_1,
        id: PROJ_1,
        title: "Alpha",
        workspaceRoot: "/repo/alpha",
        archivedAt: null,
      },
      {
        environmentId: COLON_ENV,
        id: COLON_PROJ,
        title: "Beta Colon",
        workspaceRoot: "/repo/beta",
        archivedAt: null,
      },
    ];
    mocks.snapshotData = {
      projectId: COLON_PROJ,
      revision: NonNegativeInt.make(1),
      generatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
      cards: [
        {
          cardId: CARD_1,
          projectId: COLON_PROJ,
          title: "Colon environment card",
          stage: "spec",
          status: "active",
          revision: PositiveInt.make(1),
          currentTask: null,
          blockerSummary: null,
          nextAction: "approve_spec",
          specApprovedRevision: null,
          specApprovalStatus: null,
          activeThreadId: null,
          createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
        },
      ],
    };

    act(() => {
      root.render(<WorkflowBoardRouteScreen route={{ params: undefined }} />);
    });
    clickButton("Beta Colon (Colon Env)");
    expect(container.textContent).not.toContain("Project not found");
    expect(container.textContent).toContain("Beta Colon · Colon Env");
    expect(container.textContent).toContain("Colon environment card");
  });

  it("renders Project not found fallback without crashing on blank WorkflowCardRouteScreen params", () => {
    act(() => {
      root.render(
        <WorkflowCardRouteScreen
          route={{
            params: { environmentId: "", projectId: PROJ_1, cardId: CARD_1 },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Project not found");

    act(() => {
      root.render(
        <WorkflowCardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: "   ", cardId: "" },
          }}
        />,
      );
    });
    expect(container.textContent).toContain("Project not found");
  });

  it("prevents duplicate Dialog.confirm calls while a dialog or action is pending", async () => {
    const deferred = Promise.withResolvers<{ _tag: "Success" }>();
    mocks.commandMocks.pauseCard.mockReturnValue(deferred.promise);
    mocks.cardData = makeDetail();
    mocks.deferConfirm = true;

    act(() => {
      root.render(
        <WorkflowCardRouteScreen
          route={{
            params: { environmentId: ENV_1, projectId: PROJ_1, cardId: CARD_1 },
          }}
        />,
      );
    });

    // Double-tapping Approve spec before native dialog resolves opens only one dialog
    const approveBtn = clickButton("Approve spec");
    act(() => {
      approveBtn.click();
    });
    expect(mocks.confirmCalls).toHaveLength(1);

    // Cancelling releases the dialog guard so user can open it again
    act(() => {
      mocks.pendingConfirmCallbacks?.onCancel?.();
    });
    act(() => {
      approveBtn.click();
    });
    expect(mocks.confirmCalls).toHaveLength(2);
    act(() => {
      mocks.pendingConfirmCallbacks?.onCancel?.();
    });

    // Same-tick Approve spec while Pause is in-flight does not open Dialog.confirm
    const pauseBtn = [...container.querySelectorAll("button")].find((btn) =>
      btn.textContent?.includes("Pause"),
    )!;
    act(() => {
      pauseBtn.click();
      approveBtn.click();
    });
    expect(mocks.commandMocks.pauseCard).toHaveBeenCalledTimes(1);
    expect(mocks.confirmCalls).toHaveLength(2);

    await act(async () => {
      deferred.resolve({ _tag: "Success" });
    });
  });
});
