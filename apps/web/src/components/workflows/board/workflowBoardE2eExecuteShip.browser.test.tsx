import { createMemoryHistory } from "@tanstack/react-router";
import {
  AuthStandardClientScopes,
  IsoDateTime,
  RunId,
  NonNegativeInt,
  PositiveInt,
  ProviderInstanceId,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WorkflowBoardStageRunId,
  type WorkflowBoardApproveDeliveryResult,
  type WorkflowBoardApprovePlanResult,
  type WorkflowBoardPlanAmendmentRecord,
  type WorkflowBoardPlanDocument,
  type WorkflowBoardProposeAmendmentInput,
  type WorkflowBoardProposeAmendmentResult,
  type WorkflowBoardStartStageInput,
  type WorkflowBoardStartStageResult,
  type WorkflowBoardDeliveryReceipt,
  type WorkflowBoardRecordManualAcceptanceInput,
  type WorkflowBoardRecordManualAcceptanceResult,
  type WorkflowBoardResolveFindingInput,
  type WorkflowBoardResolveFindingResult,
  type WorkflowBoardRetryStepInput,
  type WorkflowBoardRetryStepResult,
  type WorkflowBoardGetCardDiffResult,
  type WorkflowBoardShipBlocker,
  type WorkflowCardDetail,
  SourceControlProviderKind,
  WS_METHODS,
  type VcsStatusLocalResult,
  type VcsStatusRemoteResult,
  type WorkflowRun,
  type WorkflowStep,
} from "@legioncode/contracts";
import { DEFAULT_CLIENT_SETTINGS } from "@legioncode/contracts/settings";
import type * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as Option from "effect/Option";
import * as Stream from "effect/Stream";
import { describe, expect, it, vi } from "vite-plus/test";
import { page as browserPage, userEvent } from "vite-plus/test/browser";

import { AppRoot } from "~/AppRoot";
import { openCommandPalette } from "~/commandPaletteBus";
import { useCommandPaletteStore } from "~/commandPaletteStore";
import "~/components/ChatView";
import { __setClientSettingsForTests } from "~/hooks/useSettings";
import "~/index.css";
import { getRouter } from "~/router";
import { appAtomRegistry } from "~/rpc/atomRegistry";
import { environmentSession } from "~/state/session";
import { vcsEnvironment } from "~/state/vcs";
import {
  ENVIRONMENT_ID,
  PROJECT_ID,
  answerRpc,
  installFakeEnvironment,
  threadWithHistory,
  type FakeDriver,
  type FakeThread,
} from "../../../../test/fakeEnvironment";
import { act, render } from "../../../../test/render";
import {
  hasProjectWorkflowBoardCapability,
  makeBoardServerConfigFixture,
  withDesktopViewport,
} from "./workflowBoardE2eHarness";

import {
  BOARD_AT,
  boardRequestError,
  installWorkflowBoardFixtures,
  makeBoardCardDetail,
  type WorkflowBoardFixtureStore,
} from "./workflowBoardE2eFixtures";

const boardEnvState = vi.hoisted(() => ({
  workflowsEnabled: false,
  projectWorkflowBoard: false,
  emitConfigSnapshot: null as (() => void) | null,
}));

vi.mock(
  "../../../../../../packages/client-runtime/src/connection/driver.ts",
  async (importOriginal) => {
    const [actual, { fakeDriver }, Layer, EffectMod, QueueMod, StreamMod] = await Promise.all([
      importOriginal<{ readonly ConnectionDriver: Context.Key<unknown, FakeDriver> }>(),
      import("../../../../test/fakeEnvironment"),
      import("effect/Layer"),
      import("effect/Effect"),
      import("effect/Queue"),
      import("effect/Stream"),
    ]);

    const buildAugmentedConfig = (base: Record<string, unknown>) => {
      const env = (base.environment ?? {}) as Record<string, unknown>;
      const caps = (env.capabilities ?? {}) as Record<string, unknown>;
      const settings = (base.settings ?? {}) as Record<string, unknown>;
      return {
        ...base,
        environment: {
          ...env,
          capabilities: {
            ...caps,
            projectWorkflowBoard: boardEnvState.projectWorkflowBoard,
          },
        },
        settings: {
          ...settings,
          workflowsEnabled: boardEnvState.workflowsEnabled,
        },
      };
    };

    const wrappedDriver: FakeDriver = {
      ...fakeDriver,
      connect: (entry) =>
        EffectMod.map(fakeDriver.connect(entry), (lease) => ({
          ...lease,
          session: {
            ...lease.session,
            initialConfig: EffectMod.map(lease.session.initialConfig, (cfg) =>
              buildAugmentedConfig(cfg as unknown as Record<string, unknown>),
            ) as never,
            subscribeServerConfig: (() =>
              StreamMod.unwrap(
                EffectMod.gen(function* () {
                  const baseCfg = yield* lease.session.initialConfig;
                  const queue = yield* QueueMod.unbounded<unknown>();
                  const offerSnapshot = () =>
                    QueueMod.offerUnsafe(queue, {
                      version: 1 as const,
                      type: "snapshot" as const,
                      config: buildAugmentedConfig(baseCfg as unknown as Record<string, unknown>),
                    });
                  offerSnapshot();
                  boardEnvState.emitConfigSnapshot = () => {
                    void offerSnapshot();
                  };
                  yield* EffectMod.addFinalizer(() =>
                    EffectMod.sync(() => {
                      boardEnvState.emitConfigSnapshot = null;
                    }),
                  );
                  return StreamMod.fromQueue(queue);
                }),
              )) as never,
          },
        })),
    };

    return { ...actual, layer: Layer.succeed(actual.ConnectionDriver, wrappedDriver) };
  },
);

vi.mock(import("~/rpc/requestLatencyState"), async (original) => ({
  ...(await original()),
  trackRpcRequestSent: () => {},
}));

function makeWorkflowRun(
  fields: Partial<WorkflowRun> & Pick<WorkflowRun, "runId" | "title">,
): WorkflowRun {
  return {
    workflow: "board-execute",
    version: 1,
    status: "running",
    detail: null,
    result: null,
    pauses: 0,
    retired: false,
    resumesAt: null,
    parentRunId: null,
    parentStep: null,
    startedAt: "2026-11-24T00:00:00.000Z",
    updatedAt: "2026-11-24T00:05:00.000Z",
    endedAt: null,
    ...fields,
  };
}

function makeWorkflowStep(
  fields: Partial<WorkflowStep> & Pick<WorkflowStep, "name" | "kind">,
): WorkflowStep {
  return {
    status: "completed",
    attempts: 1,
    detail: null,
    threadId: null,
    environmentId: null,
    result: null,
    said: null,
    startedAt: "2026-11-24T00:00:00.000Z",
    endedAt: "2026-11-24T00:01:00.000Z",
    ...fields,
  };
}

function makeVcsSnapshot(overrides?: {
  readonly local?: Partial<VcsStatusLocalResult>;
  readonly remote?: Partial<VcsStatusRemoteResult> | null;
}) {
  const local: VcsStatusLocalResult = {
    isRepo: true,
    hasPrimaryRemote: true,
    isDefaultRef: false,
    refName: "feat/card-1",
    hasWorkingTreeChanges: true,
    workingTree: {
      rootPath: "/workspace/render",
      files: [
        { path: "kept.ts", insertions: 10, deletions: 2 },
        { path: "ignored.ts", insertions: 3, deletions: 1 },
      ],
      insertions: 13,
      deletions: 3,
    },
    sourceControlProvider: {
      kind: SourceControlProviderKind.make("github"),
      name: "GitHub",
      baseUrl: "https://github.com/org/repo",
    },
    ...overrides?.local,
  };
  const remote: VcsStatusRemoteResult | null =
    overrides?.remote === null
      ? null
      : {
          hasUpstream: true,
          aheadCount: 1,
          behindCount: 0,
          aheadOfDefaultCount: 1,
          pr: null,
          ...overrides?.remote,
        };
  return { _tag: "snapshot" as const, local, remote };
}

function setupBoardAppEnvironment(options: {
  readonly workflowsEnabled: boolean;
  readonly projectWorkflowBoard: boolean;
  readonly threads?: ReadonlyArray<FakeThread>;
}) {
  useCommandPaletteStore.setState(useCommandPaletteStore.getInitialState(), true);
  boardEnvState.workflowsEnabled = options.workflowsEnabled;
  boardEnvState.projectWorkflowBoard = options.projectWorkflowBoard;
  boardEnvState.emitConfigSnapshot?.();
  __setClientSettingsForTests({
    ...DEFAULT_CLIENT_SETTINGS,
    onboardingCompletedAt: "2026-01-01T00:00:00.000Z",
  });
  const execute = threadWithHistory("exec-ship-thread", 1);
  return installFakeEnvironment(
    options.threads ?? [
      execute,
      threadWithHistory("rev-fresh-thread-1", 1),
      threadWithHistory("exec-ship-thread-2", 1),
    ],
  );
}

