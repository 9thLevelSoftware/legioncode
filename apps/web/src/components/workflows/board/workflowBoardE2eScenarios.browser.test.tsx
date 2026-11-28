import {
  AuthStandardClientScopes,
  IsoDateTime,
  NonNegativeInt,
  WorkflowBoardNonGitArtifactKey,
  PositiveInt,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  EnvironmentId,
  ProviderInstanceId,
  SourceControlProviderKind,
  WorkflowBoardRequestError,
  WS_METHODS,
  type WorkflowBoardApplyCliImportResult,
  type WorkflowBoardPreviewCliImportResult,
  type VcsStatusLocalResult,
  type VcsStatusRemoteResult,
  type WorkflowBoardAcceptanceCriterion,
  type WorkflowBoardApprovalStatus,
  type WorkflowBoardAssistanceMode,
  type WorkflowBoardCardStatus,
  type WorkflowBoardCardSummary,
  type WorkflowBoardSpecSourceType,
  type WorkflowBoardStage,
  type WorkflowBoardStageRunLink,
  type WorkflowCardDetail,
  type WorkflowCardId,
  type WorkflowRun,
  type WorkflowStep,
} from "@legioncode/contracts";
import { DEFAULT_CLIENT_SETTINGS } from "@legioncode/contracts/settings";
import { createMemoryHistory } from "@tanstack/react-router";
import type * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as Option from "effect/Option";
import * as Stream from "effect/Stream";
import { describe, expect, it, vi } from "vite-plus/test";
import { page, userEvent } from "vite-plus/test/browser";

import { AppRoot } from "~/AppRoot";
import { openCommandPalette } from "~/commandPaletteBus";
import { useCommandPaletteStore } from "~/commandPaletteStore";
import "~/components/ChatView";
import { HtmlRenderDocument } from "~/components/files/BrowserDocumentFrame";
import { __setClientSettingsForTests } from "~/hooks/useSettings";
import { startI18n, whenLocaleReady } from "~/i18n/runtime";
import "~/index.css";
import { getRouter, type AppRouter } from "~/router";
import { appAtomRegistry } from "~/rpc/atomRegistry";
import { serverEnvironment } from "~/state/server";
import { environmentSession } from "~/state/session";
import { vcsEnvironment } from "~/state/vcs";
import {
  CODEX_PROVIDER,
  ENVIRONMENT_ID,
  PROJECT_ID,
  answerRpc,
  installFakeEnvironment,
  threadWithHistory,
  type FakeDriver,
} from "../../../../test/fakeEnvironment";
import { act, render } from "../../../../test/render";
import {
  dispatchPointerDrag,
  hasProjectWorkflowBoardCapability,
  makeBoardServerConfigFixture,
  withDesktopViewport,
} from "./workflowBoardE2eHarness";

import {
  BOARD_AT,
  boardRequestError,
  installWorkflowBoardFixtures,
  makeBoardCardDetail,
} from "./workflowBoardE2eFixtures";

const envId = EnvironmentId.make("env-m5-browser");

const boardEnvState = vi.hoisted(() => {
  const emitters = new Set<() => void>();
  return {
    workflowsEnabled: false,
    projectWorkflowBoard: false,
    emitters,
    emitConfigSnapshot: () => {
      for (const emit of emitters) {
        emit();
      }
    },
  };
});

const wireframeAssetState = vi.hoisted(() => ({
  tag: "Success" as "Success" | "Failure",
  url: "about:blank",
  refreshUrl: "about:blank" as string | null,
}));

vi.mock(import("~/assets/longLivedAssetUrls"), () => ({
  useLongLivedAssetUrl: () =>
    wireframeAssetState.tag === "Failure"
      ? { _tag: "Failure" as const, cause: new Error("Asset unavailable") }
      : {
          _tag: "Success" as const,
          url: wireframeAssetState.url,
          expiresAt: 4_102_444_800_000,
        },
}));