async function mountBoardAppAt(initialEntry: string) {
  await using stack = new AsyncDisposableStack();
  appAtomRegistry.reset();
  stack.use(await withDesktopViewport(1360, 860));
  const router = getRouter(createMemoryHistory({ initialEntries: [initialEntry] }));
  await router.load();
  const screen = stack.use(
    await render(<AppRoot router={router} />, {
      // AppRoot owns its router instance.
      router: false,
    }),
  );
  const owned = stack.move();
  return {
    ...screen,
    router,
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}

function answerSourceControlDiscovery() {
  return answerRpc(WS_METHODS.serverDiscoverSourceControl, () =>
    Effect.succeed({
      versionControlSystems: [],
      sourceControlProviders: [
        {
          kind: SourceControlProviderKind.make("github"),
          label: "GitHub",
          status: "available",
          version: Option.none(),
          installHint: "Install gh",
          detail: Option.none(),
          auth: {
            status: "authenticated",
            account: Option.none(),
            host: Option.none(),
            detail: Option.none(),
          },
        },
      ],
      sources: [],
    }),
  );
}

function installVcsThreadHandlers(options?: {
  readonly local?: Partial<VcsStatusLocalResult>;
  readonly remote?: Partial<VcsStatusRemoteResult> | null;
}) {
  const stackedActionCalls: Array<Record<string, unknown>> = [];
  const publishCalls: Array<unknown> = [];
  const snap = makeVcsSnapshot(options);
  const statusTarget = {
    environmentId: ENVIRONMENT_ID,
    input: { cwd: "/workspace/render" },
  };
  const disposables = [
    answerRpc(WS_METHODS.subscribeVcsStatus, () => Stream.concat(Stream.make(snap), Stream.never)),
    answerRpc(WS_METHODS.vcsRefreshStatus, () =>
      Effect.succeed({
        ...snap.local,
        ...(snap.remote ?? { hasUpstream: false, aheadCount: 0, behindCount: 0, pr: null }),
      }),
    ),
    answerRpc(WS_METHODS.gitRunStackedAction, (input: Record<string, unknown>) => {
      stackedActionCalls.push(input);
      const action = String(input.action ?? "commit");
      const cwd = String(input.cwd ?? "/workspace/render");
      const actionId = String(input.actionId ?? "act-1");
      return Stream.make(
        { actionId, cwd, action, kind: "action_started", phases: [] },
        {
          actionId,
          cwd,
          action,
          kind: "action_finished",
          result: {
            action,
            branch: { status: "skipped_not_requested", name: "feat/card-1" },
            commit:
              action === "commit"
                ? { status: "created", commitSha: "abc1234", subject: "commit" }
                : { status: "skipped_not_requested" },
            push:
              action === "push"
                ? { status: "pushed", remote: "origin" }
                : { status: "skipped_not_requested" },
            pr:
              action === "create_pr"
                ? { status: "created", url: "https://github.com/org/repo/pull/42" }
                : { status: "skipped_not_requested" },
            toast: { title: "Git action complete", cta: { kind: "none" } },
          },
        },
      );
    }),
    answerSourceControlDiscovery(),
    answerRpc(WS_METHODS.sourceControlPublishRepository, (input) => {
      publishCalls.push(input);
      return Effect.never;
    }),
  ];
  appAtomRegistry.refresh(vcsEnvironment.status(statusTarget));
  return {
    stackedActionCalls,
    publishCalls,
    [Symbol.dispose]() {
      for (let i = disposables.length - 1; i >= 0; i -= 1) {
        disposables[i]?.[Symbol.dispose]();
      }
      appAtomRegistry.refresh(vcsEnvironment.status(statusTarget));
    },
  };
}

interface ExecuteTaskSeed {
  readonly id: string;
  readonly title: string;
  readonly dependsOn: ReadonlyArray<string>;
  ready: boolean;
  status: "pending" | "running" | "completed" | "blocked";
  readonly filesAllowed: ReadonlyArray<string>;
  readonly filesForbidden: ReadonlyArray<string>;
}

interface CheckReceiptSeed {
  readonly checkId: string;
  readonly criterionId: string;
  readonly command: ReadonlyArray<string>;
  status: "pass" | "fail" | "pending";
  readonly exitCode?: number;
  readonly snapshotDigest: string;
  readonly logPath: string;
  stale: boolean;
}

interface ReviewFindingSeed {
  readonly id: string;
  readonly summary: string;
  resolved: boolean;
  disposition?: string;
  rationale?: string;
}

interface ExecuteShipCardSeed {
  readonly cardId: string;
  readonly title: string;
  stage: "spec" | "plan" | "execute" | "ship";
  status: "active" | "paused" | "blocked" | "completed";
  revision: number;
  threadId: string | null;
  readonly tasks?: Array<ExecuteTaskSeed>;
  readonly blockers?: Array<{
    readonly kind: WorkflowBoardShipBlocker["kind"];
    readonly message: string;
    readonly targetTab?: string;
  }>;
  readonly unexpectedFiles?: ReadonlyArray<string>;
  readonly filesForbiddenViolations?: ReadonlyArray<string>;
  readonly checkReceipts?: Array<CheckReceiptSeed>;
  reviewReceipts?: Array<{
    readonly threadId: string;
    readonly verdict: "PASS" | "FAIL" | "MALFORMED";
    readonly snapshotDigest: string;
    stale: boolean;
  }>;
  readonly reviewFindings?: Array<ReviewFindingSeed>;
  readonly manualAcceptances?: Array<{
    readonly criterionId: string;
    status: "pass" | "fail" | "not-applicable";
    observation: string;
  }>;
  externalEdits?: ReadonlyArray<{ readonly path: string }>;
  readonly stageBrief?: string;
  readonly attempts?: ReadonlyArray<{
    readonly attempt: number;
    readonly provider: string;
    readonly model: string;
    readonly threadId: string;
  }>;
  amendment?: WorkflowBoardPlanAmendmentRecord;
  planApprovalStale?: boolean;
  deliveryReceipt?: { readonly snapshotDigest: string; readonly approvedAt: string } | null;
}

function makeCard(
  overrides: Partial<ExecuteShipCardSeed> & Pick<ExecuteShipCardSeed, "cardId" | "title">,
): ExecuteShipCardSeed {
  return {
    stage: "execute",
    status: "active",
    revision: 5,
    threadId: "exec-ship-thread",
    tasks: [
      {
        id: "TSK-1",
        title: "Core implementation",
        dependsOn: [],
        ready: true,
        status: "pending",
        filesAllowed: ["src/ok.ts"],
        filesForbidden: ["src/secret.ts"],
      },
      {
        id: "TSK-2",
        title: "Follow-up task",
        dependsOn: ["TSK-1"],
        ready: false,
        status: "pending",
        filesAllowed: ["src/follow-up.ts"],
        filesForbidden: ["src/secret.ts"],
      },
    ],
    blockers: [],
    unexpectedFiles: [],
    filesForbiddenViolations: [],
    checkReceipts: [
      {
        checkId: "CHK-1",
        criterionId: "AC-1",
        command: ["node", "-e", "process.exit(0)"],
        status: "pass",
        snapshotDigest: "sha256:snap-1",
        logPath: "/host/logs/chk-1.log",
        stale: false,
      },
    ],
    reviewReceipts: [],
    reviewFindings: [],
    manualAcceptances: [],
    externalEdits: [],
    stageBrief: "Stage brief: AC-1 automated check + AC-2 manual verification",
    attempts: [
      {
        attempt: 1,
        provider: "devReplay",
        model: "dev-replay",
        threadId: "exec-ship-thread",
      },
    ],
    deliveryReceipt: null,
    ...overrides,
  };
}

function createStatefulExecuteShipEnv(
  seedCards: ReadonlyArray<ExecuteShipCardSeed>,
  threads?: ReadonlyArray<FakeThread>,
) {
  const envDisposable = setupBoardAppEnvironment({
    workflowsEnabled: true,
    projectWorkflowBoard: true,
    ...(threads ? { threads } : {}),
  });
  const cards = new Map<string, ExecuteShipCardSeed>(seedCards.map((c) => [c.cardId, c]));
  const stackedActionCalls: Array<Record<string, unknown>> = [];

  const toDetail = (card: ExecuteShipCardSeed): WorkflowCardDetail => {
    const revision = PositiveInt.make(card.revision);
    const detail = makeBoardCardDetail({ cardId: card.cardId, title: card.title });
    const threadId = card.threadId ? ThreadId.make(card.threadId) : null;
    const modelSelection = {
      instanceId: ProviderInstanceId.make("devReplay"),
      model: "dev-replay",
    };
    const workspaceStrategy = {
      type: "existing_worktree",
      worktreePath: "/workspace/render",
    } as const;
    const specDigest = "sha256:spec-1";
    const planDigest = "sha256:plan-1";
    const receipt = (gate: "spec" | "plan", artifactDigest: string) => ({
      id: WorkflowBoardApprovalId.make(`app-${gate}-1`),
      gate,
      status: gate === "plan" && card.planApprovalStale ? ("stale" as const) : ("valid" as const),
      revision: PositiveInt.make(1),
      artifactDigest,
      specDigest,
      wireframesDigest: null,
      approvedAt: BOARD_AT,
      approvedBy: "board-e2e-operator",
    });
    const deliveryReceipt: WorkflowBoardDeliveryReceipt | null = card.deliveryReceipt
      ? {
          id: "delivery-1",
          approvalId: null,
          cardRevision: revision,
          productSnapshotDigest: card.deliveryReceipt.snapshotDigest,
          commitSha: null,
          specDigest,
          planDigest,
          tasksDigest: null,
          checksDigest: null,
          reviewId: null,
          acVerdictsSummary: "AC-1 pass and AC-2 recorded",
          approvedBy: "board-e2e-operator",
          approvedAt: IsoDateTime.make(card.deliveryReceipt.approvedAt),
          status: "shipped",
        }
      : null;
    const seedBlockers = [...(card.blockers ?? [])];
    if (card.reviewFindings?.some((finding) => !finding.resolved)) {
      seedBlockers.push({
        kind: "review_unresolved",
        message: "Resolve outstanding review findings.",
        targetTab: "review",
      });
    }
    const pendingObservation = !card.manualAcceptances?.some(
      (observation) => observation.criterionId === "AC-2" && observation.status === "pass",
    );
    if (pendingObservation) {
      seedBlockers.push({
        kind: "ac_unverified",
        message: "AC-2 needs a passing manual observation.",
        targetTab: "acceptance",
      });
    }
    const blockers: WorkflowBoardShipBlocker[] = seedBlockers.map((blocker) => ({
      id: blocker.kind,
      kind: blocker.kind,
      message: blocker.message,
      pointer: {
        stage: blocker.kind === "ac_unverified" ? "ship" : "execute",
        tab: blocker.kind === "check_failed" ? "checks" : (blocker.targetTab ?? "execute"),
        targetId: blocker.kind === "check_failed" ? "CHK-1" : null,
      },
      blocking: true,
    }));
    const checkReceipts = (card.checkReceipts ?? []).map((check) => ({
      checkId: check.checkId,
      criterionId: check.criterionId,
      status: check.stale
        ? ("stale" as const)
        : check.status === "pass"
          ? ("passed" as const)
          : check.status === "fail"
            ? ("failed" as const)
            : ("pending" as const),
      argv: [check.command[0]!, ...check.command.slice(1)] as const,
      exitCode: check.exitCode ?? null,
      logRef: check.logPath,
      productSnapshotDigest: check.snapshotDigest,
      executedAt: BOARD_AT,
      updatedAt: BOARD_AT,
    }));
    const findings = (card.reviewFindings ?? []).map((finding) => ({
      id: finding.id,
      severity: "major" as const,
      status: finding.resolved ? ("resolved" as const) : ("open" as const),
      criterionId: null,
      file: null,
      line: null,
      message: finding.summary,
      disposition:
        finding.disposition === "fix" ||
        finding.disposition === "accept_risk" ||
        finding.disposition === "dismiss" ||
        finding.disposition === "defer"
          ? finding.disposition
          : ("unresolved" as const),
      resolutionNote: finding.rationale ?? null,
      resolvedBy: finding.resolved ? "board-e2e-operator" : null,
      resolvedAt: finding.resolved ? BOARD_AT : null,
    }));
    const review = card.reviewReceipts?.at(-1);
    const unexpectedFiles = (card.unexpectedFiles ?? []).map((path) =>
      WorkflowBoardArtifactKey.make(path),
    );
    const forbiddenFiles = (card.filesForbiddenViolations ?? []).map((path) =>
      WorkflowBoardArtifactKey.make(path),
    );
    const externalEdits = (card.externalEdits ?? []).map(({ path }) => ({
      id: path,
      relativePath: WorkflowBoardArtifactKey.make(path),
      expectedDigest: "sha256:old",
      actualDigest: "sha256:new",
      status: "detected" as const,
      affectedGates: ["plan" as const],
      detectedAt: BOARD_AT,
      reconciledAt: null,
      summary: `External edit detected for ${path}`,
    }));
    const acMappings = [
      { criterionId: "AC-1", method: "automated", checkIds: ["CHK-1"] },
      { criterionId: "AC-2", method: "manual", procedure: "Verify behavior manually." },
    ] as const;
    const checks = checkReceipts.map((check) => ({
      id: check.checkId,
      criterionId: check.criterionId,
      argv: check.argv,
    }));
    const tasks: WorkflowBoardPlanDocument["tasks"] = (card.tasks ?? []).map((task) => ({
      taskId: task.id,
      title: task.title,
      dependsOn: task.dependsOn,
      filesAllowed: [
        WorkflowBoardArtifactKey.make(task.filesAllowed[0]!),
        ...task.filesAllowed.slice(1).map((path) => WorkflowBoardArtifactKey.make(path)),
      ],
      filesForbidden: task.filesForbidden.map((path) => WorkflowBoardArtifactKey.make(path)),
      testingMethod: "existing-checks",
    }));
    const plan: WorkflowBoardPlanDocument = {
      lenses: (
        ["ux", "functional", "architecture", "quality", "infrastructure", "delivery"] as const
      ).map((lens) => ({
        lens,
        applicable: true,
        summary: `Covered ${lens} constraints.`,
      })),
      alternatives: [
        {
          id: "primary",
          title: "Local stage runner",
          summary: "Checkpoint each stage.",
          tradeoffs: "Smaller state footprint",
        },
        {
          id: "secondary",
          title: "Remote pipeline",
          summary: "Dispatch over queue.",
          tradeoffs: "More infrastructure",
        },
      ],
      alternativesRevisionCount: NonNegativeInt.make(0),
      selectedAlternativeId: "primary",
      selectionRationale: "Keep workflow state next to the worktree.",
      strategy: "outcomes",
      strategyRationale: null,
      strategyMetadata: null,
      granularity: "balanced",
      tasks,
      checks,
      acMappings,
      planDigest,
      tasksDigest: "sha256:tasks-1",
      updatedAt: BOARD_AT,
    };
    return {
      ...detail,
      workspaceStrategy,
      modelSelection,
      stage: card.stage,
      status: card.status,
      revision,
      currentTask: card.tasks?.find((task) => task.status === "running")?.id ?? null,
      blockerSummary: blockers[0]?.message ?? null,
      nextAction: card.stage === "ship" ? "ready_for_ship" : "start_execute",
      activeThreadId: threadId,
      spec: {
        ...detail.spec,
        brief: card.stageBrief ?? "",
        specDigest,
        acceptanceCriteria: [
          { id: "AC-1", text: "Automated check passes" },
          { id: "AC-2", text: "Manual verification recorded" },
        ],
      },
      specApproval: receipt("spec", specDigest),
      specApprovedRevision: PositiveInt.make(1),
      specApprovalStatus: "valid",
      planApproval: receipt("plan", planDigest),
      planApprovedRevision: PositiveInt.make(1),
      planApprovalStatus: card.planApprovalStale ? "stale" : "valid",
      plan,
      tasks,
      amendments: card.amendment ? [card.amendment] : [],
      revisions: (card.attempts ?? []).map((attempt) => ({
        revision: PositiveInt.make(attempt.attempt),
        stage: "execute",
        kind: "stage_started",
        summary: `Attempt ${attempt.attempt}: ${attempt.provider} / ${attempt.model}`,
        specDigest,
        createdAt: BOARD_AT,
      })),
      taskExecutions: (card.tasks ?? []).map((task, index) => ({
        taskId: task.id,
        status: task.status === "running" ? "in_progress" : task.status,
        threadId:
          task.status === "running"
            ? threadId
            : task.status === "completed" && card.attempts?.[index]
              ? ThreadId.make(card.attempts[index].threadId)
              : null,
        runId: null,
        stageRunId: null,
        worktreePath: null,
        branch: null,
        attempt: PositiveInt.make(card.attempts?.[index]?.attempt ?? 1),
        startedAt: null,
        endedAt: null,
        lastError: null,
      })),
      stageRuns: (card.attempts ?? []).map((attempt) => ({
        id: WorkflowBoardStageRunId.make(`stage-${attempt.attempt}-${card.cardId}`),
        stage: "execute",
        attempt: PositiveInt.make(attempt.attempt),
        stableLaunchKey: `launch-${attempt.attempt}-${card.cardId}`,
        status: "completed",
        cardRevision: revision,
        threadId: ThreadId.make(attempt.threadId),
        runId: null,
        projectId: PROJECT_ID,
        workspaceStrategy,
        worktreePath: "/workspace/render",
        branch: null,
        modelSelection: {
          instanceId: ProviderInstanceId.make(attempt.provider),
          model: attempt.model,
        },
        runtimeMode: "full-access",
        interactionMode: "default",
        lastError: null,
        createdAt: BOARD_AT,
        updatedAt: BOARD_AT,
        endedAt: BOARD_AT,
      })),
      fileContractReports:
        unexpectedFiles.length + forbiddenFiles.length > 0
          ? [
              {
                taskId: card.tasks?.[0]?.id ?? "TSK-1",
                threadId,
                runId: null,
                status: "violation_pending",
                changedFiles: [...unexpectedFiles, ...forbiddenFiles],
                allowedFiles: [],
                unexpectedFiles,
                forbiddenFiles,
                preservedInWorktree: true,
                blockingReason: "Edits outside task file scope were preserved for review.",
                updatedAt: BOARD_AT,
              },
            ]
          : [],
      checkReceipts,
      checks,
      reviewReport:
        review || findings.length > 0
          ? {
              reviewId: "review-1",
              stageRunId: null,
              threadId: review ? ThreadId.make(review.threadId) : null,
              status: review?.stale
                ? "stale"
                : review?.verdict === "MALFORMED"
                  ? "failed"
                  : "completed",
              verdict: review?.stale
                ? "stale"
                : review?.verdict === "PASS"
                  ? "pass"
                  : review?.verdict === "MALFORMED"
                    ? "failed"
                    : "findings",
              summary: review?.verdict === "MALFORMED" ? "Malformed review output" : null,
              productSnapshotDigest: review?.snapshotDigest ?? "sha256:snap-1",
              findings,
              createdAt: BOARD_AT,
              updatedAt: BOARD_AT,
              completedAt: BOARD_AT,
            }
          : null,
      acMappings,
      acObservations: (card.manualAcceptances ?? []).map((observation) => ({
        criterionId: observation.criterionId,
        method: "manual",
        status: observation.status === "not-applicable" ? "not_applicable" : observation.status,
        checkIds: [],
        procedure: "Verify behavior manually.",
        observationNote: observation.observation,
        evidenceRef: null,
        evidenceDigest: null,
        productSnapshotDigest: "sha256:snap-1",
        observedBy: "board-e2e-operator",
        observedAt: BOARD_AT,
        updatedAt: BOARD_AT,
      })),
      shipReadiness: {
        status: deliveryReceipt ? "shipped" : blockers.length ? "blocked" : "ready",
        ready: blockers.length === 0,
        productSnapshotDigest: "sha256:snap-1",
        blockers,
        deliveryReceipt,
      },
      deliveryReceipt,
      hasUnreconciledEdits: externalEdits.length > 0,
      recoveryState: {
        incompleteJournalCount: NonNegativeInt.make(0),
        pendingJournalEntries: [],
        externalEdits,
        requiresReconciliation: externalEdits.length > 0,
      },
    };
  };

  const stub = installWorkflowBoardFixtures(seedCards.map(toDetail));
  const emitSnapshot = () => {
    for (const card of cards.values()) stub.put(toDetail(card));
  };

  stub.on(
    WS_METHODS.workflowBoardGetCardDiff,
    (input, fixtures): WorkflowBoardGetCardDiffResult => {
      const card = fixtures.get(String(input.cardId));
      const report = card.fileContractReports?.[0];
      return {
        cardId: card.cardId,
        worktreePath: "/workspace/render",
        baseRef: "main",
        productSnapshotDigest: "sha256:snap-1",
        diffDigest: "sha256:diff",
        files: [
          ...(report?.unexpectedFiles ?? []).map((path) => ({
            path,
            contractStatus: "unexpected" as const,
          })),
          ...(report?.forbiddenFiles ?? []).map((path) => ({
            path,
            contractStatus: "forbidden" as const,
          })),
        ].map((file) => ({
          ...file,
          changeType: "change",
          previousPath: null,
          additions: 1,
          deletions: 0,
          patch: null,
        })),
        fileContractReport: report ?? null,
        truncated: false,
        generatedAt: BOARD_AT,
      };
    },
  );

  stub.on(WS_METHODS.workflowBoardStartStage, (raw): WorkflowBoardStartStageResult => {
    const input = raw as WorkflowBoardStartStageInput;
    const { cardId, stage, taskId } = input;
    const card = cardAtRevision(cardId, input.expectedRevision);
    if (taskId && card.tasks?.some((task) => task.status === "running" && task.id !== taskId)) {
      throw boardRequestError("invalid_stage_transition", "Single mutating task active on card");
    }
    card.revision += 1;
    if (taskId) {
      const task = card.tasks?.find((item) => item.id === taskId);
      if (!task) throw boardRequestError("invalid_input", "Unknown task.");
      task.status = "running";
    }
    const threadId =
      stage === "review" ? "rev-fresh-thread-1" : (card.threadId ?? "exec-ship-thread");
    if (stage === "review") {
      card.reviewReceipts = [
        { threadId, verdict: "PASS", snapshotDigest: "sha256:snap-1", stale: false },
      ];
    }
    emitSnapshot();
    return {
      cardId,
      stage,
      revision: PositiveInt.make(card.revision),
      stageRunId: WorkflowBoardStageRunId.make(`sr-${card.cardId}-${card.revision}`),
      runId: RunId.make(`run-${card.cardId}-${card.revision}`),
      threadId: ThreadId.make(threadId),
      resumed: false,
    };
  });

  stub.on("workflowBoard.pauseCard", (input) => {
    const { cardId } = input as { readonly cardId: string };
    const card = cards.get(cardId);
    if (!card) throw boardRequestError("card_not_found", `Card ${cardId} not found`);
    card.status = "paused";
    card.revision += 1;
    emitSnapshot();
    const detail = toDetail(card);
    return {
      cardId: detail.cardId,
      status: detail.status,
      revision: detail.revision,
      card: detail,
    };
  });

  stub.on("workflowBoard.resumeCard", (input) => {
    const { cardId, expectedRevision } = input as {
      readonly cardId: string;
      readonly expectedRevision?: number;
    };
    const card = cards.get(cardId);
    if (!card) throw boardRequestError("card_not_found", `Card ${cardId} not found`);
    if (expectedRevision !== undefined && expectedRevision !== card.revision) {
      throw boardRequestError(
        "revision_conflict",
        "The card changed; reconcile its current revision.",
      );
    }
    card.status = "active";
    card.revision += 1;
    const reconciledCount =
      input.reconcileInterrupted === true ? (card.externalEdits?.length ?? 0) : 0;
    if (reconciledCount > 0) card.externalEdits = [];
    emitSnapshot();
    const detail = toDetail(card);
    return {
      cardId: detail.cardId,
      status: detail.status,
      revision: detail.revision,
      reconciledCount,
      card: detail,
    };
  });

  stub.on(WS_METHODS.workflowBoardProposeAmendment, (raw): WorkflowBoardProposeAmendmentResult => {
    const input = raw as WorkflowBoardProposeAmendmentInput;
    const seed = cardAtRevision(input.cardId, input.expectedRevision);
    seed.revision += 1;
    seed.stage = input.targetStage ?? "plan";
    seed.planApprovalStale = true;
    const amendment: WorkflowBoardPlanAmendmentRecord = {
      id: "amendment-1",
      cardRevision: PositiveInt.make(seed.revision),
      targetStage: seed.stage,
      status: "proposed",
      reason: input.reason,
      summary: input.summary ?? null,
      sourceTaskId: input.sourceTaskId ?? null,
      unexpectedFiles: input.unexpectedFiles ?? [],
      createdAt: BOARD_AT,
    };
    seed.amendment = amendment;
    const card = stub.put(toDetail(seed));
    return {
      cardId: card.cardId,
      revision: card.revision,
      stage: card.stage,
      amendment,
      specApproval: card.specApproval,
      planApproval: card.planApproval ?? null,
      card,
    };
  });

  stub.on(WS_METHODS.workflowBoardApprovePlan, (input): WorkflowBoardApprovePlanResult => {
    const seed = cardAtRevision(String(input.cardId), Number(input.expectedRevision));
    seed.revision += 1;
    seed.stage = "execute";
    seed.planApprovalStale = false;
    if (seed.amendment) seed.amendment = { ...seed.amendment, status: "applied" };
    for (const check of seed.checkReceipts ?? []) check.stale = true;
    for (const review of seed.reviewReceipts ?? []) review.stale = true;
    const card = stub.put(toDetail(seed));
    return { cardId: card.cardId, revision: card.revision, planApproval: card.planApproval!, card };
  });

  const cardAtRevision = (cardId: string, expectedRevision: number) => {
    const card = cards.get(cardId);
    if (!card) throw boardRequestError("card_not_found", `Card ${cardId} not found`);
    if (card.revision !== expectedRevision)
      throw boardRequestError("revision_conflict", "The card revision changed.");
    return card;
  };

  stub.on(WS_METHODS.workflowBoardRetryStep, (raw): WorkflowBoardRetryStepResult => {
    const input = raw as WorkflowBoardRetryStepInput;
    const seed = cardAtRevision(input.cardId, input.expectedRevision);
    if (input.stepKind === "review") {
      seed.reviewReceipts?.splice(0);
    } else if (input.stepKind === "check") {
      const receipt = seed.checkReceipts?.find((item) => item.checkId === input.targetId);
      if (!receipt) throw boardRequestError("invalid_input", "Unknown planned check.");
      receipt.stale = false;
      receipt.status = "pending";
    } else if (input.stepKind === "task") {
      const task = seed.tasks?.find((item) => item.id === input.targetId);
      if (!task) throw boardRequestError("invalid_input", "Unknown task.");
      task.status = "pending";
    } else {
      for (const task of seed.tasks ?? []) task.status = "pending";
    }
    seed.revision += 1;
    const card = stub.put(toDetail(seed));
    return {
      cardId: card.cardId,
      revision: card.revision,
      targetId: input.targetId ?? null,
      stepKind: input.stepKind,
      card,
    };
  });

  stub.on(WS_METHODS.workflowBoardResolveFinding, (raw): WorkflowBoardResolveFindingResult => {
    const input = raw as WorkflowBoardResolveFindingInput;
    const seed = cardAtRevision(input.cardId, input.expectedRevision);
    const finding = seed.reviewFindings?.find((f) => f.id === input.findingId);
    if (!finding) throw boardRequestError("invalid_input", `Finding ${input.findingId} not found`);
    finding.resolved = true;
    finding.disposition = input.disposition;
    if (input.reason !== undefined) finding.rationale = input.reason;
    seed.revision += 1;
    const card = stub.put(toDetail(seed));
    const reviewReport = card.reviewReport!;
    return {
      cardId: card.cardId,
      revision: card.revision,
      finding: reviewReport.findings.find((f) => f.id === input.findingId)!,
      reviewReport,
      card,
    };
  });

  stub.on(
    WS_METHODS.workflowBoardRecordManualAcceptance,
    (raw): WorkflowBoardRecordManualAcceptanceResult => {
      const input = raw as WorkflowBoardRecordManualAcceptanceInput;
      const seed = cardAtRevision(input.cardId, input.expectedRevision);
      const existing = seed.manualAcceptances?.find(
        (item) => item.criterionId === input.criterionId,
      );
      if (existing) {
        existing.status = input.verdict;
        existing.observation = input.note ?? "";
      } else {
        seed.manualAcceptances?.push({
          criterionId: input.criterionId,
          status: input.verdict,
          observation: input.note ?? "",
        });
      }
      seed.revision += 1;
      const card = stub.put(toDetail(seed));
      return {
        cardId: card.cardId,
        revision: card.revision,
        observation: card.acObservations!.find((item) => item.criterionId === input.criterionId)!,
        shipReadiness: card.shipReadiness!,
        card,
      };
    },
  );

  stub.on(WS_METHODS.workflowBoardApproveDelivery, (input): WorkflowBoardApproveDeliveryResult => {
    const seed = cardAtRevision(String(input.cardId), Number(input.expectedRevision));
    if (!toDetail(seed).shipReadiness?.ready) {
      throw boardRequestError("invalid_stage_transition", "Resolve all Ship blockers first.");
    }
    seed.stage = "ship";
    seed.status = "completed";
    seed.revision += 1;
    seed.deliveryReceipt = {
      snapshotDigest: "sha256:snap-1",
      approvedAt: BOARD_AT,
    };
    const card = stub.put(toDetail(seed));
    return {
      cardId: card.cardId,
      status: card.status,
      revision: card.revision,
      deliveryStatus: "shipped",
      deliveryReceipt: card.deliveryReceipt!,
      deliveryApproval: {
        id: WorkflowBoardApprovalId.make("app-delivery"),
        gate: "delivery",
        status: "valid",
        revision: card.revision,
        artifactDigest: "sha256:snap-1",
        specDigest: card.spec.specDigest,
        wireframesDigest: null,
        approvedAt: BOARD_AT,
        approvedBy: "board-e2e-operator",
      },
      card,
    };
  });

  const gitActions = answerRpc(WS_METHODS.gitRunStackedAction, (input: Record<string, unknown>) => {
    stackedActionCalls.push(input);
    return Stream.empty;
  });

  return {
    stub,
    cards,
    stackedActionCalls,
    [Symbol.dispose]() {
      gitActions[Symbol.dispose]();
      stub[Symbol.dispose]();
      envDisposable[Symbol.dispose]();
    },
  };
}

async function openBoardAndSelectCard(
  stub: WorkflowBoardFixtureStore,
  options: { readonly cardTitle: string; readonly stageTab?: "Execute" | "Ship" },
) {
  const screen = await mountBoardAppAt("/");
  await using stack = new AsyncDisposableStack();
  stack.use(screen);

  openCommandPalette();
  const paletteInput = browserPage.getByPlaceholder("Search commands, projects, and threads...");
  await expect.element(paletteInput).toBeVisible();
  await userEvent.fill(paletteInput, "workflow board");
  const option = browserPage.getByRole("option", { name: /workflow board/i });
  await expect.element(option).toBeVisible();
  await userEvent.click(option);

  for (const column of ["Spec", "Plan", "Execute", "Ship"]) {
    await expect
      .element(browserPage.getByRole("region", { name: column, exact: true }))
      .toBeVisible();
  }

  await browserPage
    .getByRole("region", { name: /^(Spec|Plan|Execute|Ship)$/ })
    .getByText(options.cardTitle)
    .click();
  await vi.waitFor(() => {
    expect(stub.calls.some((c) => c.method === "workflowBoard.getCard")).toBe(true);
  });

  if (options.stageTab) {
    const tab = browserPage
      .getByRole("tablist", { name: "Workflow board stages" })
      .getByRole("tab", { name: new RegExp(`^${options.stageTab}`, "i") });
    await userEvent.click(tab);
    await expect.element(tab).toHaveAttribute("aria-selected", "true");
  }

  const owned = stack.move();
  return {
    ...screen,
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}

describe("workflowBoardE2eExecuteShip (F13–F21)", () => {
  it("[F13-T1-1] Tier 1: preserves explicit environment scope without fallback and gates board Execute launch", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    const legacy = makeBoardServerConfigFixture({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    expect(hasProjectWorkflowBoardCapability(legacy)).toBe(false);

    await using _screen = await mountBoardAppAt("/workflows?env=env-board-other&run=run-other");
    await expect.element(browserPage.getByText(/env-board-other is not connected/)).toBeVisible();
  });

  it("[F13-T2-1] Tier 2 (Board-UI-Gated): missing worktree or unavailable provider surfaces launch error without silent fallback", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({ cardId: "card-f13-err", title: "Missing Worktree Card", stage: "execute" }),
    ]);
    env.stub.on("workflowBoard.startStage", () => {
      throw boardRequestError("launch_failed", "Existing worktree path does not exist");
    });
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Missing Worktree Card",
      stageTab: "Execute",
    });
    await browserPage
      .getByRole("button", { name: /start execute|start/i })
      .first()
      .click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.startStage")).toBe(true);
    });
    await expect
      .element(browserPage.getByText("Existing worktree path does not exist", { exact: true }))
      .toBeVisible();
    expect(env.stub.callsTo(WS_METHODS.workflowBoardStartStage)).toHaveLength(1);
  });

  it("[F14-T1-1] Tier 1 (Board-UI-Gated): Execute tab renders dependency-ready tasks and enforces single mutating task", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({ cardId: "card-f14-dag", title: "DAG Execute Card", stage: "execute" }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "DAG Execute Card",
      stageTab: "Execute",
    });
    await expect.element(browserPage.getByRole("tab", { name: "Execute" })).toBeVisible();
    await expect.element(browserPage.getByText("TSK-1")).toBeVisible();
    await expect.element(browserPage.getByText("TSK-2")).toBeVisible();
    await browserPage
      .getByRole("button", { name: /start.*TSK-1|start execute|start/i })
      .first()
      .click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.startStage")).toBe(true);
    });
  });

  it("[F14-T1-2] Tier 1 (Board-UI-Gated): post-turn contract diff surfaces unexpected edits as Preserved for review", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f14-diff",
        title: "Unexpected Diff Card",
        stage: "execute",
        unexpectedFiles: ["src/unexpected.ts"],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Unexpected Diff Card",
      stageTab: "Execute",
    });
    const preserved = browserPage.getByRole("region", { name: /Unexpected edits preserved/ });
    await expect.element(preserved).toBeVisible();
    await expect
      .element(preserved.getByText("src/unexpected.ts", { exact: true }).first())
      .toBeVisible();
    await expect.element(preserved.getByText(/preserved intact in the worktree/i)).toBeVisible();
  });

  it("[F14-T2-1] Tier 2 (Board-UI-Gated): starting second mutating task on same card surfaces conflict notice", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f14-cap",
        title: "Single Mutating Cap Card",
        stage: "execute",
        tasks: [
          {
            id: "TSK-1",
            title: "Active Task",
            dependsOn: [],
            ready: true,
            status: "running",
            filesAllowed: ["src/a.ts"],
            filesForbidden: [],
          },
          {
            id: "TSK-2",
            title: "Second Ready Task",
            dependsOn: [],
            ready: true,
            status: "pending",
            filesAllowed: ["src/b.ts"],
            filesForbidden: [],
          },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Single Mutating Cap Card",
      stageTab: "Execute",
    });
    const secondTask = browserPage.getByRole("listitem").filter({ hasText: "Second Ready Task" });
    await expect.element(secondTask.getByRole("button", { name: "Start task" })).toBeDisabled();
    await expect
      .element(
        secondTask.getByText("Only one mutating task can run at a time (TSK-1 is active).", {
          exact: true,
        }),
      )
      .toBeVisible();
    expect(env.stub.callsTo(WS_METHODS.workflowBoardStartStage)).toHaveLength(0);
  });

  it("[F14-T2-2] Tier 2 (Board-UI-Gated): forbidden-file touch surfaces contract violation banner naming path", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f14-forb",
        title: "Forbidden File Card",
        stage: "execute",
        status: "blocked",
        filesForbiddenViolations: ["src/secret.ts"],
        blockers: [
          {
            kind: "contract_violation",
            message: "filesForbidden touched: src/secret.ts",
            targetTab: "execute",
          },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Forbidden File Card",
      stageTab: "Execute",
    });
    const preserved = browserPage.getByRole("region", { name: /Unexpected edits preserved/ });
    await expect
      .element(preserved.getByText("src/secret.ts", { exact: true }).first())
      .toBeVisible();
    await expect.element(preserved.getByText("Forbidden", { exact: true }).first()).toBeVisible();
  });

  it("[F15-T1-1] Tier 1 (Board-UI-Gated): Execute controls expose Pause, Resume, Retry, and Propose amendment", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({ cardId: "card-f15-ctrl", title: "Execute Controls Card", stage: "execute" }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Execute Controls Card",
      stageTab: "Execute",
    });
    const pauseBtn = browserPage.getByRole("button", { name: "Pause card" });
    await expect.element(pauseBtn).toBeVisible();
    await expect.element(browserPage.getByRole("button", { name: "Retry stage" })).toBeEnabled();
    await expect
      .element(browserPage.getByRole("button", { name: "Propose plan amendment", exact: true }))
      .toBeVisible();
    await pauseBtn.click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.pauseCard")).toBe(true);
    });
    await expect.element(browserPage.getByRole("button", { name: "Resume card" })).toBeVisible();
  });

  it("[F15-T1-2] Tier 1: paused workflow state surfaces Why it paused, Transcript, and Resume action", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    const pausedRun = makeWorkflowRun({
      runId: "sync-exec",
      title: "Execute stage",
      status: "paused",
      detail: "apply: 503",
      pauses: 1,
    });
    const resumeCalls: Array<unknown> = [];
    using _subWf = answerRpc(WS_METHODS.workflowsSubscribe, () =>
      Stream.make({ workflows: [], runs: [pausedRun], questions: [] }),
    );
    using _cntWf = answerRpc(WS_METHODS.workflowsCounts, () =>
      Stream.make({ needsYou: 1, active: 0, total: 1 }),
    );
    using _getWf = answerRpc(WS_METHODS.workflowsGet, () =>
      Effect.succeed({
        run: pausedRun,
        steps: [
          makeWorkflowStep({
            name: "apply",
            kind: "agent",
            status: "failed",
            detail: "503",
            threadId: "exec-ship-thread",
            environmentId: ENVIRONMENT_ID,
          }),
        ],
        references: [],
      }),
    );
    using _resWf = answerRpc(WS_METHODS.workflowsResume, (input) => {
      resumeCalls.push(input);
      return Effect.undefined;
    });

    await using _screen = await mountBoardAppAt("/workflows?run=sync-exec");
    const why = browserPage.getByRole("region", { name: "Why it paused" });
    await expect.element(why, { timeout: 3000 }).toMatchTextContent(/Paused at apply.*503/s);
    const step = browserPage.getByRole("region", { name: "apply", exact: true });
    const transcript = step.getByRole("button", { name: "Transcript", exact: true });
    await expect.element(transcript, { timeout: 3000 }).toBeVisible();
    await transcript.click();
    const pane = browserPage.getByRole("complementary", { name: "Beside the run" });
    await expect.element(pane, { timeout: 3000 }).toBeVisible();
    await expect
      .element(pane.getByText("exec-ship-thread: question 0"), { timeout: 3000 })
      .toBeVisible();
    const resume = why.getByRole("button", { name: "Resume", exact: true });
    await expect.element(resume, { timeout: 3000 }).toBeEnabled();
    await resume.click();
    await vi.waitFor(() => expect(resumeCalls).toEqual([{ runId: "sync-exec" }]));
  });

  it("[F15-T2-1] Tier 2 (Board-UI-Gated): approving plan amendment transitions downstream evidence badges to Stale", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({ cardId: "card-f15-amend", title: "Amendment Stale Card", stage: "execute" }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Amendment Stale Card",
      stageTab: "Execute",
    });
    await browserPage.getByRole("button", { name: "Propose plan amendment", exact: true }).click();
    await browserPage
      .getByLabelText("Reason", { exact: true })
      .fill("Widen filesAllowed to src/extra.ts");
    await browserPage.getByRole("button", { name: "Submit amendment" }).click();
    await vi.waitFor(() => {
      expect(env.stub.callsTo(WS_METHODS.workflowBoardProposeAmendment)).toMatchObject([
        {
          reason: "Widen filesAllowed to src/extra.ts",
          targetStage: "plan",
        },
      ]);
    });
    await browserPage.getByRole("tab", { name: "Plan", exact: true }).click();
    await browserPage.getByRole("button", { name: "Approve plan", exact: true }).click();
    await browserPage.getByRole("tab", { name: "Execute", exact: true }).click();
    const checks = browserPage.getByRole("region", { name: "Planned checks & receipts" });
    await expect
      .element(
        checks.getByText(
          "Snapshot changed after this check ran. Re-run check to refresh receipt.",
          { exact: true },
        ),
      )
      .toBeVisible();
  });

  it("[F15-T2-2] Tier 2 (Board-UI-Gated): stale expectedRevision conflict surfaces reconciliation prompt", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f15-rev",
        title: "Stale Revision Conflict Card",
        stage: "execute",
        status: "paused",
      }),
    ]);
    env.stub.on("workflowBoard.resumeCard", () => {
      throw boardRequestError("revision_conflict", "Revision conflict: reconcile card revision");
    });
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Stale Revision Conflict Card",
      stageTab: "Execute",
    });
    await browserPage.getByRole("button", { name: "Resume card" }).click();
    await expect
      .element(
        browserPage.getByText("This card changed on the server. Refresh and try again.", {
          exact: true,
        }),
      )
      .toBeVisible();
  });

  it("[F16-T1-1] Tier 1 (Board-UI-Gated): Execute lists planned argv checks with snapshot digest and log reference", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({ cardId: "card-f16-chk", title: "Planned Checks Card", stage: "execute" }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Planned Checks Card",
      stageTab: "Execute",
    });
    const checks = browserPage.getByRole("region", { name: "Planned checks & receipts" });
    await expect.element(checks).toBeVisible();
    await expect.element(checks.getByText("CHK-1", { exact: true })).toBeVisible();
    await expect.element(checks.getByText(/node .*process.exit\(0\)/)).toBeVisible();
    await expect.element(checks.getByText(/sha256:snap-1.*\/host\/logs\/chk-1.log/)).toBeVisible();
  });

  it("[F16-T2-1] Tier 2 (Board-UI-Gated): failed check blocks card and snapshot change stales prior check receipt", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f16-stale",
        title: "Failed & Stale Check Card",
        stage: "execute",
        status: "blocked",
        blockers: [
          { kind: "check_failed", message: "CHK-1 failed (exit 1)", targetTab: "execute" },
        ],
        checkReceipts: [
          {
            checkId: "CHK-1",
            criterionId: "AC-1",
            command: ["node", "-e", "process.exit(1)"],
            status: "fail",
            exitCode: 1,
            snapshotDigest: "sha256:snap-old",
            logPath: "/host/logs/chk-1.log",
            stale: true,
          },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Failed & Stale Check Card",
      stageTab: "Execute",
    });
    const checks = browserPage.getByRole("region", { name: "Planned checks & receipts" });
    await expect.element(checks.getByText("CHK-1", { exact: true })).toBeVisible();
    await expect.element(checks.getByText("exit 1", { exact: true })).toBeVisible();
    await expect
      .element(
        checks.getByText(
          "Snapshot changed after this check ran. Re-run check to refresh receipt.",
          { exact: true },
        ),
      )
      .toBeVisible();
    await browserPage.getByRole("tab", { name: "Ship", exact: true }).click();
    await expect
      .element(browserPage.getByRole("listitem").filter({ hasText: "CHK-1 failed (exit 1)" }))
      .toBeVisible();
    await expect
      .element(browserPage.getByRole("button", { name: "Approve delivery", exact: true }))
      .toBeDisabled();
  });

  it("[F17-T1-1] Tier 1 (Board-UI-Gated): independent review runs in fresh thread and displays PASS vs findings", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f17-pass",
        title: "Fresh Review PASS Card",
        stage: "execute",
        tasks: (makeCard({ cardId: "review-ready", title: "Review ready" }).tasks ?? []).map(
          (task) => ({ ...task, status: "completed" }),
        ),
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Fresh Review PASS Card",
      stageTab: "Execute",
    });
    const review = browserPage.getByRole("region", { name: "Independent review" });
    await review.getByRole("button", { name: "Start independent review" }).click();
    await expect.element(review.getByText("Pass", { exact: true })).toBeVisible();
    await expect.element(review.getByText("No findings reported.")).toBeVisible();
    await expect.element(browserPage.getByText("rev-fresh-thread-1: question 0")).toBeVisible();
    expect(env.stub.callsTo(WS_METHODS.workflowBoardStartStage)).toMatchObject([
      { stage: "review" },
    ]);
  });

  it("[F17-T1-2] Tier 1 (Board-UI-Gated): operator records per-finding dispositions via authenticated client action", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f17-disp",
        title: "Finding Disposition Card",
        stage: "execute",
        reviewFindings: [
          { id: "FND-1", summary: "Handle edge case in diff parser", resolved: false },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Finding Disposition Card",
      stageTab: "Execute",
    });
    const review = browserPage.getByRole("region", { name: "Independent review" });
    await review.getByRole("combobox", { name: "Disposition for FND-1" }).click();
    await browserPage.getByRole("option", { name: "Accept risk", exact: true }).click();
    await review.getByLabelText("Resolution reason for FND-1").fill("Accepted risk for M3");
    await review.getByRole("button", { name: "Resolve finding" }).click();
    await vi.waitFor(() => {
      expect(env.stub.callsTo(WS_METHODS.workflowBoardResolveFinding)).toMatchObject([
        {
          findingId: "FND-1",
          disposition: "accept_risk",
          reason: "Accepted risk for M3",
        },
      ]);
    });
    await expect
      .element(review.getByText("Resolution note: Accepted risk for M3", { exact: true }))
      .toBeVisible();
  });

  it("[F17-T2-1] Tier 2 (Board-UI-Gated): unresolved findings or stale review disable Approve delivery", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f17-blk",
        title: "Unresolved Finding Ship Block Card",
        stage: "ship",
        reviewFindings: [{ id: "FND-1", summary: "Unresolved security check", resolved: false }],
        manualAcceptances: [{ criterionId: "AC-2", status: "pass", observation: "OK" }],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Unresolved Finding Ship Block Card",
      stageTab: "Ship",
    });
    await expect
      .element(browserPage.getByRole("button", { name: "Approve delivery" }))
      .toBeDisabled();
  });

  it("[F17-T2-2] Tier 2 (Board-UI-Gated): malformed review output stays unverified with Retry review button", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f17-mal",
        title: "Malformed Review Card",
        stage: "execute",
        reviewReceipts: [
          {
            threadId: "rev-malformed-1",
            verdict: "MALFORMED",
            snapshotDigest: "sha256:snap-1",
            stale: false,
          },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Malformed Review Card",
      stageTab: "Execute",
    });
    const retryReview = browserPage.getByRole("button", { name: "Retry review" });
    await expect.element(retryReview).toBeVisible();
    await retryReview.click();
    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) => c.method === WS_METHODS.workflowBoardRetryStep && c.input.stepKind === "review",
        ),
      ).toBe(true);
    });
  });

  it("[F18-T1-1] Tier 1 (Board-UI-Gated): Ship tab records per-AC observations and produces delivery receipt", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f18-ship",
        title: "Ship Observations Card",
        stage: "ship",
        reviewReceipts: [
          { threadId: "rev-1", verdict: "PASS", snapshotDigest: "sha256:snap-1", stale: false },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Ship Observations Card",
      stageTab: "Ship",
    });
    await expect.element(browserPage.getByRole("tab", { name: "Ship", exact: true })).toBeVisible();
    const approve = browserPage.getByRole("button", { name: "Approve delivery", exact: true });
    await expect.element(approve).toBeDisabled();
    await browserPage.getByRole("combobox", { name: "Verdict for AC-2" }).click();
    await browserPage.getByRole("option", { name: "Pass", exact: true }).click();
    await browserPage.getByLabelText("Observation note for AC-2").fill("Verified manually");
    const criterion = browserPage.elementLocator(
      document.querySelector('[data-ac-criterion-id="AC-2"]')!,
    );
    await criterion.getByRole("button", { name: "Record observation" }).click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.recordManualAcceptance")).toBe(
        true,
      );
    });
    await expect.element(approve).toBeEnabled();
    await approve.click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveDelivery")).toBe(true);
    });
    expect(env.cards.get("card-f18-ship")?.stage).toBe("ship");
    expect(env.cards.get("card-f18-ship")?.status).toBe("completed");
    await expect.element(browserPage.getByText(/shipped|completed/i).first()).toBeVisible();
  });

  it("[F18-T1-2] Tier 1 (Board-UI-Gated): Ship gate surfaces blockers with deep-link Fix actions", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f18-fix",
        title: "Ship Blocker Fix Link Card",
        stage: "ship",
        status: "blocked",
        blockers: [{ kind: "check_failed", message: "CHK-1 failed", targetTab: "execute" }],
        checkReceipts: [
          {
            checkId: "CHK-1",
            criterionId: "AC-1",
            command: ["node", "-e", "process.exit(1)"],
            status: "fail",
            exitCode: 1,
            snapshotDigest: "sha256:snap-1",
            logPath: "/host/logs/chk-1.log",
            stale: false,
          },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Ship Blocker Fix Link Card",
      stageTab: "Ship",
    });
    const blocker = browserPage.getByRole("listitem").filter({ hasText: "CHK-1 failed" });
    const fixBtn = blocker.getByRole("button", { name: "Go to fix" });
    await expect.element(fixBtn).toBeVisible();
    await fixBtn.click();
    await expect
      .element(browserPage.getByRole("tab", { name: "Execute", exact: true }))
      .toHaveAttribute("aria-selected", "true");
    await expect
      .element(browserPage.getByRole("region", { name: "Planned checks & receipts" }))
      .toBeVisible();
  });

  it("[F18-T2-1] Tier 2 (Board-UI-Gated): one failing or missing AC blocks Approve delivery without bypass", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f18-failac",
        title: "Failing AC Block Card",
        stage: "ship",
        reviewReceipts: [
          { threadId: "rev-1", verdict: "PASS", snapshotDigest: "sha256:snap-1", stale: false },
        ],
        manualAcceptances: [{ criterionId: "AC-2", status: "fail", observation: "Broken" }],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Failing AC Block Card",
      stageTab: "Ship",
    });
    await expect
      .element(browserPage.getByRole("button", { name: "Approve delivery" }))
      .toBeDisabled();
    expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveDelivery")).toBe(false);
  });

  it("[F18-T2-2] Tier 2 (Board-UI-Gated): Approve delivery moves card to shipped without auto-committing", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f18-nocommit",
        title: "Ship No Auto Commit Card",
        stage: "ship",
        reviewReceipts: [
          { threadId: "rev-1", verdict: "PASS", snapshotDigest: "sha256:snap-1", stale: false },
        ],
        manualAcceptances: [{ criterionId: "AC-2", status: "pass", observation: "Verified" }],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Ship No Auto Commit Card",
      stageTab: "Ship",
    });
    const approveBtn = browserPage.getByRole("button", { name: "Approve delivery" });
    await expect.element(approveBtn).toBeEnabled();
    await approveBtn.click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveDelivery")).toBe(true);
    });
    expect(env.cards.get("card-f18-nocommit")?.stage).toBe("ship");
    expect(env.cards.get("card-f18-nocommit")?.status).toBe("completed");
    await expect.element(browserPage.getByText(/shipped|completed/i).first()).toBeVisible();
    expect(env.stackedActionCalls).toHaveLength(0);
  });

  it("[F19-T1-1] Tier 1: GitActionsControl exposes Commit/Push/Create PR individually and commits selected filePaths only", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: false });
    using vcs = installVcsThreadHandlers({
      local: {
        hasWorkingTreeChanges: true,
        isDefaultRef: false,
      },
      remote: {
        hasUpstream: true,
        aheadCount: 1,
        behindCount: 0,
        aheadOfDefaultCount: 1,
        pr: null,
      },
    });
    await using _screen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/exec-ship-thread`);

    await browserPage.getByRole("button", { name: "Git action options" }).click();
    await expect.element(browserPage.getByRole("menuitem", { name: "Commit" })).toBeVisible();
    await expect.element(browserPage.getByRole("menuitem", { name: "Push" })).toBeVisible();
    await expect.element(browserPage.getByRole("menuitem", { name: "Create PR" })).toBeVisible();

    await browserPage.getByRole("menuitem", { name: "Commit" }).click();
    await browserPage.getByRole("button", { name: "Edit" }).click();
    await browserPage.getByRole("checkbox", { name: "Include ignored.ts" }).click();
    await userEvent.fill(
      browserPage.getByPlaceholder("Leave empty to auto-generate"),
      "feat: ship card change",
    );
    await browserPage.getByRole("button", { name: "Commit", exact: true }).click();

    await vi.waitFor(() =>
      expect(
        vcs.stackedActionCalls,
        browserPage.getByRole("region", { name: "Notifications" }).query()?.textContent ?? "",
      ).toHaveLength(1),
    );
    expect(vcs.stackedActionCalls[0]).toMatchObject({
      action: "commit",
      commitMessage: "feat: ship card change",
      filePaths: ["kept.ts"],
    });
  });

  it("[F19-T1-2] Tier 1: selecting Push or Create PR individually dispatches only push or create_pr", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: false });
    using vcs = installVcsThreadHandlers({
      local: {
        hasWorkingTreeChanges: false,
        isDefaultRef: false,
        workingTree: {
          rootPath: "/workspace/render",
          files: [],
          insertions: 0,
          deletions: 0,
        },
      },
      remote: {
        hasUpstream: true,
        aheadCount: 2,
        behindCount: 0,
        aheadOfDefaultCount: 2,
        pr: null,
      },
    });
    await using _screen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/exec-ship-thread`);

    await browserPage.getByRole("button", { name: "Git action options" }).click();
    await browserPage.getByRole("menuitem", { name: "Push" }).click();
    await vi.waitFor(() =>
      expect(
        vcs.stackedActionCalls,
        browserPage.getByRole("region", { name: "Notifications" }).query()?.textContent ?? "",
      ).toHaveLength(1),
    );
    expect(vcs.stackedActionCalls[0]).toMatchObject({ action: "push" });

    await browserPage.getByRole("button", { name: "Git action options" }).click();
    await browserPage.getByRole("menuitem", { name: "Create PR" }).click();
    await vi.waitFor(() => expect(vcs.stackedActionCalls).toHaveLength(2));
    expect(vcs.stackedActionCalls[1]).toMatchObject({ action: "create_pr" });
  });

  it("[F19-T2-1] Tier 2: default-branch Push prompts confirmation (Abort cancels) and excluding all files disables Commit", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: false });
    using vcs = installVcsThreadHandlers({
      local: {
        refName: "main",
        isDefaultRef: true,
        hasWorkingTreeChanges: true,
      },
      remote: {
        hasUpstream: true,
        aheadCount: 1,
        behindCount: 0,
        aheadOfDefaultCount: 1,
        pr: null,
      },
    });
    await using _screen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/exec-ship-thread`);

    await browserPage.getByRole("button", { name: "Git action options" }).click();
    await browserPage.getByRole("menuitem", { name: "Push" }).click();
    await expect.element(browserPage.getByText("Push to default ref?")).toBeVisible();
    await browserPage.getByRole("button", { name: "Abort" }).click();
    expect(vcs.stackedActionCalls).toHaveLength(0);

    await browserPage.getByRole("button", { name: "Git action options" }).click();
    await browserPage.getByRole("menuitem", { name: "Push" }).click();
    await browserPage.getByRole("button", { name: "Push to main" }).click();
    await vi.waitFor(() =>
      expect(
        vcs.stackedActionCalls,
        browserPage.getByRole("region", { name: "Notifications" }).query()?.textContent ?? "",
      ).toHaveLength(1),
    );
    expect(vcs.stackedActionCalls[0]).toMatchObject({ action: "push" });
    expect(vcs.stackedActionCalls[0]?.featureBranch).toBeUndefined();

    await browserPage.getByRole("button", { name: "Git action options" }).click();
    await browserPage.getByRole("menuitem", { name: "Commit" }).click();
    await browserPage.getByRole("button", { name: "Edit" }).click();
    await browserPage.getByRole("checkbox", { name: "Include all files" }).click();
    await expect
      .element(browserPage.getByRole("button", { name: "Commit", exact: true }))
      .toBeDisabled();
    await expect
      .element(browserPage.getByRole("button", { name: "Commit on new branch" }))
      .toBeDisabled();
  });

  it("[F19-T2-2] Tier 2: missing write scope disables Git actions and PublishRepositoryDialog guards IME composition Enter", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      const baseFetch = globalThis.fetch;
      globalThis.fetch = async (input, init) => {
        const url = new URL(
          input instanceof Request ? input.url : String(input),
          window.location.href,
        );
        if (url.pathname === "/api/auth/session") {
          const readonlyScopes = AuthStandardClientScopes.filter(
            (s) => s !== "source-control:write",
          );
          return new Response(
            JSON.stringify({
              authenticated: true,
              auth: {
                policy: "loopback-browser",
                bootstrapMethods: ["one-time-token"],
                sessionMethods: ["browser-session-cookie"],
                sessionCookieName: "legioncode_session",
              },
              scopes: readonlyScopes,
              permissions: readonlyScopes,
              sessionMethod: "browser-session-cookie",
              expiresAt: "2099-01-01T00:00:00.000Z",
            }),
            { status: 200, headers: { "content-type": "application/json" } },
          );
        }
        return baseFetch(input, init);
      };
      using _restoreFetch = {
        [Symbol.dispose]() {
          globalThis.fetch = baseFetch;
          appAtomRegistry.refresh(environmentSession.sessionStateAtom(ENVIRONMENT_ID));
        },
      };
      using _vcs = installVcsThreadHandlers({
        local: { hasPrimaryRemote: false },
        remote: { hasUpstream: false, aheadCount: 0, behindCount: 0, pr: null },
      });
      appAtomRegistry.refresh(environmentSession.sessionStateAtom(ENVIRONMENT_ID));
      await using _screen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/exec-ship-thread`);
      await expect
        .element(browserPage.getByRole("button", { name: "Commit" }))
        .toHaveAttribute("aria-disabled", "true");
    }

    using _env2 = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    appAtomRegistry.refresh(environmentSession.sessionStateAtom(ENVIRONMENT_ID));
    using vcs2 = installVcsThreadHandlers({
      local: {
        hasPrimaryRemote: false,
        hasWorkingTreeChanges: false,
        workingTree: {
          rootPath: "/workspace/render",
          files: [],
          insertions: 0,
          deletions: 0,
        },
      },
      remote: { hasUpstream: false, aheadCount: 0, behindCount: 0, pr: null },
    });
    await using _screen2 = await mountBoardAppAt(`/${ENVIRONMENT_ID}/exec-ship-thread`);

    await browserPage.getByRole("button", { name: "Publish repository" }).click();
    await browserPage.getByRole("button", { name: "Next" }).click();
    const repoInput = browserPage.getByRole("textbox", { name: "Repository" });
    await repoInput.fill("octo/card-repo");
    await act(() => {
      repoInput.element().dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          isComposing: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    expect(vcs2.publishCalls).toHaveLength(0);
    await userEvent.keyboard("{Enter}");
    await vi.waitFor(() =>
      expect(
        vcs2.publishCalls,
        browserPage.getByRole("dialog").query()?.textContent ?? "",
      ).toHaveLength(1),
    );
  });

  it("[F20-T1-1] Tier 1: WorkflowsPage run inspector renders cleanly when Labs is on/off and board capability is false", async () => {
    {
      using _envOff = setupBoardAppEnvironment({
        workflowsEnabled: false,
        projectWorkflowBoard: false,
      });
      await using _screenOff = await mountBoardAppAt("/workflows");
      await expect.element(browserPage.getByText("Workflows are off")).toBeVisible();
    }

    using _envOn = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    using _subWf = answerRpc(WS_METHODS.workflowsSubscribe, () =>
      Stream.make({ workflows: [], runs: [], questions: [] }),
    );
    using _cntWf = answerRpc(WS_METHODS.workflowsCounts, () =>
      Stream.make({ needsYou: 0, active: 0, total: 0 }),
    );
    await using _screenOn = await mountBoardAppAt("/workflows");
    await expect.element(browserPage.getByRole("heading", { name: "Workflows" })).toBeVisible();
    await expect
      .element(browserPage.getByRole("button", { name: "Definitions", exact: true }).first())
      .toBeVisible();
  });

  it("[F20-T2-1] Tier 2 (Board-UI-Gated): external workspace edits surface reconcile banner before Execute or Ship", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f20-ext",
        title: "External Disk Edit Card",
        stage: "execute",
        externalEdits: [{ path: ".legion/workflows/card-f20-ext/spec.md" }],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "External Disk Edit Card",
      stageTab: "Execute",
    });
    const recovery = browserPage.getByRole("region", { name: "Reconciliation required" });
    await expect.element(recovery).toBeVisible();
    await expect
      .element(recovery.getByText(".legion/workflows/card-f20-ext/spec.md", { exact: true }))
      .toBeVisible();
    await expect
      .element(recovery.getByRole("button", { name: "Pause to reconcile" }))
      .toBeEnabled();
  });

  it("[F21-T1-1] Tier 1: stage view renders assembled brief and active provider/model", async () => {
    const brief = "Stage brief: AC-1 + AC-2 + sourceRefs";
    const thread = threadWithHistory("exec-ship-thread", 1);
    using env = createStatefulExecuteShipEnv(
      [makeCard({ cardId: "card-f21-brief", title: "Stage Brief Card", stageBrief: brief })],
      [
        {
          ...thread,
          items: thread.items.map((item) =>
            item.type === "user_message" ? { ...item, text: brief } : item,
          ),
        },
      ],
    );
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Stage Brief Card",
      stageTab: "Execute",
    });
    const stageThread = browserPage.getByRole("region", { name: "Connected stage thread" });
    await expect.element(stageThread.getByText(brief, { exact: true })).toBeVisible();
    await expect
      .element(stageThread.getByText("Connected Execute thread", { exact: true }))
      .toBeVisible();
    await browserPage.getByRole("tab", { name: /^History/ }).click();
    await expect
      .element(browserPage.getByText("Attempt 1: devReplay / dev-replay", { exact: true }))
      .toBeVisible();
  });

  it("[F21-T2-1] Tier 2 (Board-UI-Gated): switching provider between attempts preserves card history and prior transcripts", async () => {
    using env = createStatefulExecuteShipEnv([
      makeCard({
        cardId: "card-f21-switch",
        title: "Provider Switch Attempts Card",
        stage: "execute",
        tasks: (makeCard({ cardId: "attempts", title: "Attempts" }).tasks ?? []).map((task) => ({
          ...task,
          status: "completed",
        })),
        attempts: [
          {
            attempt: 1,
            provider: "devReplay",
            model: "dev-replay",
            threadId: "exec-ship-thread",
          },
          {
            attempt: 2,
            provider: "claudeAgent",
            model: "claude-opus-4-6",
            threadId: "exec-ship-thread-2",
          },
        ],
      }),
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Provider Switch Attempts Card",
      stageTab: "Execute",
    });
    await browserPage.getByRole("tab", { name: /^History/ }).click();
    const ledger = browserPage.getByRole("region", { name: "Revision ledger" });
    await expect
      .element(ledger.getByText("Attempt 1: devReplay / dev-replay", { exact: true }))
      .toBeVisible();
    await expect
      .element(ledger.getByText("Attempt 2: claudeAgent / claude-opus-4-6", { exact: true }))
      .toBeVisible();
    await browserPage.getByRole("tab", { name: "Execute", exact: true }).click();
    const pane = browserPage.getByRole("region", { name: "Connected stage thread" });
    for (const [title, transcript] of [
      ["Core implementation", "exec-ship-thread: question 0"],
      ["Follow-up task", "exec-ship-thread-2: question 0"],
    ] as const) {
      const task = browserPage.getByRole("listitem").filter({ hasText: title });
      await task.getByRole("button", { name: "Open thread" }).click();
      await expect.element(pane.getByText(transcript, { exact: true })).toBeVisible();
    }
  });
});