vi.mock(import("~/assets/assetUrls"), async (importOriginal) => ({
  ...(await importOriginal()),
  useAssetUrlRefresh: () => async () => {
    if (wireframeAssetState.refreshUrl === null) {
      throw new Error("Refresh failed");
    }
    return wireframeAssetState.refreshUrl;
  },
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
                  const emit = () => {
                    void offerSnapshot();
                  };
                  boardEnvState.emitters.add(emit);
                  yield* EffectMod.addFinalizer(() =>
                    EffectMod.sync(() => {
                      boardEnvState.emitters.delete(emit);
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

const DEV_REPLAY_SERVER_PROVIDER = {
  instanceId: ProviderInstanceId.make("devReplay"),
  driver: "devReplay",
  enabled: true,
  installed: true,
  version: "1.0.0",
  status: "ready",
  auth: { status: "authenticated" },
  checkedAt: "2026-03-17T19:12:28.000Z",
  models: [{ slug: "dev-replay", name: "Replay (dev)", isCustom: false, capabilities: null }],
};

function makeBlobPage(html: string): { readonly url: string } & Disposable {
  const url = URL.createObjectURL(new Blob([html], { type: "text/html" }));
  return {
    url,
    [Symbol.dispose]: () => URL.revokeObjectURL(url),
  };
}

function setupBoardAppEnvironment(options: {
  readonly workflowsEnabled: boolean;
  readonly projectWorkflowBoard: boolean;
  readonly language?: string;
  readonly providers?: ReadonlyArray<unknown>;
  readonly boringSidebarEnabled?: boolean;
}) {
  useCommandPaletteStore.setState(useCommandPaletteStore.getInitialState(), true);
  boardEnvState.workflowsEnabled = options.workflowsEnabled;
  boardEnvState.projectWorkflowBoard = options.projectWorkflowBoard;
  boardEnvState.emitConfigSnapshot();
  __setClientSettingsForTests({
    ...DEFAULT_CLIENT_SETTINGS,
    onboardingCompletedAt: "2026-01-01T00:00:00.000Z",
    ...(options.language ? { language: options.language } : {}),
    ...(options.boringSidebarEnabled !== undefined
      ? { boringSidebarEnabled: options.boringSidebarEnabled }
      : {}),
  });
  return installFakeEnvironment([threadWithHistory("board-m5-thread", 1)], {
    providers: options.providers ?? [DEV_REPLAY_SERVER_PROVIDER, CODEX_PROVIDER],
  });
}

async function mountBoardAppAt(
  initialEntry: string,
  viewport: { readonly width: number; readonly height: number } = { width: 1280, height: 800 },
) {
  await using stack = new AsyncDisposableStack();
  appAtomRegistry.reset();
  stack.use(await withDesktopViewport(viewport.width, viewport.height));
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

function answerWorkflowRunsEmpty() {
  const sub = answerRpc(WS_METHODS.workflowsSubscribe, () =>
    Stream.make({ workflows: [], runs: [], questions: [] }),
  );
  const cnt = answerRpc(WS_METHODS.workflowsCounts, () =>
    Stream.make({ needsYou: 0, active: 0, total: 0 }),
  );
  appAtomRegistry.refresh(
    serverEnvironment.workflowsLive({ environmentId: ENVIRONMENT_ID, input: {} }),
  );
  return {
    [Symbol.dispose]() {
      cnt[Symbol.dispose]();
      sub[Symbol.dispose]();
    },
  };
}

interface M5CardState {
  cardId: WorkflowCardId;
  projectId: typeof PROJECT_ID;
  title: string;
  stage: WorkflowBoardStage;
  status: WorkflowBoardCardStatus;
  revision: number;
  currentTask: string | null;
  blockerSummary: string | null;
  nextAction: string | null;
  specApprovedRevision: number | null;
  specApprovalStatus: WorkflowBoardApprovalStatus | null;
  planApprovalStatus: WorkflowBoardApprovalStatus | null;
  activeThreadId: string | null;
  sourceType: WorkflowBoardSpecSourceType;
  assistanceMode: WorkflowBoardAssistanceMode;
  brief: string;
  specDigest: string | null;
  acceptanceCriteria: Array<WorkflowBoardAcceptanceCriterion>;
  constraints: Array<string>;
  sourceRefs: Array<string>;
  wireframesIndex: string | null;
  workspaceStrategy: { readonly type: "existing_worktree"; readonly worktreePath: string };
  modelSelection: { readonly instanceId: string; readonly model: string } | null;
  runtimeMode: "approval-required" | "auto-accept-edits" | "full-access" | null;
  interactionMode: "default" | "plan" | null;
  stageRuns: Array<WorkflowBoardStageRunLink>;
  artifactKeys: Array<string>;
  unexpectedFiles: Array<string>;
  checkStale: boolean;
  reviewStale: boolean;
  unresolvedFindings: number;
  unsupportedPolicies: Array<string>;
}

function makeM5CardState(
  overrides: Partial<M5CardState> & Pick<M5CardState, "cardId" | "title">,
): M5CardState {
  const wireframesIndex = overrides.wireframesIndex ?? null;
  return {
    projectId: PROJECT_ID,
    stage: "spec",
    status: "active",
    revision: 1,
    currentTask: null,
    blockerSummary: null,
    nextAction: "draft_spec",
    specApprovedRevision: null,
    specApprovalStatus: null,
    planApprovalStatus: null,
    activeThreadId: null,
    sourceType: "idea",
    assistanceMode: "balanced",
    brief: "Deliver workflow board feature cleanly.",
    specDigest: "sha256:spec-digest-1",
    acceptanceCriteria: [{ id: "AC-1", text: "Behavior passes end-to-end." }],
    constraints: ["Never auto-commit deliverables"],
    sourceRefs: ["README.md"],
    wireframesIndex,
    workspaceStrategy: { type: "existing_worktree", worktreePath: "/workspace/render" },
    modelSelection: { instanceId: "devReplay", model: "dev-replay" },
    runtimeMode: "approval-required",
    interactionMode: "default",
    stageRuns: [],
    artifactKeys: wireframesIndex ? ["spec.md", wireframesIndex] : ["spec.md"],
    unexpectedFiles: [],
    checkStale: false,
    reviewStale: false,
    unresolvedFindings: 0,
    unsupportedPolicies: [],
    ...overrides,
  };
}

function toM12Summary(card: M5CardState): WorkflowBoardCardSummary {
  return {
    cardId: card.cardId,
    projectId: card.projectId,
    title: card.title,
    stage: card.stage,
    status: card.status,
    revision: PositiveInt.make(card.revision),
    currentTask: card.currentTask,
    blockerSummary: card.blockerSummary,
    nextAction: card.nextAction,
    specApprovedRevision:
      card.specApprovedRevision === null ? null : PositiveInt.make(card.specApprovedRevision),
    specApprovalStatus: card.specApprovalStatus,
    activeThreadId: card.activeThreadId === null ? null : ThreadId.make(card.activeThreadId),
    createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
    updatedAt: IsoDateTime.make(
      new Date(Date.parse("2026-11-24T00:00:00.000Z") + card.revision * 1000).toISOString(),
    ),
  };
}

function toM12Detail(card: M5CardState): WorkflowCardDetail & Record<string, unknown> {
  const digest = card.specDigest ?? "sha256:spec-digest-1";
  return {
    ...toM12Summary(card),
    workspaceStrategy: card.workspaceStrategy,
    modelSelection: card.modelSelection as WorkflowCardDetail["modelSelection"],
    runtimeMode: card.runtimeMode,
    interactionMode: card.interactionMode,
    spec: {
      title: card.title,
      brief: card.brief,
      sourceType: card.sourceType,
      assistanceMode: card.assistanceMode,
      acceptanceCriteria: card.acceptanceCriteria,
      constraints: card.constraints,
      sourceRefs: card.sourceRefs,
      wireframesIndex: card.wireframesIndex
        ? WorkflowBoardArtifactKey.make(card.wireframesIndex)
        : null,
      specDigest: card.specDigest,
      updatedAt: "2026-11-24T00:05:00.000Z",
    },
    specApproval:
      card.specApprovalStatus === null
        ? null
        : {
            id: WorkflowBoardApprovalId.make("appr-spec-1"),
            gate: "spec",
            status: card.specApprovalStatus,
            revision: card.specApprovedRevision ?? card.revision,
            artifactDigest: digest,
            specDigest: digest,
            wireframesDigest: card.wireframesIndex ? "sha256:wf-1" : null,
            approvedAt: "2026-11-24T00:01:00.000Z",
            approvedBy: "client",
          },
    planApprovalStatus: card.planApprovalStatus,
    unexpectedFiles: card.unexpectedFiles,
    checkStale: card.checkStale,
    reviewStale: card.reviewStale,
    unresolvedFindings: card.unresolvedFindings,
    unsupportedPolicies: card.unsupportedPolicies,
    stageRuns: card.stageRuns,
    revisions: [
      {
        revision: card.revision,
        stage: card.stage,
        kind: "update",
        summary: "Updated card",
        specDigest: card.specDigest,
        createdAt: "2026-11-24T00:05:00.000Z",
      },
    ],
    artifactKeys: card.artifactKeys.map((key) => WorkflowBoardArtifactKey.make(key)),
  };
}

function installStatefulM5BoardRpc(
  initialCards: ReadonlyArray<M5CardState>,
  options?: { readonly initialSnapshotRevision?: number },
) {
  const cards = new Map<string, M5CardState>(
    initialCards.map((c) => [
      c.cardId,
      {
        ...c,
        acceptanceCriteria: [...c.acceptanceCriteria],
        constraints: [...c.constraints],
        sourceRefs: [...c.sourceRefs],
        stageRuns: [...c.stageRuns],
        artifactKeys: [...c.artifactKeys],
      },
    ]),
  );
  const startStageResults: Array<{
    readonly cardId: string;
    readonly stage: WorkflowBoardStage;
    readonly threadId: string;
    readonly resumed: boolean;
  }> = [];
  const stub = installWorkflowBoardFixtures([...cards.values()].map(toM12Detail), options);
  const emitSnapshot = () => {
    for (const card of cards.values()) {
      for (const key of card.artifactKeys) {
        const label = key === "plan-brief.md" ? "Plan Brief" : "Spec";
        stub.artifacts.set(`${card.cardId}:${key}`, `# ${label}: ${card.title}\n\n${card.brief}`);
      }
      stub.put(toM12Detail(card));
    }
  };
  emitSnapshot();

  stub.on("workflowBoard.createCard", (input) => {
    const {
      projectId,
      title,
      brief,
      sourceType,
      assistanceMode,
      workspaceStrategy,
      modelSelection,
      runtimeMode,
      interactionMode,
    } = input as {
      readonly projectId?: typeof PROJECT_ID;
      readonly title: string;
      readonly brief?: string;
      readonly sourceType?: WorkflowBoardSpecSourceType;
      readonly assistanceMode?: WorkflowBoardAssistanceMode;
      readonly workspaceStrategy?: M5CardState["workspaceStrategy"];
      readonly modelSelection?: M5CardState["modelSelection"];
      readonly runtimeMode?: M5CardState["runtimeMode"];
      readonly interactionMode?: M5CardState["interactionMode"];
    };
    const cardId = `card-${cards.size + 1}` as WorkflowCardId;
    const created = makeM5CardState({
      cardId,
      projectId: projectId ?? PROJECT_ID,
      title,
      brief: brief ?? "",
      sourceType: sourceType ?? "idea",
      assistanceMode: assistanceMode ?? "balanced",
      specDigest: "sha256:spec-digest-1",
      stage: "spec",
      status: "active",
      revision: 1,
      artifactKeys: ["spec.md"],
      ...(workspaceStrategy ? { workspaceStrategy } : {}),
      ...(modelSelection !== undefined ? { modelSelection } : {}),
      ...(runtimeMode !== undefined ? { runtimeMode } : {}),
      ...(interactionMode !== undefined ? { interactionMode } : {}),
    });
    cards.set(cardId, created);
    emitSnapshot();
    return toM12Detail(created);
  });

  stub.on("workflowBoard.updateSpec", (input) => {
    const {
      cardId,
      expectedRevision,
      title,
      brief,
      acceptanceCriteria,
      constraints,
      sourceRefs,
      wireframesIndex,
    } = input as {
      readonly cardId: string;
      readonly expectedRevision: number;
      readonly title?: string;
      readonly brief?: string;
      readonly acceptanceCriteria?: ReadonlyArray<WorkflowBoardAcceptanceCriterion>;
      readonly constraints?: ReadonlyArray<string>;
      readonly sourceRefs?: ReadonlyArray<string>;
      readonly wireframesIndex?: string | null;
    };
    const card = cards.get(cardId);
    if (!card) {
      throw new WorkflowBoardRequestError({
        code: "card_not_found",
        message: `Card ${cardId} not found`,
      });
    }
    if (card.revision !== expectedRevision) {
      throw new WorkflowBoardRequestError({
        code: "revision_conflict",
        message: "Revision conflict",
        currentRevision: card.revision as never,
      });
    }
    card.revision += 1;
    if (title !== undefined) card.title = title;
    if (brief !== undefined) card.brief = brief;
    if (acceptanceCriteria !== undefined) card.acceptanceCriteria = [...acceptanceCriteria];
    if (constraints !== undefined) card.constraints = [...constraints];
    if (sourceRefs !== undefined) card.sourceRefs = [...sourceRefs];
    if (wireframesIndex !== undefined) card.wireframesIndex = wireframesIndex;
    card.specDigest = `sha256:spec-digest-${card.revision}`;
    if (card.specApprovalStatus === "valid" || card.planApprovalStatus === "valid") {
      card.specApprovalStatus = "stale";
      card.planApprovalStatus = "stale";
    }
    emitSnapshot();
    return {
      cardId,
      revision: card.revision,
      specDigest: card.specDigest,
      specApprovalStatus: card.specApprovalStatus,
      card: toM12Detail(card),
    };
  });

  stub.on("workflowBoard.approveSpec", (input) => {
    const { cardId, expectedRevision } = input as {
      readonly cardId: string;
      readonly expectedRevision?: number;
    };
    const card = cards.get(cardId);
    if (!card) {
      throw new WorkflowBoardRequestError({
        code: "card_not_found",
        message: `Card ${cardId} not found`,
      });
    }
    if (expectedRevision !== undefined && card.revision !== expectedRevision) {
      throw new WorkflowBoardRequestError({
        code: "revision_conflict",
        message: "Revision conflict",
        currentRevision: card.revision as never,
      });
    }
    card.revision += 1;
    card.specApprovedRevision = card.revision;
    card.specApprovalStatus = "valid";
    card.stage = "plan";
    card.nextAction = "start_plan";
    emitSnapshot();
    const detail = toM12Detail(card);
    return {
      cardId,
      revision: card.revision,
      specApproval: detail.specApproval,
      card: detail,
    };
  });

  stub.on("workflowBoard.startStage", (input) => {
    const {
      cardId,
      expectedRevision,
      stage,
      modelSelection,
      runtimeMode,
      interactionMode,
      workspaceStrategy,
    } = input as {
      readonly cardId: string;
      readonly expectedRevision?: number;
      readonly stage: WorkflowBoardStage;
      readonly modelSelection?: M5CardState["modelSelection"];
      readonly runtimeMode?: M5CardState["runtimeMode"];
      readonly interactionMode?: M5CardState["interactionMode"];
      readonly workspaceStrategy?: M5CardState["workspaceStrategy"];
    };
    const card = cards.get(cardId);
    if (!card) {
      throw new WorkflowBoardRequestError({
        code: "card_not_found",
        message: `Card ${cardId} not found`,
      });
    }
    if (stage === "plan" && card.specApprovalStatus !== "valid") {
      throw new WorkflowBoardRequestError({
        code: "spec_not_approved",
        message: "Spec must be approved first",
      });
    }
    const resumed = card.activeThreadId !== null && card.stage === stage;
    if (!resumed) {
      if (expectedRevision !== undefined && card.revision !== expectedRevision) {
        throw new WorkflowBoardRequestError({
          code: "revision_conflict",
          message: "Revision conflict",
          currentRevision: card.revision as never,
        });
      }
      card.revision += 1;
      card.stage = stage;
      card.activeThreadId = card.activeThreadId ?? "board-m5-thread";
      card.nextAction = "view_plan_thread";
      if (!card.artifactKeys.includes("plan-brief.md")) {
        card.artifactKeys.push("plan-brief.md");
      }
      card.stageRuns.push({
        id: "sr-1" as never,
        stage,
        stableLaunchKey: `board-${stage}-${cardId}`,
        status: "running",
        cardRevision: card.revision as never,
        runId: "run-1" as never,
        threadId: card.activeThreadId as never,
        projectId: card.projectId,
        workspaceStrategy: (workspaceStrategy ?? card.workspaceStrategy) as never,
        worktreePath: (workspaceStrategy ?? card.workspaceStrategy).worktreePath,
        branch: null,
        modelSelection: (modelSelection ??
          card.modelSelection ?? { instanceId: "devReplay", model: "dev-replay" }) as never,
        runtimeMode: (runtimeMode ?? card.runtimeMode ?? "approval-required") as never,
        interactionMode: (interactionMode ?? card.interactionMode ?? "default") as never,
        lastError: null,
        createdAt: "2026-11-24T00:05:00.000Z",
        updatedAt: "2026-11-24T00:05:00.000Z",
        endedAt: null,
      });
    }
    emitSnapshot();
    const res = {
      cardId,
      revision: card.revision,
      stage,
      stageRunId: "sr-1",
      runId: "run-1",
      threadId: card.activeThreadId!,
      resumed,
    };
    startStageResults.push({
      cardId,
      stage,
      threadId: res.threadId,
      resumed,
    });
    return res;
  });

  stub.on("workflowBoard.approveDelivery", (input) => {
    const { cardId } = input as { readonly cardId: string };
    const card = cards.get(cardId);
    if (!card) {
      throw new WorkflowBoardRequestError({
        code: "card_not_found",
        message: `Card ${cardId} not found`,
      });
    }
    card.revision += 1;
    card.stage = "ship";
    card.status = "completed";
    emitSnapshot();
    return {
      cardId,
      revision: card.revision,
      deliveryReceipt: { id: "deliv-1", status: "approved", revision: card.revision },
      card: toM12Detail(card),
    };
  });

  return {
    cards,
    stub,
    startStageResults,
    pushExternalUpdate(cardId: string, mutate: (card: M5CardState) => void) {
      const card = cards.get(cardId);
      if (card) {
        mutate(card);
        emitSnapshot();
      }
    },
    [Symbol.dispose]() {
      stub[Symbol.dispose]();
    },
  };
}

async function openWorkflowBoard(
  screen: { readonly router: AppRouter },
  options?: { readonly allowStageTabs?: boolean },
): Promise<string> {
  const previousHref = screen.router.state.location.href;
  openCommandPalette();
  const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
  await expect.element(paletteInput).toBeVisible();
  await userEvent.fill(paletteInput, "Open workflows");
  await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
  await userEvent.fill(paletteInput, "workflow board");
  await vi.waitFor(() => {
    expect(page.getByRole("option", { name: "Open workflows" }).elements()).toHaveLength(0);
  });
  const option = page.getByRole("option", { name: /Open workflow board/i });
  await expect.element(option.first()).toBeVisible();
  await userEvent.click(option.first());

  const hasStage = (stage: string) =>
    page.getByRole("region", { name: stage }).elements().length > 0 ||
    page.getByRole("heading", { name: stage }).elements().length > 0 ||
    (options?.allowStageTabs !== false &&
      page.getByRole("tab", { name: new RegExp(`^${stage}\\b`, "i") }).elements().length > 0);

  const verifyBoardReady = async () => {
    await vi.waitFor(() => {
      expect(useCommandPaletteStore.getState().open).toBe(false);
      expect(screen.router.state.location.href).not.toBe(previousHref);
      expect(screen.router.state.location.href).toContain("/board");
      for (const column of ["Spec", "Plan", "Execute", "Ship"]) {
        expect(hasStage(column)).toBe(true);
      }
    });
  };

  await verifyBoardReady();
  return screen.router.state.location.href;
}

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

function installVcsHandlers(options?: {
  readonly local?: Partial<VcsStatusLocalResult>;
  readonly remote?: Partial<VcsStatusRemoteResult> | null;
}) {
  const stackedCalls: Array<Record<string, unknown>> = [];
  const publishCalls: Array<unknown> = [];
  const local: VcsStatusLocalResult = {
    isRepo: true,
    hasPrimaryRemote: true,
    isDefaultRef: false,
    refName: "feat/card-m5",
    hasWorkingTreeChanges: true,
    workingTree: {
      rootPath: "/workspace/render",
      files: [{ path: "src/card.ts", insertions: 8, deletions: 1 }],
      insertions: 8,
      deletions: 1,
    },
    sourceControlProvider: {
      kind: SourceControlProviderKind.make("github"),
      name: "GitHub",
      baseUrl: "https://github.com/org/repo",
    },
    ...options?.local,
  };
  const remote: VcsStatusRemoteResult | null =
    options?.remote === null
      ? null
      : {
          hasUpstream: true,
          aheadCount: 1,
          behindCount: 0,
          aheadOfDefaultCount: 1,
          pr: null,
          ...options?.remote,
        };
  const statusTarget = {
    environmentId: ENVIRONMENT_ID,
    input: { cwd: "/workspace/render" },
  };
  const disposables = [
    answerRpc(WS_METHODS.subscribeVcsStatus, () =>
      Stream.concat(Stream.make({ _tag: "snapshot" as const, local, remote }), Stream.never),
    ),
    answerRpc(WS_METHODS.vcsRefreshStatus, () =>
      Effect.succeed({
        ...local,
        ...(remote ?? { hasUpstream: false, aheadCount: 0, behindCount: 0, pr: null }),
      }),
    ),
    answerRpc(WS_METHODS.gitRunStackedAction, (input: Record<string, unknown>) => {
      stackedCalls.push(input);
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
            branch: { status: "skipped_not_requested", name: "feat/card-m5" },
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
                ? { status: "created", url: "https://github.com/org/repo/pull/99" }
                : { status: "skipped_not_requested" },
            toast: { title: "Git action complete", cta: { kind: "none" } },
          },
        },
      );
    }),
    answerRpc(WS_METHODS.serverDiscoverSourceControl, () =>
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
    ),
    answerRpc(WS_METHODS.sourceControlPublishRepository, (input) => {
      publishCalls.push(input);
      return Effect.never;
    }),
  ];
  appAtomRegistry.refresh(vcsEnvironment.status(statusTarget));
  return {
    stackedCalls,
    publishCalls,
    [Symbol.dispose]() {
      for (let i = disposables.length - 1; i >= 0; i -= 1) {
        disposables[i]?.[Symbol.dispose]();
      }
      appAtomRegistry.refresh(vcsEnvironment.status(statusTarget));
    },
  };
}

async function withLocaleCleanup(run: () => Promise<void>): Promise<void> {
  startI18n();
  try {
    await run();
  } finally {
    __setClientSettingsForTests({
      ...DEFAULT_CLIENT_SETTINGS,
      language: "system",
      onboardingCompletedAt: "2026-01-01T00:00:00.000Z",
    });
    startI18n();
    await whenLocaleReady();
  }
}

function makeQualifiedPairwiseCard(
  cardId: string,
  title: string,
  stage: WorkflowBoardStage,
): WorkflowCardDetail {
  const card = makeBoardCardDetail({ cardId, title, stage });
  return {
    ...card,
    specApprovalStatus: "valid",
    specApprovedRevision: card.revision,
    specApproval: {
      id: WorkflowBoardApprovalId.make(`spec-${cardId}`),
      gate: "spec",
      status: "valid",
      revision: card.revision,
      artifactDigest: card.spec.specDigest!,
      specDigest: card.spec.specDigest!,
      wireframesDigest: null,
      approvedAt: BOARD_AT,
      approvedBy: "operator",
    },
    plan: {
      lenses: ["ux", "functional", "architecture", "quality", "infrastructure", "delivery"].map(
        (lens) => ({
          lens: lens as
            | "ux"
            | "functional"
            | "architecture"
            | "quality"
            | "infrastructure"
            | "delivery",
          applicable: true,
          summary: `Qualified ${lens} design`,
        }),
      ),
      alternatives: [
        {
          id: "direct",
          title: "Direct",
          summary: "Direct implementation",
          tradeoffs: "Small change",
        },
        {
          id: "layered",
          title: "Layered",
          summary: "Introduce adapter",
          tradeoffs: "More maintenance",
        },
      ],
      alternativesRevisionCount: NonNegativeInt.make(0),
      selectedAlternativeId: "direct",
      selectionRationale: "Smaller implementation",
      strategy: "outcomes",
      strategyRationale: null,
      strategyMetadata: null,
      granularity: "balanced",
      tasks: [
        {
          taskId: "TSK-1",
          title: "Implement feature",
          filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/core.ts")],
          testingMethod: "test-first",
        },
      ],
      checks: [{ id: "CHK-1", argv: ["vp", "test", "run"] }],
      acMappings: [{ criterionId: "AC-01", method: "automated", checkIds: ["CHK-1"] }],
      planDigest: "sha256:pairwise-plan",
      tasksDigest: "sha256:pairwise-tasks",
      updatedAt: BOARD_AT,
    },
  };
}

describe("E2E-M5 Tier 3 web browser pairwise combinations", () => {
  it("[T3-W-01] F1 x F2 x F22: live capability toggle hides board entries without breaking ChatView or /workflows", async () => {
    expect(hasProjectWorkflowBoardCapability(null)).toBe(false);
    const enabledCfg = makeBoardServerConfigFixture({
      environmentId: envId,
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    expect(hasProjectWorkflowBoardCapability(enabledCfg)).toBe(true);

    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: true,
      });
      using _wf = answerWorkflowRunsEmpty();
      await using _chatScreen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/board-m5-thread`);
      await expect.element(page.getByText("board-m5-thread: question 0")).toBeVisible();
    }

    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
      boringSidebarEnabled: true,
    });
    using _wf = answerWorkflowRunsEmpty();
    using _board = installStatefulM5BoardRpc([]);
    await using runsScreen = await mountBoardAppAt("/workflows");
    await expect.element(runsScreen.getByRole("heading", { name: "Workflows" })).toBeVisible();
    expect(page.getByRole("button", { name: "Workflow board for render" }).elements()).toHaveLength(
      0,
    );

    openCommandPalette();
    const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
    await expect.element(paletteInput).toBeVisible();
    await userEvent.fill(paletteInput, "Open workflows");
    await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
    await userEvent.fill(paletteInput, "workflow board");
    await vi.waitFor(() => {
      expect(page.getByRole("option", { name: "Open workflows" }).elements()).toHaveLength(0);
    });
    expect(page.getByRole("option", { name: /Open workflow board/i }).elements()).toHaveLength(0);
    await userEvent.keyboard("{Escape}");
    await vi.waitFor(() => {
      expect(useCommandPaletteStore.getState().open).toBe(false);
    });

    boardEnvState.projectWorkflowBoard = true;
    boardEnvState.emitConfigSnapshot?.();
    await vi.waitFor(() => {
      expect(
        page.getByRole("button", { name: "Workflow board for render" }).elements(),
      ).toHaveLength(1);
    });
    await page.getByRole("button", { name: "render" }).first().hover();
    await expect
      .element(page.getByRole("button", { name: "Workflow board for render" }))
      .toBeVisible();

    await openWorkflowBoard(runsScreen);
    for (const col of ["Spec", "Plan", "Execute", "Ship"]) {
      await expect.element(page.getByRole("region", { name: col })).toBeVisible();
    }
    await page.getByRole("tab", { name: /^Spec\b/i }).click();
    await expect.element(page.getByText("No cards in this stage.")).toBeVisible();
  });

  it("[T3-W-02] F3 x F23: pointer and keyboard confirm the same transition and keyboard cancellation sends no RPC", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const card = makeBoardCardDetail({ cardId: "pairwise-dnd", title: "Dnd Parity Card" });
    const transitions: Array<Readonly<Record<string, unknown>>> = [];
    for (const input of ["pointer", "keyboard"] as const) {
      using store = installWorkflowBoardFixtures([card]);
      store.on(WS_METHODS.workflowBoardApproveSpec, (payload, current) => {
        const next = current.advance(card.cardId, { stage: "plan" });
        return { cardId: next.cardId, revision: next.revision, card: next, specApproval: null };
      });
      await using screen = await mountBoardAppAt("/", { width: 1360, height: 860 });
      await openWorkflowBoard(screen);
      await expect
        .element(page.getByRole("dialog", { name: "Command palette" }))
        .not.toBeInTheDocument();
      if (input === "pointer") {
        using _drag = await dispatchPointerDrag(
          page.getByRole("button", { name: "Drag Dnd Parity Card to another stage" }).element(),
          page.getByRole("region", { name: "Plan", exact: true }).element(),
        );
      } else {
        const select = page
          .getByRole("region", { name: "Spec", exact: true })
          .getByRole("button", { name: /^Dnd Parity Card/ });
        await act(() => select.element().focus());
        await expect.element(select).toHaveFocus();
        await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
      }
      const dialog = page.getByRole("alertdialog", { name: "Approve spec and move to Plan" });
      await expect.element(dialog).toBeVisible();
      expect(store.callsTo(WS_METHODS.workflowBoardApproveSpec)).toHaveLength(0);
      await dialog.getByRole("button", { name: "Confirm spec approval" }).click();
      await vi.waitFor(() =>
        expect(store.callsTo(WS_METHODS.workflowBoardApproveSpec)).toHaveLength(1),
      );
      transitions.push(store.callsTo(WS_METHODS.workflowBoardApproveSpec)[0]!);
    }
    expect(transitions).toEqual([
      { cardId: card.cardId, expectedRevision: card.revision },
      { cardId: card.cardId, expectedRevision: card.revision },
    ]);
    using store = installWorkflowBoardFixtures([card]);
    await using screen = await mountBoardAppAt("/", { width: 390, height: 844 });
    await openWorkflowBoard(screen, { allowStageTabs: true });
    await expect
      .element(page.getByRole("dialog", { name: "Command palette" }))
      .not.toBeInTheDocument();
    const handle = page.getByRole("button", { name: "Drag Dnd Parity Card to another stage" });
    await act(() => handle.element().focus());
    await expect.element(handle).toHaveFocus();
    await userEvent.keyboard(" {ArrowRight}{Escape}");
    await expect.element(page.getByRole("alertdialog")).not.toBeInTheDocument();
    expect(
      store.calls.filter((call) =>
        [
          WS_METHODS.workflowBoardApproveSpec,
          WS_METHODS.workflowBoardApprovePlan,
          WS_METHODS.workflowBoardReturnForRevision,
          WS_METHODS.workflowBoardApproveDelivery,
        ].some((method) => method === call.method),
      ),
    ).toHaveLength(0);
    expect(window.innerWidth).toBe(390);
  });

  it("[T3-W-03] F4 x F6: codebase/file Spec sources preserve optional wireframes and sandbox the artifact preview", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const nonUi = makeBoardCardDetail({ cardId: "non-ui", title: "Non-UI Codebase Card" });
    const ui = makeBoardCardDetail({ cardId: "wireframe", title: "UI Brief Card" });
    const key = WorkflowBoardArtifactKey.make("wireframes/index.html");
    using store = installWorkflowBoardFixtures([
      {
        ...nonUi,
        spec: {
          ...nonUi.spec,
          sourceType: "codebase",
          sourceRefs: ["src/main.ts"],
          wireframesIndex: null,
        },
      },
      {
        ...ui,
        spec: {
          ...ui.spec,
          sourceType: "file",
          sourceRefs: ["docs/brief.md"],
          wireframesIndex: key,
        },
        artifactKeys: [key],
      },
    ]);
    store.artifacts.set(
      `${ui.cardId}:${key}`,
      "<!doctype html><html><body><main>Pairwise wireframe</main></body></html>",
    );
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByText(nonUi.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Spec", exact: true }).click();
    await expect.element(page.getByText("Skipped (non-UI)", { exact: true })).toBeVisible();
    expect(document.querySelectorAll("iframe")).toHaveLength(0);
    expect(store.get(nonUi.cardId).spec).toMatchObject({
      sourceType: "codebase",
      wireframesIndex: null,
      sourceRefs: ["src/main.ts"],
    });
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    await page.getByText(ui.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Spec", exact: true }).click();
    const frame = page.getByTitle("Wireframe preview for UI Brief Card");
    await expect.element(frame).toBeVisible();
    expect(frame.element().getAttribute("sandbox")).toBe("allow-scripts allow-forms");
    expect(store.get(ui.cardId).spec).toMatchObject({
      sourceType: "file",
      wireframesIndex: key,
      sourceRefs: ["docs/brief.md"],
    });
    expect(store.callsTo(WS_METHODS.workflowBoardGetArtifact)).toContainEqual({
      cardId: ui.cardId,
      key,
    });
    expect(store.callsTo(WS_METHODS.workflowBoardApproveSpec)).toHaveLength(0);
  });

  it("[T3-W-04] F5 x F7 x F8: paused exploration and manual challenge fallback gate Spec approval", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const base = makeBoardCardDetail({
      cardId: "exploration-gate",
      title: "Explore Challenge Card",
    });
    const card: WorkflowCardDetail = {
      ...base,
      spec: { ...base.spec, assistanceMode: "guided" },
      exploration: {
        roundIndex: NonNegativeInt.make(1),
        pendingRoundPrompt: false,
        paused: true,
        unresolvedIds: [],
        decisions: [
          {
            id: "DEC-1",
            prompt: "Storage?",
            status: "resolved",
            lastAction: "edit",
            answer: "SQLite",
            updatedAt: BOARD_AT,
          },
        ],
      },
      challengeReceipt: {
        status: "in_progress",
        concerns: [],
        synthesisCount: NonNegativeInt.make(1),
        specDigest: base.spec.specDigest!,
        interviewDigest: null,
        manualFallback: null,
        updatedAt: BOARD_AT,
      },
    };
    using store = installWorkflowBoardFixtures([card]);
    store.on(WS_METHODS.workflowBoardExploreStep, (input, current) => {
      expect(input.action).toBe("resume");
      const next = current.advance(card.cardId, {
        exploration: { ...card.exploration!, paused: false },
      });
      return {
        cardId: next.cardId,
        revision: next.revision,
        exploration: next.exploration,
        card: next,
      };
    });
    store.on(WS_METHODS.workflowBoardChallengeStep, (input, current) => {
      expect(input).toMatchObject({
        action: "manualFallback",
        acknowledgementToken: "I acknowledge",
        answers: ["Rollback storage changes", "AC-01 persistence", "Run planned checks"],
      });
      const next = current.advance(card.cardId, {
        challengeReceipt: {
          ...card.challengeReceipt!,
          status: "manual_fallback",
          manualFallback: {
            acknowledgementToken: "I acknowledge",
            answers: ["Rollback storage changes", "AC-01 persistence", "Run planned checks"],
            recordedAt: BOARD_AT,
          },
        },
      });
      return {
        cardId: next.cardId,
        revision: next.revision,
        challengeReceipt: next.challengeReceipt,
        card: next,
      };
    });
    store.on(WS_METHODS.workflowBoardApproveSpec, (input, current) => {
      expect(input.expectedRevision).toBe(current.get(card.cardId).revision);
      const next = current.advance(card.cardId, {
        stage: "plan",
        specApprovalStatus: "valid",
        specApproval: makeQualifiedPairwiseCard(card.cardId, card.title, "plan").specApproval,
      });
      return {
        cardId: next.cardId,
        revision: next.revision,
        specApproval: next.specApproval,
        card: next,
      };
    });
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByText(card.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Spec", exact: true }).click();
    const approve = page.getByRole("button", { name: "Approve spec", exact: true });
    await expect.element(approve).toBeDisabled();
    await page.getByRole("button", { name: "Resume exploration" }).click();
    await expect.element(page.getByText(/: SQLite/)).toBeVisible();
    await page.getByRole("button", { name: "Manual fallback", exact: true }).click();
    const submit = page.getByRole("button", { name: "Submit manual fallback" });
    await expect.element(submit).toBeDisabled();
    await page
      .getByLabelText("1. What failure mode or rollback boundary could invalidate this spec?")
      .fill("Rollback storage changes");
    await page
      .getByLabelText("2. Which acceptance criteria or file contracts remain most exposed?")
      .fill("AC-01 persistence");
    await page
      .getByLabelText("3. How will Plan verification confirm these assumptions?")
      .fill("Run planned checks");
    await page
      .getByLabelText("Type I acknowledge to confirm manual challenge completion")
      .fill("I acknowledge");
    await submit.click();
    await expect.element(approve).toBeEnabled();
    await approve.click();
    await vi.waitFor(() =>
      expect(store.callsTo(WS_METHODS.workflowBoardApproveSpec)).toHaveLength(1),
    );
    expect(store.callsTo(WS_METHODS.workflowBoardExploreStep)).toHaveLength(1);
    expect(store.callsTo(WS_METHODS.workflowBoardChallengeStep)).toHaveLength(1);
    expect(store.get(card.cardId).stage).toBe("plan");
  });

  it("[T3-W-05] F9 x F10 x F11 x F12: six lenses, alternative choice, custom strategy and complete AC mapping qualify Plan", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const card = makeQualifiedPairwiseCard("plan-gate", "Plan Gate Card", "plan");
    using store = installWorkflowBoardFixtures([card]);
    store.on(WS_METHODS.workflowBoardUpdatePlan, (input, current) => {
      expect(input).toMatchObject({
        selectedAlternativeId: "layered",
        strategy: "custom",
        strategyRationale: "Probe persistence before UI",
        lenses: expect.arrayContaining([
          { lens: "ux", applicable: false, summary: null, reason: "CLI only" },
        ]),
        acMappings: [{ criterionId: "AC-01", method: "automated", checkIds: ["CHK-1"] }],
      });
      const next = current.advance(card.cardId, {
        plan: { ...card.plan!, ...input, updatedAt: BOARD_AT },
      });
      return { cardId: next.cardId, revision: next.revision, plan: next.plan, card: next };
    });
    store.on(WS_METHODS.workflowBoardApprovePlan, (input, current) => {
      expect(input.expectedRevision).toBe(current.get(card.cardId).revision);
      const receipt = {
        ...card.specApproval!,
        id: WorkflowBoardApprovalId.make("plan-pairwise"),
        gate: "plan" as const,
        artifactDigest: card.plan!.planDigest!,
      };
      const next = current.advance(card.cardId, {
        stage: "execute",
        planApproval: receipt,
        planApprovalStatus: "valid",
      });
      return { cardId: next.cardId, revision: next.revision, planApproval: receipt, card: next };
    });
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByText(card.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Plan", exact: true }).click();
    for (const label of [
      "UX",
      "Functional",
      "Architecture",
      "Quality",
      "Infrastructure",
      "Delivery",
    ]) {
      await expect
        .element(page.getByRole("button", { name: `${label} applicable`, exact: true }))
        .toHaveAttribute("aria-pressed", "true");
    }
    await page.getByRole("button", { name: "UX not applicable", exact: true }).click();
    await page.getByLabelText("UX non-applicable reason").fill("CLI only");
    await page.getByRole("button", { name: "Select alternative", exact: true }).click();
    await page.getByRole("combobox", { name: "Planning strategy", exact: true }).click();
    await page.getByRole("option", { name: "Custom", exact: true }).click();
    await page.getByLabelText("Custom strategy rationale").fill("Probe persistence before UI");
    await page.getByLabelText("Check IDs for AC-01").fill("");
    await page.getByRole("button", { name: "Save plan", exact: true }).click();
    await expect
      .element(page.getByText("Complete all acceptance criterion evidence mappings before saving."))
      .toBeVisible();
    expect(store.callsTo(WS_METHODS.workflowBoardUpdatePlan)).toHaveLength(0);
    await page.getByLabelText("Check IDs for AC-01").fill("CHK-1");
    await page.getByRole("button", { name: "Save plan", exact: true }).click();
    await vi.waitFor(() =>
      expect(store.callsTo(WS_METHODS.workflowBoardUpdatePlan)).toHaveLength(1),
    );
    await page.getByRole("button", { name: "Approve plan", exact: true }).click();
    await vi.waitFor(() =>
      expect(store.callsTo(WS_METHODS.workflowBoardApprovePlan)).toHaveLength(1),
    );
    expect(store.get(card.cardId).stage).toBe("execute");
  });

  it("[T3-W-06] F14 x F15: preserved file-contract violation submits an amendment after revision conflict reconciliation", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const base = makeQualifiedPairwiseCard("amend-gate", "Out of Contract Card", "execute");
    const unexpected = WorkflowBoardArtifactKey.make("src/unexpected.ts");
    const card: WorkflowCardDetail = {
      ...base,
      status: "blocked",
      fileContractReports: [
        {
          taskId: "TSK-1",
          threadId: null,
          runId: null,
          status: "violation_pending",
          changedFiles: [unexpected],
          allowedFiles: [],
          unexpectedFiles: [unexpected],
          forbiddenFiles: [],
          preservedInWorktree: true,
          blockingReason: "Unexpected edits preserved",
          updatedAt: BOARD_AT,
        },
      ],
    };
    using store = installWorkflowBoardFixtures([card]);
    store.on(WS_METHODS.workflowBoardProposeAmendment, (input, current) => {
      if (current.callsTo(WS_METHODS.workflowBoardProposeAmendment).length === 1) {
        current.advance(card.cardId);
        throw boardRequestError(
          "revision_conflict",
          "Revision conflict: reconcile preserved edits",
        );
      }
      expect(input).toMatchObject({
        cardId: card.cardId,
        expectedRevision: 2,
        targetStage: "plan",
        sourceTaskId: "TSK-1",
        unexpectedFiles: [unexpected],
        tasks: [{ taskId: "TSK-1", filesAllowed: ["src/core.ts", unexpected] }],
      });
      const next = current.advance(card.cardId, {
        stage: "plan",
        status: "active",
        planApprovalStatus: "stale",
      });
      return {
        cardId: next.cardId,
        revision: next.revision,
        stage: next.stage,
        card: next,
        specApproval: next.specApproval,
        planApproval: next.planApproval ?? null,
        amendment: {
          id: "amend-1",
          cardRevision: next.revision,
          targetStage: "plan",
          reason: input.reason,
          summary: null,
          sourceTaskId: "TSK-1",
          unexpectedFiles: [unexpected],
          createdAt: BOARD_AT,
        },
      };
    });
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    await page.getByText(card.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Execute", exact: true }).click();
    const violation = page.getByRole("region", {
      name: "Unexpected edits preserved for review (TSK-1)",
    });
    await expect.element(violation).toBeVisible();
    await violation.getByRole("button", { name: "Propose plan amendment", exact: true }).click();
    const submit = page
      .getByRole("region", { name: "Execution lifecycle" })
      .getByRole("button", { name: "Submit amendment" });
    await submit.click();
    await expect.element(page.getByText(/This card changed on the server/)).toBeVisible();
    const workspace = page.getByRole("region", { name: "Card workspace" });
    await expect.element(workspace.getByText(/rev 2/).first()).toBeVisible();
    await submit.click();
    await vi.waitFor(() =>
      expect(store.callsTo(WS_METHODS.workflowBoardProposeAmendment)).toHaveLength(2),
    );
    expect(store.get(card.cardId).fileContractReports?.[0]?.preservedInWorktree).toBe(true);
    expect(store.get(card.cardId).stage).toBe("plan");
  });

  it("[T3-W-07] F16 x F17 x F18: stale checks, stale review and unresolved findings block Ship and expose working fix pointers", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const base = makeQualifiedPairwiseCard("ship-gate", "Ship Gate Blocked Card", "ship");
    const card: WorkflowCardDetail = {
      ...base,
      status: "blocked",
      shipReadiness: {
        status: "blocked",
        ready: false,
        productSnapshotDigest: "sha256:current",
        deliveryReceipt: null,
        blockers: [
          {
            id: "check",
            kind: "check_stale",
            message: "Check CHK-1 is stale",
            blocking: true,
            pointer: { stage: "execute", tab: "checks", targetId: "CHK-1" },
          },
          {
            id: "review",
            kind: "review_stale",
            message: "Independent review is stale",
            blocking: true,
            pointer: { stage: "execute", tab: "review", targetId: null },
          },
          {
            id: "finding",
            kind: "review_unresolved",
            message: "Finding FIND-1 is unresolved",
            blocking: true,
            pointer: { stage: "execute", tab: "review", targetId: "FIND-1" },
          },
        ],
      },
    };
    using store = installWorkflowBoardFixtures([card]);
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    await page.getByText(card.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Ship", exact: true }).click();
    await expect
      .element(page.getByRole("button", { name: "Approve delivery", exact: true }))
      .toBeDisabled();
    for (const [message, destination] of [
      ["Check CHK-1 is stale", "Planned checks & receipts"],
      ["Independent review is stale", "Independent review"],
      ["Finding FIND-1 is unresolved", "Independent review"],
    ] as const) {
      const blocker = page.getByText(message, { exact: true });
      await expect.element(blocker).toBeVisible();
      const item = blocker.element().closest("li")!;
      await userEvent.click(item.querySelector("button")!);
      await expect
        .element(page.getByRole("tab", { name: "Execute", exact: true }))
        .toHaveAttribute("aria-selected", "true");
      await expect
        .element(page.getByRole("region", { name: destination, exact: true }))
        .toBeVisible();
      await page.getByRole("tab", { name: "Ship", exact: true }).click();
    }
    expect(store.callsTo(WS_METHODS.workflowBoardApproveDelivery)).toHaveLength(0);
  });

  it("[T3-W-08] F18 x F19 x F24: scope-gated GitActionsControl disables Commit without write scope & imported CLI policies block Execute", async () => {
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
      using _vcs = installVcsHandlers({
        local: { hasPrimaryRemote: false },
        remote: null,
      });
      appAtomRegistry.refresh(environmentSession.sessionStateAtom(ENVIRONMENT_ID));
      await using _screen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/board-m5-thread`);
      await expect.element(page.getByRole("button", { name: "Commit" })).toBeDisabled();
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const base = makeQualifiedPairwiseCard("policy-gate", "spec-t3-code", "execute");
    const policies = ["sandbox.requireHardened", "qa.passScore", "skills.trustKeys"].map((key) => ({
      key,
      category: key.split(".")[0]!,
      summary: `${key} is unsupported`,
      configuredValue: "true",
      status: "unsupported" as const,
      blocksExecution: key === "sandbox.requireHardened",
      sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
    }));
    const card: WorkflowCardDetail = {
      ...base,
      planApprovalStatus: "historical",
      status: "blocked",
      blockerSummary: "TSK-0002 blocked",
      cliImport: {
        sourceSystem: "legion-cli",
        sourceCliDir: WorkflowBoardArtifactKey.make(".legion-cli"),
        sourceSpecId: "spec-t3-code",
        owner: "legioncode",
        importedAt: BOARD_AT,
        lastSyncedAt: BOARD_AT,
        sourceDigest: "sha256:import",
        decisionIds: [],
        assumptionIds: [],
        acceptanceCriterionIds: ["AC-01"],
        historicalReceipts: [],
        unsupportedPolicies: policies,
        acknowledgedPolicies: [],
        missingLogNotes: [],
        preservedArtifactPaths: [],
        assurancePolicySummary: null,
      },
    };
    using store = installWorkflowBoardFixtures([card]);
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    await page.getByText(card.title, { exact: true }).click();
    await page.getByRole("tab", { name: "Execute", exact: true }).click();
    for (const policy of policies) {
      await expect
        .element(
          page
            .getByRole("region", { name: "Unsupported .legion-cli policies (3)", exact: true })
            .getByText(policy.key, { exact: true }),
        )
        .toBeVisible();
    }
    const taskQueue = page.getByRole("region", { name: "Task queue & DAG", exact: true });
    const startTask = taskQueue.getByRole("button", { name: "Start task", exact: true });
    await expect.element(startTask).toBeDisabled();
    await expect
      .element(taskQueue.getByText("Acknowledge unsupported CLI policies before starting Execute."))
      .toBeVisible();
    await page
      .getByRole("checkbox", { name: /I acknowledge these unsupported .legion-cli policies/ })
      .click();
    await expect.element(startTask).toBeEnabled();
    expect(store.callsTo(WS_METHODS.workflowBoardStartStage)).toHaveLength(0);
  });
});

describe("E2E-M5 Tier 4 real-world browser scenarios", () => {
  it("[T4-W-01] Scenario 1 (Web): Labs workflows toggle + CommandPalette deferred search -> M1 handoff across restart", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: false,
        projectWorkflowBoard: true,
      });
      using _settings = answerRpc(
        WS_METHODS.serverUpdateSettings,
        (input: { readonly patch?: { readonly workflowsEnabled?: boolean } }) => {
          expect(input.patch?.workflowsEnabled).toBe(true);
          boardEnvState.workflowsEnabled = true;
          boardEnvState.emitConfigSnapshot();
          return Effect.succeed({ settings: { workflowsEnabled: true } });
        },
      );
      await using _labsScreen = await mountBoardAppAt("/settings/labs");
      const workflowsToggle = page.getByRole("switch", { name: "Workflows" });
      await expect.element(workflowsToggle).toBeVisible();
      await expect.element(workflowsToggle).toHaveAttribute("aria-checked", "false");
      await expect
        .element(page.getByRole("dialog", { name: "Settings", exact: true }))
        .toHaveFocus();
      await act(() => workflowsToggle.element().focus());
      await expect.element(workflowsToggle).toHaveFocus();
      await userEvent.keyboard(" ");
      await expect.element(workflowsToggle).toHaveAttribute("aria-checked", "true");
    }

    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: true,
      });
      using _wf = answerWorkflowRunsEmpty();
      await using _homeScreen = await mountBoardAppAt("/");
      openCommandPalette();
      const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
      await expect.element(paletteInput).toBeVisible();
      await userEvent.fill(paletteInput, "Open workflows");
      await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
      await userEvent.fill(paletteInput, "workflow board");
      await vi.waitFor(() => {
        expect(page.getByRole("option", { name: "Open workflows" }).elements()).toHaveLength(0);
      });
      await expect
        .element(page.getByRole("option", { name: /Open workflow board/i }).first())
        .toBeVisible();
    }

    let persistedCards: Array<M5CardState> = [];
    {
      using _env1 = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: true,
        providers: [DEV_REPLAY_SERVER_PROVIDER, CODEX_PROVIDER],
      });
      using board1 = installStatefulM5BoardRpc([]);
      await using screen1 = await mountBoardAppAt("/");
      await openWorkflowBoard(screen1);

      await page.getByRole("button", { name: "New card" }).click();
      await expect.element(page.getByRole("heading", { name: "New workflow card" })).toBeVisible();
      await userEvent.fill(page.getByLabelText("Title"), "M1 Web Card");
      await userEvent.fill(page.getByLabelText("Initial spec or idea"), "Initial M1 spec");
      await page.getByRole("button", { name: "Create card" }).click();

      const briefInput = page.getByLabelText("Spec brief (Markdown)");
      await expect.element(briefInput).toBeVisible();
      await userEvent.fill(briefInput, "Updated M1 spec");
      await userEvent.fill(
        page.getByLabelText("Acceptance criteria (one per line)"),
        "AC-1: Plan handoff works",
      );
      await page.getByRole("button", { name: "Save spec" }).click();
      await expect.element(page.getByText(/rev 2/).first()).toBeVisible();
      await expect.element(briefInput).toHaveValue("Updated M1 spec");

      await page.getByRole("button", { name: "Approve spec" }).click();
      await expect
        .element(
          page
            .getByRole("region", { name: "Card workspace" })
            .getByText("Spec approved", { exact: true })
            .first(),
        )
        .toBeVisible();

      const cardId = [...board1.cards.keys()][0]!;
      expect(board1.cards.get(cardId)?.stage).toBe("plan");
      const approveCall = board1.stub.calls.find((c) => c.method === "workflowBoard.approveSpec");
      expect(approveCall).toBeDefined();
      expect(approveCall!.input).toMatchObject({ cardId, expectedRevision: 2 });
      expect("specDigest" in (approveCall!.input as Record<string, unknown>)).toBe(false);

      persistedCards = [...board1.cards.values()].map((c) => ({
        ...c,
        acceptanceCriteria: [...c.acceptanceCriteria],
        constraints: [...c.constraints],
        sourceRefs: [...c.sourceRefs],
        stageRuns: [...c.stageRuns],
        artifactKeys: [...c.artifactKeys],
      }));
    }

    {
      using _env2 = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: true,
        providers: [DEV_REPLAY_SERVER_PROVIDER, CODEX_PROVIDER],
      });
      using board2 = installStatefulM5BoardRpc(persistedCards, { initialSnapshotRevision: 10 });
      await using screen2 = await mountBoardAppAt("/");
      await openWorkflowBoard(screen2);

      const planRegion = page.getByRole("region", { name: "Plan", exact: true });
      const savedCard = planRegion.getByRole("button", { name: /^M1 Web Card/ });
      await expect.element(savedCard).toBeVisible();
      await savedCard.click();

      await expect.element(page.getByText(/rev 3/).first()).toBeVisible();
      await expect
        .element(
          page
            .getByRole("region", { name: "Card workspace" })
            .getByText("Spec approved", { exact: true })
            .first(),
        )
        .toBeVisible();
      await page.getByRole("tab", { name: "Spec", exact: true }).click();
      await expect
        .element(page.getByLabelText("Spec brief (Markdown)"))
        .toHaveValue("Updated M1 spec");
      await page.getByRole("tab", { name: "Plan", exact: true }).click();
      await expect
        .element(page.getByRole("button", { name: "Start Plan", exact: true }))
        .toBeEnabled();

      await page.getByRole("button", { name: "Start Plan" }).click();
      await expect
        .element(page.getByRole("heading", { name: "Prepared Plan brief" }))
        .toBeVisible();
      await expect
        .element(page.getByRole("heading", { name: "Plan Brief: M1 Web Card" }))
        .toBeVisible();
      const threadPane = page.getByRole("region", { name: "Connected stage thread" });
      await expect.element(threadPane.getByText("Connected Plan thread")).toBeVisible();
      await expect.element(threadPane.getByText("board-m5-thread: question 0")).toBeVisible();

      const resumePlanBtn = page.getByRole("button", { name: "Resume Plan" });
      await expect.element(resumePlanBtn).toBeVisible();
      await resumePlanBtn.click();

      await vi.waitFor(() => {
        const startCalls = board2.stub.calls.filter((c) => c.method === "workflowBoard.startStage");
        expect(startCalls).toHaveLength(2);
      });
      expect(board2.startStageResults).toHaveLength(2);
      expect(board2.startStageResults[0]?.resumed).toBe(false);
      expect(board2.startStageResults[1]?.resumed).toBe(true);
      expect(board2.startStageResults[0]?.threadId).toBe("board-m5-thread");
      expect(board2.startStageResults[1]?.threadId).toBe("board-m5-thread");
      const firstStartCall = board2.stub.calls.find((c) => c.method === "workflowBoard.startStage");
      expect(firstStartCall?.input).toMatchObject({
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      });
    }
  });

  it("[T4-W-02] Scenario 2 (Web): Sandboxed HtmlRenderDocument wireframe zoom -> Guided UI feature to Ship + individual commit", async () => {
    using blob = makeBlobPage("<!doctype html><main><h1>Guided UI</h1></main>");
    {
      await using doc = await render(
        <HtmlRenderDocument src={blob.url} title="Guided UI Wireframe" />,
      );
      const iframe = doc.getByTitle("Guided UI Wireframe");
      await expect.element(iframe).toBeVisible();
      expect(iframe.element().getAttribute("sandbox")).toBe("allow-scripts allow-forms");
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using vcs = installVcsHandlers();
    const base = makeBoardCardDetail({ cardId: "card-s2-ui", title: "Guided Wireframe Feature" });
    const card: WorkflowCardDetail = {
      ...base,
      spec: {
        ...base.spec,
        assistanceMode: "guided",
        wireframesIndex: WorkflowBoardArtifactKey.make("wireframes/index.html"),
      },
    };
    const snapshot = "sha256:guided-wireframe-reviewed";
    const approvalId = WorkflowBoardApprovalId.make("guided-ui-delivery");
    using store = installWorkflowBoardFixtures([card]);
    store.on(WS_METHODS.workflowBoardApproveDelivery, (input, state) => {
      expect(input).toMatchObject({
        cardId: card.cardId,
        expectedRevision: 2,
        expectedProductSnapshotDigest: snapshot,
      });
      const revision = PositiveInt.make(3);
      const deliveryReceipt = {
        id: "guided-ui-receipt",
        approvalId,
        cardRevision: revision,
        productSnapshotDigest: snapshot,
        commitSha: null,
        specDigest: card.spec.specDigest,
        planDigest: null,
        tasksDigest: null,
        checksDigest: null,
        reviewId: null,
        acVerdictsSummary: "[]",
        approvedBy: "operator",
        approvedAt: BOARD_AT,
        status: "shipped" as const,
      };
      const deliveryApproval = {
        id: approvalId,
        gate: "delivery",
        status: "valid",
        revision,
        artifactDigest: snapshot,
        specDigest: card.spec.specDigest,
        wireframesDigest: null,
        approvedAt: BOARD_AT,
        approvedBy: "operator",
      };
      const shipped = state.advance(card.cardId, {
        status: "completed",
        deliveryStatus: "shipped",
        deliveryReceipt,
        shipReadiness: {
          status: "shipped",
          ready: false,
          productSnapshotDigest: snapshot,
          blockers: [],
          deliveryReceipt,
        },
      });
      return {
        cardId: card.cardId,
        revision,
        status: shipped.status,
        deliveryStatus: "shipped",
        deliveryReceipt,
        deliveryApproval,
        card: shipped,
      };
    });

    {
      await using screen = await mountBoardAppAt("/");
      await openWorkflowBoard(screen, { allowStageTabs: true });
      await page.getByRole("tab", { name: "Overview", exact: true }).click();
      await page.getByRole("button", { name: /^Guided Wireframe Feature/ }).click();
      await expect.element(page.getByLabelText("Spec brief (Markdown)")).toBeVisible();

      store.advance(card.cardId, {
        stage: "ship",
        deliveryStatus: "ready",
        shipReadiness: {
          status: "ready",
          ready: true,
          productSnapshotDigest: snapshot,
          blockers: [],
          deliveryReceipt: null,
        },
      });
      await page.getByRole("tab", { name: "Ship", exact: true }).click();
      const approve = page.getByRole("button", { name: "Approve delivery", exact: true });
      await expect.element(approve).toBeEnabled();
      await approve.click();
      await vi.waitFor(() =>
        expect(store.callsTo(WS_METHODS.workflowBoardApproveDelivery)).toHaveLength(1),
      );
      expect(store.get(card.cardId).deliveryReceipt?.productSnapshotDigest).toBe(snapshot);
      expect(vcs.stackedCalls).toHaveLength(0);
    }

    await using thread = await mountBoardAppAt(`/${ENVIRONMENT_ID}/board-m5-thread`);
    await thread.getByRole("button", { name: "Git action options" }).click();
    await page.getByRole("menuitem", { name: "Commit" }).click();
    await page.getByPlaceholder("Leave empty to auto-generate").fill("feat: ship guided wireframe");
    await page.getByRole("button", { name: "Commit", exact: true }).click();
    await vi.waitFor(() => expect(vcs.stackedCalls).toHaveLength(1));
    expect(vcs.stackedCalls[0]).toMatchObject({
      action: "commit",
      commitMessage: "feat: ship guided wireframe",
    });
  });

  it("[T4-W-04] Scenario 4 (Web): @dnd-kit pointer/keyboard parity + Spec revision return invalidation cascade", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using board = installStatefulM5BoardRpc([
      makeM5CardState({
        cardId: "card-s4-ret" as WorkflowCardId,
        title: "Revision Return Card",
        revision: 3,
        stage: "execute",
        specApprovedRevision: 2,
        specApprovalStatus: "valid",
        planApprovalStatus: "valid",
      }),
    ]);
    board.stub.on(WS_METHODS.workflowBoardReturnForRevision, (input, state) => {
      expect(input).toMatchObject({
        cardId: "card-s4-ret",
        expectedRevision: 3,
        targetStage: "spec",
      });
      board.pushExternalUpdate("card-s4-ret", (next) => {
        next.stage = "spec";
        next.revision += 1;
        next.specApprovalStatus = "stale";
        next.planApprovalStatus = "stale";
      });
      const returned = state.get("card-s4-ret");
      return {
        cardId: returned.cardId,
        revision: returned.revision,
        stage: returned.stage,
        specApproval: returned.specApproval,
        planApproval: returned.planApproval ?? null,
        history: returned.revisions,
        card: returned,
      };
    });
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    await expect
      .element(page.getByRole("dialog", { name: "Command palette" }))
      .not.toBeInTheDocument();

    const dialog = page.getByRole("alertdialog", { name: "Return card for revision" });
    const drag = page.getByRole("button", { name: "Drag Revision Return Card to another stage" });
    const plan = page.getByRole("region", { name: "Plan", exact: true });
    {
      using _drag = await dispatchPointerDrag(drag.element(), plan.element());
      await expect.element(dialog).toBeVisible();
      await dialog.getByRole("button", { name: "Cancel" }).click();
    }
    await expect.element(dialog).not.toBeInTheDocument();
    expect(
      board.stub.calls.filter((call) => call.method === WS_METHODS.workflowBoardReturnForRevision),
    ).toHaveLength(0);

    const select = page
      .getByRole("region", { name: "Execute", exact: true })
      .getByRole("button", { name: /^Revision Return Card/ });
    await act(() => select.element().focus());
    await expect.element(select).toHaveFocus();
    await userEvent.keyboard("{Alt>}{ArrowLeft}{/Alt}");
    await expect.element(dialog).toBeVisible();
    await dialog.getByRole("combobox", { name: "Target stage" }).click();
    await page.getByRole("option", { name: "Spec", exact: true }).click();
    await dialog.getByLabelText("Reason for revision").fill("Clarify acceptance evidence");
    await dialog.getByRole("button", { name: "Confirm return for revision" }).click();
    await expect.element(dialog).not.toBeInTheDocument();
    const returns = board.stub.calls.filter(
      (call) => call.method === WS_METHODS.workflowBoardReturnForRevision,
    );
    expect(returns.map((call) => call.input)).toEqual([
      {
        cardId: "card-s4-ret",
        expectedRevision: 3,
        targetStage: "spec",
        reason: "Clarify acceptance evidence",
      },
    ]);
    expect(board.cards.get("card-s4-ret")).toMatchObject({
      stage: "spec",
      specApprovalStatus: "stale",
      planApprovalStatus: "stale",
    });

    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    const returnedCard = page
      .getByRole("region", { name: "Spec", exact: true })
      .getByRole("button", { name: /^Revision Return Card/ });
    await returnedCard.click();
    await page.getByLabelText("Spec brief (Markdown)").fill("Edited spec after return");
    await page.getByRole("button", { name: "Save spec" }).click();
    await expect.element(page.getByText("Approval stale", { exact: true }).first()).toBeVisible();
    await page.getByRole("button", { name: "Approve spec", exact: true }).click();
    await vi.waitFor(() =>
      expect(board.cards.get("card-s4-ret")?.specApprovalStatus).toBe("valid"),
    );
    expect(board.cards.get("card-s4-ret")?.planApprovalStatus).toBe("stale");
  });

  it("[T4-W-06] Scenario 6 (Web): /workflows paused run inspector -> failed check repair & stale recovery on board", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      const pausedRun = makeWorkflowRun({
        runId: "run-s6-paused",
        title: "Verify check CHK-1",
        status: "paused",
        detail: "exit 1",
        pauses: 1,
      });
      using _sub = answerRpc(WS_METHODS.workflowsSubscribe, () =>
        Stream.make({ workflows: [], runs: [pausedRun], questions: [] }),
      );
      using _cnt = answerRpc(WS_METHODS.workflowsCounts, () =>
        Stream.make({ needsYou: 1, active: 0, total: 1 }),
      );
      using _get = answerRpc(WS_METHODS.workflowsGet, () =>
        Effect.succeed({
          run: pausedRun,
          steps: [
            makeWorkflowStep({
              name: "check",
              kind: "agent",
              status: "failed",
              threadId: "board-m5-thread",
            }),
          ],
          references: [],
        }),
      );
      appAtomRegistry.refresh(
        serverEnvironment.workflowsLive({ environmentId: ENVIRONMENT_ID, input: {} }),
      );
      await using _wfScreen = await mountBoardAppAt(
        `/workflows?env=${ENVIRONMENT_ID}&run=run-s6-paused`,
      );
      await expect.element(page.getByRole("region", { name: "Why it paused" })).toBeVisible();
      await expect.element(page.getByRole("button", { name: "Resume" })).toBeVisible();
      await expect.element(page.getByRole("button", { name: "Transcript" })).toBeVisible();
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const failedReceipt = {
      checkId: "CHK-1",
      argv: ["node", "verify.js"] as const,
      status: "failed" as const,
      exitCode: 1,
      logRef: ".legion/checks/verify.log",
      productSnapshotDigest: "sha256:failed",
      executedAt: BOARD_AT,
      updatedAt: BOARD_AT,
    };
    const card = makeBoardCardDetail({
      cardId: "card-s6-chk",
      title: "Failed Check Repair Card",
      stage: "execute",
      status: "blocked",
      blockerSummary: "CHK-1 failed (exit 1)",
      checkReceipts: [failedReceipt],
    });
    using store = installWorkflowBoardFixtures([card]);
    store.on(WS_METHODS.workflowBoardRetryStep, (input, state) => {
      expect(input).toMatchObject({
        cardId: card.cardId,
        expectedRevision: 2,
        stepKind: "check",
        targetId: "CHK-1",
      });
      const updated = state.advance(card.cardId, {
        status: "active",
        blockerSummary: null,
        checkReceipts: [
          {
            ...failedReceipt,
            status: "passed",
            exitCode: 0,
            productSnapshotDigest: "sha256:repaired",
          },
        ],
      });
      return {
        cardId: card.cardId,
        revision: updated.revision,
        stepKind: "check",
        targetId: "CHK-1",
        card: updated,
      };
    });
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByRole("button", { name: /^Failed Check Repair Card/ }).click();
    await page.getByRole("tab", { name: "Execute", exact: true }).click();
    await expect.element(page.getByText("Failed", { exact: true }).first()).toBeVisible();

    store.advance(card.cardId, { checkReceipts: [{ ...failedReceipt, status: "stale" }] });
    await expect.element(page.getByText("Stale", { exact: true }).first()).toBeVisible();
    await page.getByRole("button", { name: "Re-run check" }).click();
    await vi.waitFor(() =>
      expect(store.callsTo(WS_METHODS.workflowBoardRetryStep)).toHaveLength(1),
    );
    await expect.element(page.getByText("Passed", { exact: true }).first()).toBeVisible();
  });

  it("[T4-W-09] Scenario 9 (Web): /workflows omits import when capability is false -> CLI import spec-t3-code on board", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      using _wf = answerWorkflowRunsEmpty();
      await using wfScreen = await mountBoardAppAt("/workflows");
      const mainRegion = wfScreen.getByRole("main");
      await expect.element(mainRegion.getByRole("heading", { name: "Workflows" })).toBeVisible();
      expect(mainRegion.getByRole("button", { name: /import/i }).elements()).toHaveLength(0);
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using store = installWorkflowBoardFixtures([]);
    const imported = makeBoardCardDetail({
      cardId: "card-s9-imp",
      title: "spec-t3-code",
      stage: "execute",
      status: "blocked",
      blockerSummary: "TSK-0002 blocked",
    });
    const task = {
      id: "TSK-0002",
      taskId: "TSK-0002",
      title: "Verify qualification",
      status: "blocked" as const,
      blockerSummary: "Verification exit 1",
      verificationCommands: [],
      missingLogNotes: [],
    };
    const evidence = { historicalReceipts: [], unsupportedPolicies: [], missingLogNotes: [] };
    store.on(
      WS_METHODS.workflowBoardPreviewCliImport,
      () =>
        ({
          ...evidence,
          projectId: PROJECT_ID,
          sourcePath: WorkflowBoardArtifactKey.make(".legion-cli"),
          specId: "spec-t3-code",
          title: "spec-t3-code",
          stage: "execute",
          status: "blocked",
          existingCardId: null,
          alreadyImported: false,
          activeLocks: [],
          staleLocks: [],
          canApply: true,
          refusalReason: null,
          tasks: [task],
          decisionIds: [],
          assumptionIds: [],
          acceptanceCriterionIds: [],
          warnings: [],
        }) satisfies WorkflowBoardPreviewCliImportResult,
    );
    store.on(WS_METHODS.workflowBoardApplyCliImport, (_input, state) => {
      state.put(imported);
      return {
        ...evidence,
        cardId: imported.cardId,
        projectId: PROJECT_ID,
        revision: imported.revision,
        created: true,
        sourceSpecId: "spec-t3-code",
        card: imported,
        tasks: [
          {
            taskId: task.taskId,
            title: task.title,
            status: task.status,
            filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/workflow.ts")],
            testingMethod: "test-first",
          },
        ],
      } satisfies WorkflowBoardApplyCliImportResult;
    });
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    expect(store.cards.size).toBe(0);
    await page.getByRole("button", { name: "Import CLI project" }).click();
    const dialog = page.getByRole("dialog", { name: "Import CLI project" });
    await expect.element(dialog.getByText("TSK-0002", { exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: "Apply import" }).click();
    await expect.element(dialog).not.toBeInTheDocument();
    expect(store.callsTo(WS_METHODS.workflowBoardPreviewCliImport)).toHaveLength(1);
    expect(store.callsTo(WS_METHODS.workflowBoardApplyCliImport)).toHaveLength(1);
    expect(store.cards.get(imported.cardId)).toMatchObject({
      title: "spec-t3-code",
      blockerSummary: "TSK-0002 blocked",
    });
    await expect.element(page.getByText("TSK-0002 blocked").first()).toBeVisible();
  });

  it("[T4-W-11] Scenario 11 (Web): GitActionsControl individual Commit/Push/PR + IME guard -> Mobile approve & Web reopen", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      using vcs = installVcsHandlers();
      await using _thread = await mountBoardAppAt(`/${ENVIRONMENT_ID}/board-m5-thread`);
      const actions = page.getByRole("button", { name: "Git action options" });
      await actions.click();
      await page.getByRole("menuitem", { name: "Commit" }).click();
      await page.getByPlaceholder("Leave empty to auto-generate").fill("feat: handoff");
      await page.getByRole("button", { name: "Commit", exact: true }).click();
      await vi.waitFor(() => expect(vcs.stackedCalls).toHaveLength(1));

      await actions.click();
      await page.getByRole("menuitem", { name: "Push" }).click();
      await vi.waitFor(() => expect(vcs.stackedCalls).toHaveLength(2));
      await actions.click();
      await page.getByRole("menuitem", { name: "Create PR" }).click();
      await vi.waitFor(() => expect(vcs.stackedCalls).toHaveLength(3));
      expect(vcs.stackedCalls.map((call) => call.action)).toEqual(["commit", "push", "create_pr"]);
    }

    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      using vcs = installVcsHandlers({
        local: { hasPrimaryRemote: false, hasWorkingTreeChanges: false },
        remote: { hasUpstream: false, aheadCount: 0, behindCount: 0 },
      });
      await using _thread = await mountBoardAppAt(`/${ENVIRONMENT_ID}/board-m5-thread`);
      await page.getByRole("button", { name: "Publish repository" }).click();
      await page.getByRole("button", { name: "Next" }).click();
      const repository = page.getByRole("textbox", { name: "Repository" });
      await repository.fill("fixture/workflow-board");
      await act(() => {
        repository.element().dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Enter",
            isComposing: true,
            bubbles: true,
            cancelable: true,
          }),
        );
      });
      expect(vcs.publishCalls).toHaveLength(0);
      await userEvent.keyboard("{Enter}");
      await vi.waitFor(() => expect(vcs.publishCalls).toHaveLength(1));
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using board = installStatefulM5BoardRpc([
      makeM5CardState({
        cardId: "card-s11-ship" as WorkflowCardId,
        title: "Cross Surface Ship Card",
        stage: "ship",
      }),
    ]);
    let boardHref: string;
    {
      await using screen = await mountBoardAppAt("/");
      boardHref = await openWorkflowBoard(screen, { allowStageTabs: true });
      await page.getByRole("tab", { name: "Overview", exact: true }).click();
      await expect
        .element(
          page
            .getByRole("region", { name: "Ship", exact: true })
            .getByText("Cross Surface Ship Card"),
        )
        .toBeVisible();
      board.pushExternalUpdate("card-s11-ship", (card) => {
        card.status = "completed";
        card.revision += 1;
      });
      await expect
        .element(
          page
            .getByRole("region", { name: "Completed cards" })
            .getByText("Cross Surface Ship Card"),
        )
        .toBeVisible();
    }

    await using reopened = await mountBoardAppAt(boardHref);
    const completed = reopened.getByRole("region", { name: "Completed cards" });
    await completed.getByText("Cross Surface Ship Card").click();
    await expect.element(reopened.getByText("Shipped", { exact: true }).first()).toBeVisible();
    expect(
      board.stub.calls.filter((call) => call.method === WS_METHODS.workflowBoardApproveDelivery),
    ).toHaveLength(0);
  });

  it("[T4-W-12] Scenario 12 (Web): legacy server fallback + German/Japanese i18n locale switching + keyboard drag", async () => {
    await withLocaleCleanup(async () => {
      {
        using _env = setupBoardAppEnvironment({
          workflowsEnabled: false,
          projectWorkflowBoard: false,
          language: "de",
        });
        startI18n();
        await whenLocaleReady();
        using _wf = answerWorkflowRunsEmpty();
        await using deScreen = await mountBoardAppAt("/workflows");
        await expect
          .element(deScreen.getByText("Workflows sind aus", { exact: true }))
          .toBeVisible();
        expect(deScreen.getByRole("button", { name: /import/i }).elements()).toHaveLength(0);
      }
      {
        using _env = setupBoardAppEnvironment({
          workflowsEnabled: false,
          projectWorkflowBoard: false,
          language: "ja",
        });
        startI18n();
        await whenLocaleReady();
        using _wf = answerWorkflowRunsEmpty();
        await using jaScreen = await mountBoardAppAt("/workflows");
        await expect
          .element(jaScreen.getByText("ワークフローはオフです", { exact: true }))
          .toBeVisible();
        expect(jaScreen.getByRole("button", { name: /import/i }).elements()).toHaveLength(0);
      }
    });

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using board = installStatefulM5BoardRpc([
      makeM5CardState({ cardId: "card-s12-kbd" as WorkflowCardId, title: "Keyboard Drag Card" }),
    ]);
    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoard(screen);
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    for (const col of ["Spec", "Plan", "Execute", "Ship"]) {
      await expect.element(page.getByRole("region", { name: col })).toBeVisible();
    }

    boardEnvState.projectWorkflowBoard = false;
    boardEnvState.emitConfigSnapshot?.();
    await expect.element(screen.getByText("Workflow board is not supported")).toBeVisible();

    boardEnvState.projectWorkflowBoard = true;
    boardEnvState.emitConfigSnapshot?.();
    for (const col of ["Spec", "Plan", "Execute", "Ship"]) {
      await expect.element(page.getByRole("region", { name: col })).toBeVisible();
    }
    const card = page
      .getByRole("region", { name: "Spec", exact: true })
      .getByRole("button", { name: /^Keyboard Drag Card/ });
    await act(() => card.element().focus());
    await expect.element(card).toHaveFocus();
    await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
    const dialog = page.getByRole("alertdialog", { name: "Approve spec and move to Plan" });
    await expect.element(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect.element(dialog).not.toBeInTheDocument();
    expect(
      board.stub.calls.filter((call) => call.method === WS_METHODS.workflowBoardApproveSpec),
    ).toHaveLength(0);
  });
});
