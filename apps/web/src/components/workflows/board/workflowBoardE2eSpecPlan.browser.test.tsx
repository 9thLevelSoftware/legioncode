import {
  EnvironmentId,
  NonNegativeInt,
  PositiveInt,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  type WorkflowBoardAcEvidenceMapping,
  type WorkflowBoardApprovalReceipt,
  type WorkflowBoardChallengeStepInput,
  type WorkflowBoardExploreAction,
  type WorkflowBoardExploreStepInput,
  type WorkflowBoardPlannedCheck,
  type WorkflowBoardUpdatePlanInput,
  type WorkflowBoardTaskContract,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { isMacPlatform } from "@legioncode/shared/keybindings";
import { createMemoryHistory } from "@tanstack/react-router";
import { DEFAULT_CLIENT_SETTINGS } from "@legioncode/contracts/settings";
import type * as Context from "effect/Context";
import { describe, expect, it, vi } from "vite-plus/test";
import { page, userEvent } from "vite-plus/test/browser";

import { AppRoot } from "~/AppRoot";
import { openCommandPalette } from "~/commandPaletteBus";
import { useCommandPaletteStore } from "~/commandPaletteStore";
import "~/components/ChatView";
import { HtmlRenderFrame } from "~/components/chat/HtmlRenderFrame";
import { HtmlRenderDocument } from "~/components/files/BrowserDocumentFrame";
import { __setClientSettingsForTests } from "~/hooks/useSettings";
import "~/index.css";
import { ensureLocalApi } from "~/localApi";
import { getRouter } from "~/router";
import { appAtomRegistry } from "~/rpc/atomRegistry";
import {
  installFakeEnvironment,
  threadWithHistory,
  type FakeDriver,
} from "../../../../test/fakeEnvironment";
import { render } from "../../../../test/render";
import { withDesktopViewport } from "./workflowBoardE2eHarness";
import {
  BOARD_AT,
  boardRequestError,
  installWorkflowBoardFixtures,
  makeBoardCardDetail,
  type WorkflowBoardFixtureStore,
} from "./workflowBoardE2eFixtures";

const wireframeAssetState = vi.hoisted(() => ({
  tag: "Success" as "Success" | "Failure",
  url: "about:blank",
  refreshUrl: "about:blank" as string | null,
}));

const boardEnvState = vi.hoisted(() => ({
  workflowsEnabled: false,
  projectWorkflowBoard: false,
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
    const [actual, { fakeDriver }, Layer, EffectMod, StreamMod] = await Promise.all([
      importOriginal<{ readonly ConnectionDriver: Context.Key<unknown, FakeDriver> }>(),
      import("../../../../test/fakeEnvironment"),
      import("effect/Layer"),
      import("effect/Effect"),
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
            subscribeServerConfig: () =>
              StreamMod.map(lease.session.subscribeServerConfig(), (evt) => {
                if (evt.type === "snapshot") {
                  return {
                    ...evt,
                    config: buildAugmentedConfig(
                      evt.config as unknown as Record<string, unknown>,
                    ) as never,
                  };
                }
                return evt;
              }),
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

function resetAssetMock(url = "about:blank") {
  wireframeAssetState.tag = "Success";
  wireframeAssetState.url = url;
  wireframeAssetState.refreshUrl = url;
}

function makeBlobPage(html: string): { readonly url: string; [Symbol.dispose](): void } {
  const url = URL.createObjectURL(new Blob([html], { type: "text/html" }));
  return {
    url,
    [Symbol.dispose]: () => URL.revokeObjectURL(url),
  };
}

function setupBoardSpecPlanApp() {
  useCommandPaletteStore.setState(useCommandPaletteStore.getInitialState(), true);
  boardEnvState.workflowsEnabled = true;
  boardEnvState.projectWorkflowBoard = true;
  __setClientSettingsForTests({
    ...DEFAULT_CLIENT_SETTINGS,
    onboardingCompletedAt: "2026-01-01T00:00:00.000Z",
  });
  return installFakeEnvironment([threadWithHistory("spec-plan-thread", 1)]);
}

async function mountBoardSpecPlanApp(initialEntry = "/") {
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
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}

const sizeChangedMsg = (height: number) => ({
  jsonrpc: "2.0",
  method: "ui/notifications/size-changed",
  params: { height },
});

interface TestDecisionSeed {
  id: string;
  prompt: string;
  status: "unresolved" | "resolved";
  answer?: string | null;
  explanation?: string;
  recommendation?: string;
  lastAction?: WorkflowBoardExploreAction;
}

interface TestConcernSeed {
  id: string;
  summary: string;
  disposition: NonNullable<
    WorkflowCardDetail["challengeReceipt"]
  >["concerns"][number]["disposition"];
  answer?: string;
  reason?: string;
}

interface TestLensSeed {
  lens: "ux" | "functional" | "architecture" | "quality" | "infrastructure" | "delivery";
  applicable: boolean | null;
  summary?: string;
  reason?: string;
}

interface TestAlternativeSeed {
  id: string;
  title: string;
  summary: string;
  tradeoffs: string;
}

interface TestTaskSeed {
  taskId: string;
  title: string;
  dependsOn: Array<string>;
  filesAllowed: Array<string>;
  filesForbidden: Array<string>;
  testingMethod: string;
}

interface TestAcMappingSeed {
  criterionId: string;
  method: "automated" | "manual" | "external";
  command?: ReadonlyArray<string>;
  checkIds?: ReadonlyArray<string>;
  procedure?: string;
}

interface TestCardSeed {
  cardId: string;
  title: string;
  brief?: string;
  stage?: "spec" | "plan" | "execute" | "ship";
  revision?: number;
  sourceType?: "idea" | "file" | "codebase";
  intakeGoal?: "greenfield" | "change" | "audit";
  assistanceMode?: "guided" | "balanced" | "direct";
  sourceRefs?: Array<string>;
  wireframesIndex?: string | null;
  acceptanceCriteria?: Array<{ readonly id: string; readonly text: string }>;
  constraints?: Array<string>;
  suppliedDecisions?: Array<string>;
  inferredDecisions?: Array<string>;
  missingDecisions?: Array<string>;
  explorationPaused?: boolean;
  decisions?: Array<TestDecisionSeed>;
  challengeAutomationFailed?: boolean;
  concerns?: Array<TestConcernSeed>;
  synthesisCount?: number;
  manualAnswers?: readonly [string, string, string];
  challengeCompleted?: boolean;
  specApproval?: {
    readonly status: "valid" | "stale" | "none";
    readonly specDigest?: string;
  } | null;
  planApproval?: {
    readonly status: "valid" | "stale" | "none";
    readonly planDigest?: string;
  } | null;
  lenses?: Array<TestLensSeed>;
  alternatives?: Array<TestAlternativeSeed>;
  alternativesRevisionCount?: number;
  selectedAlternativeId?: string | null;
  selectionRationale?: string | null;
  strategy?: "outcomes" | "risk-first" | "expand-contract" | "custom";
  strategyRationale?: string;
  granularity?: "coarse" | "balanced" | "fine";
  tasks?: Array<TestTaskSeed>;
  acMappings?: ReadonlyArray<TestAcMappingSeed>;
  checks?: ReadonlyArray<WorkflowBoardPlannedCheck>;
  artifacts?: Record<string, string>;
}

function defaultSixLenses(complete = true): Array<TestLensSeed> {
  return [
    { lens: "ux", applicable: complete ? true : null, summary: complete ? "Split board UX" : "" },
    {
      lens: "functional",
      applicable: complete ? true : null,
      summary: complete ? "Card lifecycle" : "",
    },
    {
      lens: "architecture",
      applicable: complete ? true : null,
      summary: complete ? "SQLite card store" : "",
    },
    {
      lens: "quality",
      applicable: complete ? true : null,
      summary: complete ? "Opaque-box E2E tests" : "",
    },
    {
      lens: "infrastructure",
      applicable: complete ? true : null,
      summary: complete ? "Tempdir isolation" : "",
    },
    {
      lens: "delivery",
      applicable: complete ? true : null,
      summary: complete ? "Gated milestone ship" : "",
    },
  ];
}

function defaultTwoAlternatives(): Array<TestAlternativeSeed> {
  return [
    {
      id: "alt-a",
      title: "Transactional ledger",
      summary: "SQLite ledger with revision checks.",
      tradeoffs: "Strong auditability",
    },
    {
      id: "alt-b",
      title: "Sidecar snapshot",
      summary: "Single row plus journal.",
      tradeoffs: "Simpler queries",
    },
  ];
}

function defaultDisjointTasks(): Array<TestTaskSeed> {
  return [
    {
      taskId: "TSK-0001",
      title: "Core validator",
      dependsOn: [],
      filesAllowed: ["src/validator.ts"],
      filesForbidden: ["src/secrets.ts"],
      testingMethod: "test-first",
    },
    {
      taskId: "TSK-0002",
      title: "RPC endpoint",
      dependsOn: ["TSK-0001"],
      filesAllowed: ["src/rpc.ts"],
      filesForbidden: ["src/validator.ts"],
      testingMethod: "regression-first",
    },
  ];
}

function makeStatefulCard(seed: TestCardSeed): TestCardSeed {
  return {
    brief: `Deliver ${seed.title}`,
    stage: "spec",
    revision: 1,
    sourceType: "idea",
    assistanceMode: "balanced",
    sourceRefs: ["README.md"],
    wireframesIndex: null,
    acceptanceCriteria: [{ id: "AC-01", text: "Validates requirements." }],
    constraints: ["Zero auto-commits"],
    suppliedDecisions: [],
    inferredDecisions: [],
    missingDecisions: [],
    explorationPaused: false,
    decisions: [],
    challengeAutomationFailed: false,
    concerns: [],
    synthesisCount: 0,
    specApproval: null,
    planApproval: null,
    lenses: defaultSixLenses(false),
    alternatives: defaultTwoAlternatives(),
    alternativesRevisionCount: 0,
    selectedAlternativeId: null,
    selectionRationale: null,
    strategy: "outcomes",
    strategyRationale: "",
    granularity: "balanced",
    tasks: defaultDisjointTasks(),
    acMappings: [],
    artifacts: { "spec.md": `# ${seed.title}\nAC-01: Validates requirements.` },
    ...seed,
  };
}

function createStatefulBoardSpecPlanEnv(seedCards: ReadonlyArray<TestCardSeed> = []) {
  const envDisposable = setupBoardSpecPlanApp();
  const cards = new Map<string, TestCardSeed>(
    seedCards.map((seed) => {
      const card = makeStatefulCard(seed);
      return [card.cardId, card];
    }),
  );
  let nextCreatedIdx = 1;

  const toDetail = (card: TestCardSeed): WorkflowCardDetail => {
    const revision = PositiveInt.make(card.revision ?? 1);
    const specDigest = card.specApproval?.specDigest ?? `sha256:spec-${card.cardId}`;
    const planDigest = card.planApproval?.planDigest ?? `sha256:plan-${card.cardId}`;
    const receipt = (
      gate: "spec" | "plan",
      status: "valid" | "stale" | "none" | undefined,
    ): WorkflowBoardApprovalReceipt | null =>
      status === undefined || status === "none"
        ? null
        : {
            id: WorkflowBoardApprovalId.make(`receipt-${gate}-${card.cardId}`),
            gate,
            status,
            revision,
            specDigest,
            artifactDigest: gate === "plan" ? planDigest : specDigest,
            wireframesDigest: null,
            approvedAt: BOARD_AT,
            approvedBy: "board-e2e-operator",
          };
    const specApproval = receipt("spec", card.specApproval?.status);
    const planApproval = receipt("plan", card.planApproval?.status);
    const decisions = (card.decisions ?? []).map((decision) => ({
      ...decision,
      answer: decision.answer ?? null,
      lastAction: decision.lastAction ?? null,
      updatedAt: BOARD_AT,
    }));
    const concerns = (card.concerns ?? []).map((concern) => ({
      ...concern,
      answer: concern.answer ?? null,
      reason: concern.reason ?? null,
    }));
    const detail = makeBoardCardDetail({ cardId: card.cardId, title: card.title });
    return {
      ...detail,
      stage: card.stage ?? "spec",
      revision,
      spec: {
        ...detail.spec,
        brief: card.brief ?? "",
        sourceType: card.sourceType ?? "idea",
        assistanceMode: card.assistanceMode ?? "balanced",
        intakeGoal: card.intakeGoal ?? null,
        acceptanceCriteria: card.acceptanceCriteria ?? [],
        constraints: card.constraints ?? [],
        sourceRefs: card.sourceRefs ?? [],
        wireframesIndex: card.wireframesIndex
          ? WorkflowBoardArtifactKey.make(card.wireframesIndex)
          : null,
        specDigest,
        intakeAnalysis: {
          suppliedDecisions: card.suppliedDecisions ?? [],
          inferredDecisions: card.inferredDecisions ?? [],
          missingDecisions: card.missingDecisions ?? [],
          sourceDigest: null,
        },
      },
      specApproval,
      specApprovalStatus: specApproval?.status ?? null,
      specApprovedRevision: specApproval?.revision ?? null,
      planApproval,
      planApprovalStatus: planApproval?.status ?? null,
      planApprovedRevision: planApproval?.revision ?? null,
      artifactKeys: Object.keys(card.artifacts ?? {}).map((key) =>
        WorkflowBoardArtifactKey.make(key),
      ),
      exploration: {
        roundIndex: NonNegativeInt.make(1),
        pendingRoundPrompt: false,
        paused: card.explorationPaused ?? false,
        unresolvedIds: decisions.filter((d) => d.status === "unresolved").map((d) => d.id),
        decisions,
      },
      challengeReceipt: {
        status:
          card.manualAnswers || card.challengeAutomationFailed
            ? "manual_fallback"
            : card.synthesisCount
              ? "synthesized"
              : concerns.some((c) => c.disposition === "unresolved") ||
                  card.challengeCompleted === false
                ? "in_progress"
                : "complete",
        concerns,
        synthesisCount: NonNegativeInt.make(card.synthesisCount ?? 0),
        specDigest,
        interviewDigest: null,
        manualFallback: card.manualAnswers
          ? {
              acknowledgementToken: "I acknowledge",
              answers: card.manualAnswers,
              recordedAt: BOARD_AT,
            }
          : null,
        updatedAt: BOARD_AT,
      },
      plan: {
        lenses: card.lenses ?? [],
        alternatives: card.alternatives ?? [],
        alternativesRevisionCount: NonNegativeInt.make(card.alternativesRevisionCount ?? 0),
        selectedAlternativeId: card.selectedAlternativeId ?? null,
        selectionRationale: card.selectionRationale ?? null,
        strategy: card.strategy ?? null,
        strategyRationale: card.strategyRationale ?? null,
        strategyMetadata: null,
        granularity: card.granularity ?? null,
        tasks: (card.tasks ?? []).map((task): WorkflowBoardTaskContract => ({
          ...task,
          blockedBy: task.dependsOn,
          filesAllowed: [
            WorkflowBoardArtifactKey.make(task.filesAllowed[0]!),
            ...task.filesAllowed.slice(1).map((path) => WorkflowBoardArtifactKey.make(path)),
          ],
          filesForbidden: task.filesForbidden.map((path) => WorkflowBoardArtifactKey.make(path)),
          testingMethod:
            task.testingMethod === "test-first" || task.testingMethod === "regression-first"
              ? task.testingMethod
              : "existing-checks",
        })),
        checks: card.checks ?? [],
        acMappings: (card.acMappings ?? []).map(
          ({ command, ...mapping }): WorkflowBoardAcEvidenceMapping =>
            command && command[0]
              ? { ...mapping, command: [command[0], ...command.slice(1)] }
              : mapping,
        ),
        planDigest,
        tasksDigest: planApproval?.artifactDigest ?? null,
        updatedAt: BOARD_AT,
      },
    };
  };

  const stub = installWorkflowBoardFixtures([...cards.values()].map(toDetail));
  for (const card of cards.values()) {
    for (const [key, content] of Object.entries(card.artifacts ?? {})) {
      stub.artifacts.set(`${card.cardId}:${key}`, content);
    }
  }
  function persist(card: TestCardSeed) {
    const detail = stub.put(toDetail(card));
    return {
      card: detail,
      cardId: detail.cardId,
      revision: detail.revision,
      specApproval: detail.specApproval,
      specApprovalStatus: detail.specApprovalStatus,
      specDigest: detail.spec.specDigest,
      planApproval: detail.planApproval,
      ...detail.plan,
      roundIndex: detail.exploration?.roundIndex,
      pendingRoundPrompt: detail.exploration?.pendingRoundPrompt,
      paused: detail.exploration?.paused,
      exploration: detail.exploration,
      decisions: detail.exploration?.decisions,
      challengeReceipt: detail.challengeReceipt,
      concerns: detail.challengeReceipt?.concerns,
    };
  }

  stub.on("workflowBoard.createCard", (input) => {
    const payload = input as Partial<TestCardSeed> & { readonly briefPath?: string };
    const cardId = `card-created-${nextCreatedIdx}`;
    nextCreatedIdx += 1;
    const card = makeStatefulCard({
      cardId,
      title: payload.title ?? payload.briefPath ?? `Spec ${cardId}`,
      brief: payload.brief ?? payload.briefPath ?? "Imported brief",
      sourceType: payload.sourceType ?? "idea",
      assistanceMode: payload.assistanceMode ?? "balanced",
      sourceRefs: payload.sourceRefs ?? (input.inputPaths as string[] | undefined) ?? [],
      ...(payload.intakeGoal ? { intakeGoal: payload.intakeGoal } : {}),
      suppliedDecisions: payload.sourceType === "file" ? ["Supplied: SQLite WAL persistence"] : [],
      inferredDecisions: payload.sourceType === "file" ? ["Inferred: Local board directory"] : [],
      missingDecisions: payload.sourceType === "file" ? ["Missing: Wireframe preference"] : [],
      specApproval: null,
    });
    cards.set(cardId, card);
    return persist(card).card;
  });

  stub.on("workflowBoard.updateSpec", (input) => {
    const payload = input as Partial<TestCardSeed> & { readonly cardId: string };
    const card = cards.get(payload.cardId);
    if (!card) throw new Error(`Card ${payload.cardId} not found`);
    if (payload.brief !== undefined) card.brief = payload.brief;
    if (payload.acceptanceCriteria !== undefined) {
      card.acceptanceCriteria = payload.acceptanceCriteria;
    }
    if (payload.constraints !== undefined) card.constraints = payload.constraints;
    if (payload.sourceRefs !== undefined) card.sourceRefs = payload.sourceRefs;
    if (payload.wireframesIndex !== undefined) card.wireframesIndex = payload.wireframesIndex;
    card.revision = (card.revision ?? 1) + 1;
    if (card.specApproval?.status === "valid") {
      card.specApproval = { ...card.specApproval, status: "stale" };
    }
    if (card.planApproval?.status === "valid") {
      card.planApproval = { ...card.planApproval, status: "stale" };
    }
    return persist(card);
  });

  stub.on("workflowBoard.exploreStep", (input) => {
    const payload = input as WorkflowBoardExploreStepInput;
    const card = cards.get(payload.cardId);
    if (!card) throw boardRequestError("card_not_found", `Card ${payload.cardId} not found`);
    if (payload.expectedRevision !== card.revision) {
      throw boardRequestError("revision_conflict", "The decision state changed.");
    }
    if (payload.action === "pause" || payload.action === "resume") {
      card.explorationPaused = payload.action === "pause";
    }
    for (const entry of payload.answers ?? []) {
      const decision = card.decisions?.find((item) => item.id === entry.decisionId);
      const action = entry.action ?? entry.choice;
      if (!decision || !action) throw boardRequestError("invalid_input", "Unknown decision.");
      decision.lastAction = action;
      if (action === "edit") {
        if (!entry.answer?.trim()) {
          throw boardRequestError("invalid_input", "Decision answer cannot be blank.");
        }
        decision.status = "resolved";
        decision.answer = entry.answer;
      }
    }
    card.revision += 1;
    return persist(card);
  });

  stub.on("workflowBoard.challengeStep", (input) => {
    const payload = input as WorkflowBoardChallengeStepInput;
    const card = cards.get(payload.cardId);
    if (!card) throw boardRequestError("card_not_found", `Card ${payload.cardId} not found`);
    if (payload.expectedRevision !== card.revision) {
      throw boardRequestError("revision_conflict", "The challenge changed.");
    }
    if (payload.action === "resolve") {
      for (const response of payload.answers ?? []) {
        if (typeof response === "string")
          throw boardRequestError("invalid_input", "Invalid concern.");
        const concern = card.concerns?.find((item) => item.id === response.concernId);
        if (!concern) throw boardRequestError("invalid_input", "Unknown concern.");
        concern.disposition = response.disposition;
        if (response.response !== undefined) concern.answer = response.response;
        if (response.reason !== undefined) concern.reason = response.reason;
      }
    }
    if (payload.action === "manualFallback") {
      const [a, b, c] = payload.answers ?? [];
      if (
        payload.acknowledgementToken !== "I acknowledge" ||
        typeof a !== "string" ||
        typeof b !== "string" ||
        typeof c !== "string"
      ) {
        throw boardRequestError(
          "invalid_input",
          "Exact acknowledgement and three answers required.",
        );
      }
      card.manualAnswers = [a, b, c];
      card.challengeAutomationFailed = false;
    }
    if (payload.action === "synthesize") {
      if ((card.synthesisCount ?? 0) >= 1) {
        throw boardRequestError("invalid_input", "Maximum one synthesis pass.");
      }
      card.synthesisCount = 1;
    }
    if (payload.action === "complete" || payload.action === "start") {
      card.challengeCompleted = payload.action === "complete";
    }
    card.revision += 1;
    return persist(card);
  });

  stub.on("workflowBoard.approveSpec", (input) => {
    const { cardId } = input as { readonly cardId: string };
    const card = cards.get(cardId);
    if (!card) throw new Error(`Card ${cardId} not found`);
    if ((card.acceptanceCriteria?.length ?? 0) === 0) {
      throw new Error("At least one acceptance criterion is required");
    }
    const unresolved = card.concerns?.find((c) => c.disposition === "unresolved");
    if (unresolved) {
      throw new Error(`Unresolved challenge concern ${unresolved.id}`);
    }
    card.revision = (card.revision ?? 1) + 1;
    card.stage = "plan";
    card.specApproval = { status: "valid", specDigest: `sha256:spec-${card.revision}` };
    return persist(card);
  });

  stub.on("workflowBoard.updatePlan", (input) => {
    const payload = input as Partial<TestCardSeed> & {
      readonly cardId: string;
      readonly reviseAlternativesFeedback?: string;
    };
    const card = cards.get(payload.cardId);
    if (!card) throw new Error(`Card ${payload.cardId} not found`);
    if (payload.lenses !== undefined) card.lenses = payload.lenses;
    if (payload.alternatives !== undefined) card.alternatives = payload.alternatives;
    if (payload.reviseAlternativesFeedback !== undefined) {
      if ((card.alternativesRevisionCount ?? 0) >= 1) {
        throw new Error("Only 1 comparison revision is permitted");
      }
      card.alternativesRevisionCount = 1;
      card.alternatives = (card.alternatives ?? defaultTwoAlternatives()).map((alt) => ({
        ...alt,
        summary: `${alt.summary} (Revised for: ${payload.reviseAlternativesFeedback})`,
      }));
    }
    if (payload.selectedAlternativeId !== undefined) {
      card.selectedAlternativeId = payload.selectedAlternativeId;
    }
    if (payload.selectionRationale !== undefined) {
      card.selectionRationale = payload.selectionRationale;
    }
    if (payload.strategy !== undefined) card.strategy = payload.strategy;
    if (payload.strategyRationale !== undefined) {
      card.strategyRationale = payload.strategyRationale;
    }
    if (payload.granularity !== undefined) card.granularity = payload.granularity;
    if (payload.tasks !== undefined) card.tasks = payload.tasks;
    if (payload.acMappings !== undefined) card.acMappings = payload.acMappings;
    card.checks = (input as WorkflowBoardUpdatePlanInput).checks ?? card.checks ?? [];
    card.revision = (card.revision ?? 1) + 1;
    if (card.planApproval?.status === "valid") {
      card.planApproval = { ...card.planApproval, status: "stale" };
    }
    return persist(card);
  });

  stub.on("workflowBoard.approvePlan", (input) => {
    const { cardId } = input as { readonly cardId: string };
    const card = cards.get(cardId);
    if (!card) throw new Error(`Card ${cardId} not found`);
    const missingLens = card.lenses?.find((l) => l.applicable === null);
    if (missingLens) throw new Error(`Lens ${missingLens.lens} is undecided`);
    if (!card.selectedAlternativeId) throw new Error("Design alternative must be selected");
    if (card.strategy === "custom" && !(card.strategyRationale ?? "").trim()) {
      throw new Error("Custom strategy requires rationale");
    }
    const mappedIds = new Set((card.acMappings ?? []).map((m) => m.criterionId));
    const missingAc = (card.acceptanceCriteria ?? []).find((ac) => !mappedIds.has(ac.id));
    if (missingAc) throw new Error(`Missing evidence mapping for ${missingAc.id}`);
    card.revision = (card.revision ?? 1) + 1;
    card.stage = "execute";
    card.planApproval = { status: "valid", planDigest: `sha256:plan-${card.revision}` };
    return persist(card);
  });

  stub.on("workflowBoard.returnForRevision", (input) => {
    const payload = input as { readonly cardId: string; readonly targetStage: "spec" | "plan" };
    const card = cards.get(payload.cardId);
    if (!card) throw new Error(`Card ${payload.cardId} not found`);
    card.stage = payload.targetStage;
    card.revision = (card.revision ?? 1) + 1;
    if (card.planApproval) {
      card.planApproval = { ...card.planApproval, status: "stale" };
    }
    return persist(card);
  });

  return {
    stub,
    cards,
    [Symbol.dispose]() {
      stub[Symbol.dispose]();
      envDisposable[Symbol.dispose]();
    },
  };
}

async function openBoardAndSelectCard(
  stub: WorkflowBoardFixtureStore,
  options?: { readonly cardTitle?: string; readonly stageTab?: "Spec" | "Plan" },
) {
  const screen = await mountBoardSpecPlanApp("/");
  await using stack = new AsyncDisposableStack();
  stack.use(screen);

  openCommandPalette();
  const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
  await expect.element(paletteInput).toBeVisible();
  await userEvent.fill(paletteInput, "workflow board");
  const option = page.getByRole("option", { name: /workflow board/i });
  await expect.element(option).toBeVisible();
  await userEvent.click(option);

  for (const column of ["Spec", "Plan", "Execute", "Ship"]) {
    await expect.element(page.getByRole("region", { name: column, exact: true })).toBeVisible();
  }

  if (options?.cardTitle) {
    await userEvent.click(
      page.getByRole("region", { name: /^(Spec|Plan|Execute|Ship)$/ }).getByText(options.cardTitle),
    );
    await vi.waitFor(() => {
      expect(stub.calls.some((c) => c.method === "workflowBoard.getCard")).toBe(true);
    });
  }

  if (options?.stageTab) {
    const tab = page
      .getByRole("region", { name: "Card workspace" })
      .getByRole("tab", { name: options.stageTab, exact: true });
    await userEvent.click(tab);
    await expect.element(tab).toHaveAttribute("aria-selected", "true");
  }

  const owned = stack.move();
  return {
    ...screen,
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}

describe("workflowBoardE2eSpecPlan (F4–F12)", () => {
  it("[UNCOND-WP-01] HtmlRenderDocument sets sandbox='allow-scripts allow-forms' and strictly omits allow-same-origin", async () => {
    using blobPage = makeBlobPage("<!doctype html><html><body><p>Wireframe</p></body></html>");
    await using screen = await render(
      <HtmlRenderDocument src={blobPage.url} title="Spec Wireframe Preview" />,
    );
    const iframe = screen.getByTitle("Spec Wireframe Preview");
    await expect.element(iframe).toBeVisible();
    const sandboxAttr = iframe.element().getAttribute("sandbox");
    expect(sandboxAttr).toBe("allow-scripts allow-forms");
    expect(sandboxAttr).not.toContain("allow-same-origin");
  });

  it("[F6-T1-03] syncs wireframe height via postMessage and opens/closes fullscreen HtmlRenderDialog with zoom controls", async () => {
    const html = `<script>parent.postMessage(${JSON.stringify(sizeChangedMsg(1500))}, "*");</script><main>Mockup</main>`;
    using blobPage = makeBlobPage(html);
    resetAssetMock(blobPage.url);
    const onFullscreenChange = vi.fn();

    await using screen = await render(
      <HtmlRenderFrame
        environmentId={EnvironmentId.make("env-wp-e2e")}
        htmlRender={{ attachmentId: "wf-f6-t1-03", title: "Wireframe F6", height: 240 }}
        threadRef={undefined}
        onFullscreenChange={onFullscreenChange}
      />,
    );

    const frameHeight = () =>
      screen.getByTitle("Wireframe F6").element().parentElement?.style.height;
    await vi.waitFor(() => expect(frameHeight()).toBe("1500px"));

    await userEvent.click(screen.getByRole("button", { name: "Open full screen" }));
    await expect.element(page.getByRole("dialog")).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Zoom in" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Zoom out" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Reset zoom" })).toBeVisible();
    expect(onFullscreenChange).toHaveBeenLastCalledWith(true);

    await screen.unmount();
    expect(onFullscreenChange).toHaveBeenLastCalledWith(false);
  });

  it("[F6-T2-01] enforces opaque-origin SecurityError inside wireframe iframe and ignores forged non-allowlisted shortcut postMessage", async () => {
    const mac = isMacPlatform(navigator.platform);
    let opaqueBlocked: boolean | null = null;
    const onOpaqueProbe = (event: MessageEvent) => {
      if (event.data?.kind === "opaque-probe") {
        opaqueBlocked = event.data.blocked === true;
      }
    };
    window.addEventListener("message", onOpaqueProbe);
    using _opaqueCleanup = {
      [Symbol.dispose]: () => window.removeEventListener("message", onOpaqueProbe),
    };

    const probeHtml = `<!doctype html><html><body>
      <button type="button">Focus Frame</button>
      <script>
        let blocked = false;
        try {
          void window.parent.document.title;
        } catch (err) {
          blocked = err && err.name === "SecurityError";
        }
        window.parent.postMessage({ kind: "opaque-probe", blocked }, "*");
        document.querySelector("button").addEventListener("click", () => {
          window.parent.postMessage({
            jsonrpc: "2.0",
            method: "legion/notifications/keydown",
            params: { key: "n", code: "KeyN", metaKey: ${String(mac)}, ctrlKey: ${String(!mac)}, shiftKey: false, altKey: false }
          }, "*");
          window.parent.postMessage({
            jsonrpc: "2.0",
            method: "legion/notifications/keydown",
            params: { key: "k", code: "KeyK", metaKey: ${String(mac)}, ctrlKey: ${String(!mac)}, shiftKey: false, altKey: false }
          }, "*");
          window.parent.postMessage({ kind: "shortcut-probe-done" }, "*");
        });
      </script></body></html>`;
    using blobPage = makeBlobPage(probeHtml);
    resetAssetMock(blobPage.url);

    await using screen = await render(
      <HtmlRenderFrame
        environmentId={EnvironmentId.make("env-wp-e2e")}
        htmlRender={{ attachmentId: "wf-f6-t2-01", title: "Opaque Origin Probe", height: 220 }}
        threadRef={undefined}
      />,
    );

    const dispatchedKeys: Array<string> = [];
    const onKeydown = (event: KeyboardEvent) => {
      dispatchedKeys.push(event.key);
    };
    document.addEventListener("keydown", onKeydown);
    using _kdCleanup = {
      [Symbol.dispose]: () => document.removeEventListener("keydown", onKeydown),
    };

    let shortcutProbeDone = false;
    const onShortcutDone = (event: MessageEvent) => {
      if (event.data?.kind === "shortcut-probe-done") {
        shortcutProbeDone = true;
      }
    };
    window.addEventListener("message", onShortcutDone);
    using _doneCleanup = {
      [Symbol.dispose]: () => window.removeEventListener("message", onShortcutDone),
    };

    await expect.element(screen.getByTitle("Opaque Origin Probe")).toBeVisible();
    await vi.waitFor(() => expect(opaqueBlocked).toBe(true));

    await page
      .frameLocator(screen.getByTitle("Opaque Origin Probe"))
      .getByRole("button", { name: "Focus Frame" })
      .click();

    await vi.waitFor(() => expect(shortcutProbeDone).toBe(true));
    expect(dispatchedKeys).toEqual(["k"]);
  });

  it("[F6-T2-02] renders recoverable fallback when wireframe asset URL fails without crashing Spec workspace", async () => {
    wireframeAssetState.tag = "Failure";
    wireframeAssetState.url = "about:blank";
    wireframeAssetState.refreshUrl = null;

    await using screen = await render(
      <HtmlRenderFrame
        environmentId={EnvironmentId.make("env-wp-e2e")}
        htmlRender={{ attachmentId: "wf-f6-t2-02", title: "Broken Wireframe", height: 200 }}
        threadRef={undefined}
      />,
    );

    await expect.element(screen.getByText("Unable to load Broken Wireframe")).toBeVisible();
  });

  it("[F6-T2-05] ignores link and image postMessage requests from unfocused wireframe iframe", async () => {
    const localApi = ensureLocalApi();
    using openExternalSpy = vi.spyOn(localApi.shell, "openExternal").mockResolvedValue(undefined);
    using windowOpenSpy = vi.spyOn(window, "open").mockImplementation(() => null);

    let probeReady = false;
    const onReady = (event: MessageEvent) => {
      if (event.data?.kind === "unfocused-probe-ready") {
        probeReady = true;
      }
    };
    window.addEventListener("message", onReady);
    using _readyCleanup = {
      [Symbol.dispose]: () => window.removeEventListener("message", onReady),
    };

    const html = `<!doctype html><html><body><script>
      let linkAckedInFrame = false;
      window.addEventListener("message", (event) => {
        if (event.data?.id === "l1" && "result" in event.data) {
          linkAckedInFrame = true;
          return;
        }
        if (event.data?.kind === "collect-unfocused-probe") {
          window.parent.postMessage({ kind: "unfocused-probe-done", linkAcked: linkAckedInFrame }, "*");
          return;
        }
        if (event.data?.kind !== "run-unfocused-probe") return;
        window.parent.postMessage({
          jsonrpc: "2.0",
          id: "l1",
          method: "ui/open-link",
          params: { url: "https://example.com", metaKey: true, ctrlKey: false }
        }, "*");
        window.parent.postMessage({
          jsonrpc: "2.0",
          method: "legion/notifications/open-image",
          params: {
            src: "https://example.com/a.png",
            name: "Mockup A",
            images: [{ src: "https://example.com/a.png", name: "Mockup A" }],
            index: 0
          }
        }, "*");
        window.parent.postMessage({ kind: "unfocused-probe-sent" }, "*");
      });
      window.parent.postMessage({ kind: "unfocused-probe-ready" }, "*");
    </script></body></html>`;
    using blobPage = makeBlobPage(html);
    resetAssetMock(blobPage.url);

    await using screen = await render(
      <div>
        <button type="button">Outside Focus</button>
        <HtmlRenderFrame
          environmentId={EnvironmentId.make("env-wp-e2e")}
          htmlRender={{ attachmentId: "wf-f6-t2-05", title: "Unfocused Frame", height: 200 }}
          threadRef={undefined}
        />
      </div>,
    );

    let probeSent = false;
    let probeDone = false;
    let linkAcked = false;
    const onDone = (event: MessageEvent) => {
      if (event.data?.kind === "unfocused-probe-sent") {
        probeSent = true;
      }
      if (event.data?.kind === "unfocused-probe-done") {
        linkAcked = event.data.linkAcked === true;
        probeDone = true;
      }
    };
    window.addEventListener("message", onDone);
    using _doneCleanup = {
      [Symbol.dispose]: () => window.removeEventListener("message", onDone),
    };

    await vi.waitFor(() => expect(probeReady).toBe(true));
    await userEvent.click(screen.getByRole("button", { name: "Outside Focus" }));

    const frame = screen.getByTitle("Unfocused Frame").element() as HTMLIFrameElement;
    frame.contentWindow?.postMessage({ kind: "run-unfocused-probe" }, "*");
    await vi.waitFor(() => expect(probeSent).toBe(true));
    frame.contentWindow?.postMessage({ kind: "collect-unfocused-probe" }, "*");
    await vi.waitFor(() => expect(probeDone).toBe(true));

    expect(openExternalSpy).not.toHaveBeenCalled();
    expect(windowOpenSpy).not.toHaveBeenCalled();
    expect(linkAcked).toBe(false);
    expect(page.getByRole("dialog").elements()).toHaveLength(0);
  });

  it("[F4-T1-01] creates Spec from idea across guided, balanced, and direct assistance modes", async () => {
    using env = createStatefulBoardSpecPlanEnv();
    await using _screen = await openBoardAndSelectCard(env.stub);

    for (const [modeLabel, modeKey, title] of [
      ["Guided", "guided", "Guided Idea Card"],
      ["Balanced", "balanced", "Balanced Idea Card"],
      ["Direct", "direct", "Direct Idea Card"],
    ] as const) {
      await userEvent.click(page.getByRole("button", { name: "New card" }).first());
      const dialog = page.getByRole("dialog");
      await expect.element(dialog).toBeVisible();
      await userEvent.fill(dialog.getByLabelText(/title/i), title);
      await userEvent.fill(dialog.getByLabelText("Initial spec or idea"), `Brief for ${modeKey}`);
      await dialog.getByRole("combobox", { name: "Assistance mode" }).click();
      await page.getByRole("option", { name: modeLabel }).click();
      await userEvent.click(dialog.getByRole("button", { name: /create|submit/i }));
      await expect.element(page.getByRole("heading", { name: title, exact: true })).toBeVisible();
    }

    const createCalls = env.stub.calls.filter((c) => c.method === "workflowBoard.createCard");
    expect(createCalls).toHaveLength(3);
    expect(
      createCalls.map((c) => (c.input as { readonly assistanceMode?: string }).assistanceMode),
    ).toEqual(["guided", "balanced", "direct"]);
    expect(
      createCalls.every((c) => (c.input as { readonly sourceType?: string }).sourceType === "idea"),
    ).toBe(true);
  });

  it("[F4-T1-02] local .md/.txt brief import separates supplied, inferred, and missing decisions without auto-approving", async () => {
    using env = createStatefulBoardSpecPlanEnv();
    await using _screen = await openBoardAndSelectCard(env.stub);

    await userEvent.click(page.getByRole("button", { name: "New card" }).first());
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("combobox", { name: "Source" }).click();
    await page.getByRole("option", { name: "Import brief file" }).click();
    await dialog.getByLabelText("Title").fill("Imported brief");
    await userEvent.fill(dialog.getByLabelText(/brief path|file path/i), "docs/brief.md");
    await userEvent.click(dialog.getByRole("button", { name: /create|import|submit/i }));

    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.createCard" &&
            (c.input as { readonly sourceType?: string }).sourceType === "file" &&
            (c.input as { readonly briefPath?: string }).briefPath === "docs/brief.md",
        ),
      ).toBe(true);
    });
    await expect
      .element(page.getByText("Supplied: SQLite WAL persistence", { exact: true }))
      .toBeVisible();
    await expect
      .element(page.getByText("Inferred: Local board directory", { exact: true }))
      .toBeVisible();
    await expect
      .element(page.getByText("Missing: Wireframe preference", { exact: true }))
      .toBeVisible();
    expect(page.getByText(/spec approved/i).elements()).toHaveLength(0);
  });

  it("[F4-T1-03] brownfield codebase investigation records bounded goal and source refs without starting execution", async () => {
    using env = createStatefulBoardSpecPlanEnv();
    await using _screen = await openBoardAndSelectCard(env.stub);

    await userEvent.click(page.getByRole("button", { name: "New card" }).first());
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("combobox", { name: "Source" }).click();
    await page.getByRole("option", { name: "Investigate codebase" }).click();
    await dialog.getByLabelText("Title").fill("Brownfield audit");
    await dialog.getByRole("combobox", { name: "Intake goal" }).click();
    await page.getByRole("option", { name: "Audit", exact: true }).click();
    await userEvent.fill(
      dialog.getByLabelText("Initial spec or idea"),
      "Audit launcher worktree lookup",
    );
    await userEvent.fill(
      dialog.getByLabelText("Input reference files (optional, one per line)"),
      "rust/crates/legion-server-runtime/src/workflow_driver.rs",
    );
    await userEvent.click(dialog.getByRole("button", { name: /create|submit/i }));

    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.createCard" &&
            c.input.sourceType === "codebase" &&
            c.input.intakeGoal === "audit",
        ),
      ).toBe(true);
    });
    await expect
      .element(page.getByRole("textbox", { name: "Spec brief (Markdown)" }))
      .toHaveValue("Audit launcher worktree lookup");
    await expect
      .element(page.getByLabelText("Source references (one per line)"))
      .toHaveValue("rust/crates/legion-server-runtime/src/workflow_driver.rs");
    expect(env.stub.calls.some((c) => c.method === "workflowBoard.startStage")).toBe(false);
  });

  it("[F4-T2-04] direct mode still enforces mandatory requirements and blocks Approve Spec with empty ACs", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f4-empty-ac",
        title: "Direct Empty AC Card",
        assistanceMode: "direct",
        acceptanceCriteria: [],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Direct Empty AC Card",
      stageTab: "Spec",
    });

    const approveBtn = page.getByRole("button", { name: "Approve spec", exact: true });
    await expect.element(approveBtn).toBeDisabled();
    expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(false);

    await userEvent.fill(page.getByLabelText(/acceptance criteria/i), "AC-01: Must pass checks");
    await page.getByRole("button", { name: "Save spec", exact: true }).click();
    await expect.element(approveBtn).toBeEnabled();
    await approveBtn.click();
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(true);
    });
  });

  it("[F5-T1-01] guided exploration round renders <=3 dependency-ready decisions with Explain, Recommend, Edit, Unsure, and Pause", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f5-round",
        title: "Exploration Round Card",
        assistanceMode: "guided",
        decisions: [
          { id: "DEC-1", prompt: "DEC-1 Storage mode", status: "unresolved" },
          { id: "DEC-2", prompt: "DEC-2 Concurrency model", status: "unresolved" },
          { id: "DEC-3", prompt: "DEC-3 Audit trail", status: "unresolved" },
          { id: "DEC-4", prompt: "DEC-4 Deferred round 2", status: "unresolved" },
          { id: "DEC-5", prompt: "DEC-5 Deferred round 2", status: "unresolved" },
        ],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Exploration Round Card",
      stageTab: "Spec",
    });

    await expect.element(page.getByText("DEC-1 Storage mode")).toBeVisible();
    await expect.element(page.getByText("DEC-2 Concurrency model")).toBeVisible();
    await expect.element(page.getByText("DEC-3 Audit trail")).toBeVisible();
    expect(page.getByText("DEC-4 Deferred round 2").elements()).toHaveLength(0);
    expect(page.getByText("DEC-5 Deferred round 2").elements()).toHaveLength(0);

    for (const label of ["Explain", "Recommend", "Edit", "Unsure", "Pause"]) {
      await expect
        .element(page.getByRole("button", { name: new RegExp(label, "i") }).first())
        .toBeVisible();
    }
  });

  it("[F5-T1-02] Explain and Recommend inform without resolving decision while Edit resolves with exact answer", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f5-actions",
        title: "Decision Actions Card",
        assistanceMode: "guided",
        decisions: [
          {
            id: "DEC-1",
            prompt: "DEC-1 Journal mode",
            status: "unresolved",
            explanation: "WAL allows concurrent readers.",
            recommendation: "Use SQLite WAL mode",
          },
        ],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Decision Actions Card",
      stageTab: "Spec",
    });

    const exploration = page.getByRole("region", { name: "Guided exploration" });
    for (const [label, action] of [
      ["Explain", "explain"],
      ["Recommend", "recommend"],
    ] as const) {
      await exploration.getByRole("button", { name: label, exact: true }).click();
      await exploration.getByRole("button", { name: "Submit round answers" }).click();
      await vi.waitFor(() => {
        expect(env.cards.get("card-f5-actions")?.decisions?.[0]).toMatchObject({
          lastAction: action,
          status: "unresolved",
        });
      });
      await expect
        .element(exploration.getByRole("button", { name: "Submit round answers" }))
        .toBeEnabled();
    }
    await exploration.getByRole("button", { name: "Edit", exact: true }).click();
    await exploration.getByLabelText("Answer for DEC-1").fill("Use SQLite WAL mode");
    await exploration.getByRole("button", { name: "Submit round answers" }).click();
    await expect.element(exploration.getByText(/Resolved decisions/)).toBeVisible();
    await expect.element(exploration.getByText(/: Use SQLite WAL mode/)).toBeVisible();
    expect(env.cards.get("card-f5-actions")?.decisions?.[0]?.answer).toBe("Use SQLite WAL mode");
  });

  it("[F5-T1-03] Pause preserves partial exploration progress and Resume displays exact remaining unresolved decisions", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f5-pause",
        title: "Pause Resume Card",
        assistanceMode: "guided",
        decisions: [
          { id: "DEC-1", prompt: "DEC-1 Engine", status: "unresolved" },
          { id: "DEC-2", prompt: "DEC-2 Retention", status: "unresolved" },
          { id: "DEC-3", prompt: "DEC-3 Lock policy", status: "unresolved" },
        ],
      },
    ]);

    {
      await using _screen = await openBoardAndSelectCard(env.stub, {
        cardTitle: "Pause Resume Card",
        stageTab: "Spec",
      });
      await page
        .getByRole("region", { name: "Guided exploration" })
        .getByRole("button", { name: "Edit", exact: true })
        .first()
        .click();
      await userEvent.fill(page.getByLabelText("Answer for DEC-1"), "Keep SQLite");
      await userEvent.click(page.getByRole("button", { name: "Submit round answers" }));
      const exploration = page.getByRole("region", { name: "Guided exploration" });
      await expect.element(exploration.getByText(/: Keep SQLite/)).toBeVisible();
      await exploration.getByRole("button", { name: "Pause", exact: true }).click();
      await expect
        .element(exploration.getByRole("button", { name: "Resume exploration" }))
        .toBeEnabled();
      expect(env.cards.get("card-f5-pause")?.explorationPaused).toBe(true);
    }

    await using _reopened = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Pause Resume Card",
      stageTab: "Spec",
    });
    await userEvent.click(page.getByRole("button", { name: "Resume exploration" }));
    await expect.element(page.getByText(/: Keep SQLite/)).toBeVisible();
    await expect.element(page.getByText("DEC-2 Retention")).toBeVisible();
    await expect.element(page.getByText("DEC-3 Lock policy")).toBeVisible();
  });

  it("[F5-T2-03] rejects empty or whitespace Edit answer inline while keeping decision unresolved", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f5-blank",
        title: "Blank Answer Guard Card",
        assistanceMode: "guided",
        decisions: [{ id: "DEC-1", prompt: "DEC-1 Blank check", status: "unresolved" }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Blank Answer Guard Card",
      stageTab: "Spec",
    });

    await page
      .getByRole("region", { name: "Guided exploration" })
      .getByRole("button", { name: "Edit", exact: true })
      .first()
      .click();
    await userEvent.fill(page.getByLabelText("Answer for DEC-1"), "   ");
    await userEvent.click(page.getByRole("button", { name: "Submit round answers" }));

    await expect
      .element(page.getByText("Enter an answer for decision DEC-1 before submitting."))
      .toBeVisible();
    expect(env.stub.callsTo("workflowBoard.exploreStep")).toHaveLength(0);
    expect(env.cards.get("card-f5-blank")?.decisions?.[0]?.status).toBe("unresolved");
  });

  it("[F6-T1-01] opt-in Spec wireframe preview renders HtmlRenderFrame without allow-same-origin", async () => {
    using blobPage = makeBlobPage(
      "<!doctype html><html><body><main>Spec Mockup</main></body></html>",
    );
    resetAssetMock(blobPage.url);
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f6-optin",
        title: "Wireframe Opt-In Card",
        wireframesIndex: "wireframes/index.html",
        artifacts: {
          "spec.md": "# Wireframe Opt-In",
          "wireframes/index.html":
            "<!doctype html><html><body><main>Spec Mockup</main></body></html>",
        },
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Wireframe Opt-In Card",
      stageTab: "Spec",
    });

    const previewToggle = page.getByRole("button", { name: /preview wireframe|show wireframe/i });
    if (previewToggle.elements().length > 0) {
      await userEvent.click(previewToggle.first());
    }
    const iframe = await vi.waitFor(() => {
      const el = document.querySelector("iframe");
      expect(el).not.toBeNull();
      return el!;
    });
    const sandboxAttr = iframe.getAttribute("sandbox");
    expect(sandboxAttr).toContain("allow-scripts");
    expect(sandboxAttr).not.toContain("allow-same-origin");
  });

  it("[F6-T1-02] non-UI feature skips wireframes (wireframesIndex: null) and allows Spec approval cleanly", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f6-nonui",
        title: "Backend Non-UI Card",
        wireframesIndex: null,
        acceptanceCriteria: [{ id: "AC-01", text: "No wireframe required" }],
        concerns: [],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Backend Non-UI Card",
      stageTab: "Spec",
    });

    expect(document.querySelectorAll("iframe")).toHaveLength(0);
    await userEvent.click(page.getByRole("button", { name: "Approve spec", exact: true }));
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(true);
    });
    expect(env.cards.get("card-f6-nonui")?.wireframesIndex).toBeNull();
    await expect
      .element(
        page
          .getByRole("region", { name: "Card workspace" })
          .getByText("Spec approved", { exact: true })
          .first(),
      )
      .toBeVisible();
  });

  it("[F7-T1-01] bounded challenge displays 1-3 concerns with answer, dismiss-with-reason, and accept-risk dispositions", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f7-concerns",
        title: "Bounded Challenge Card",
        concerns: [
          { id: "CON-1", summary: "CON-1 Rollback missing", disposition: "unresolved" },
          { id: "CON-2", summary: "CON-2 Legacy API parity", disposition: "unresolved" },
          { id: "CON-3", summary: "CON-3 Latency budget", disposition: "unresolved" },
        ],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Bounded Challenge Card",
      stageTab: "Spec",
    });

    await expect.element(page.getByText("CON-1 Rollback missing")).toBeVisible();
    await expect.element(page.getByText("CON-2 Legacy API parity")).toBeVisible();
    await expect.element(page.getByText("CON-3 Latency budget")).toBeVisible();

    for (const [id, label] of [
      ["CON-1", "Answer"],
      ["CON-2", "Dismiss"],
      ["CON-3", "Accept risk"],
    ] as const) {
      await page.getByRole("combobox", { name: `Disposition for ${id}` }).click();
      await page.getByRole("option", { name: label, exact: true }).click();
    }
    await page.getByLabelText("Answer for CON-1").fill("Keep pre-edit snapshot");
    await page.getByLabelText("Dismissal reason for CON-2").fill("Out of scope for M2");
    await page.getByRole("button", { name: "Save challenge responses" }).click();
    await vi.waitFor(() => {
      expect(env.cards.get("card-f7-concerns")?.concerns).toMatchObject([
        { disposition: "answer", answer: "Keep pre-edit snapshot" },
        { disposition: "dismiss", reason: "Out of scope for M2" },
        { disposition: "accept-risk" },
      ]);
    });
  });

  it("[F7-T1-02] single synthesis pass preserves verbatim human answer text and shows diff without auto-approving", async () => {
    const verbatim = "Verbatim constraint: preserve SQLite FK.";
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f7-synth",
        title: "Synthesis Pass Card",
        synthesisCount: 0,
        concerns: [
          {
            id: "CON-1",
            summary: "FK constraint",
            disposition: "answer",
            answer: verbatim,
          },
        ],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Synthesis Pass Card",
      stageTab: "Spec",
    });

    const synthesizeBtn = page.getByRole("button", { name: /synthesize/i });
    await userEvent.click(synthesizeBtn);
    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.challengeStep" &&
            (c.input as { readonly action?: string }).action === "synthesize",
        ),
      ).toBe(true);
    });
    await expect.element(page.getByLabelText("Answer for CON-1")).toHaveValue(verbatim);
    await expect
      .element(
        page.getByText("Challenge synthesis complete while preserving verbatim human answers."),
      )
      .toBeVisible();
    await expect.element(synthesizeBtn).toBeDisabled();
    expect(env.cards.get("card-f7-synth")?.specApproval?.status).not.toBe("valid");
  });

  it("[F7-T1-03] zero-concern challenge completes directly and permits Approve Spec", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f7-zero",
        title: "Zero Concern Card",
        challengeCompleted: false,
        concerns: [],
        acceptanceCriteria: [{ id: "AC-01", text: "Direct clean path" }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Zero Concern Card",
      stageTab: "Spec",
    });

    const approve = page.getByRole("button", { name: "Approve spec", exact: true });
    await expect.element(approve).toBeDisabled();
    await page.getByRole("button", { name: "Complete challenge" }).click();
    await expect.element(approve).toBeEnabled();
    await userEvent.click(page.getByRole("button", { name: "Approve spec", exact: true }));
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(true);
    });
    await expect
      .element(
        page
          .getByRole("region", { name: "Card workspace" })
          .getByText("Spec approved", { exact: true })
          .first(),
      )
      .toBeVisible();
  });

  it("[F7-T1-04] challenge automation failure renders 3-question manual fallback gated by exact 'I acknowledge'", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f7-fallback",
        title: "Manual Fallback Card",
        challengeAutomationFailed: true,
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Manual Fallback Card",
      stageTab: "Spec",
    });

    const challenge = page.getByRole("region", { name: "Bounded challenge" });
    await challenge.getByRole("button", { name: "Manual fallback", exact: true }).click();
    await challenge.getByRole("textbox", { name: /^1\./ }).fill("Answer 1");
    await challenge.getByRole("textbox", { name: /^2\./ }).fill("Answer 2");
    await challenge.getByRole("textbox", { name: /^3\./ }).fill("Answer 3");
    const ackInput = challenge.getByRole("textbox", { name: /Type I acknowledge/ });
    const submitBtn = challenge.getByRole("button", { name: "Submit manual fallback" });
    await ackInput.fill("i acknowledge");
    await expect.element(submitBtn).toBeDisabled();
    expect(
      env.stub.calls.some(
        (c) =>
          c.method === "workflowBoard.challengeStep" &&
          (c.input as { readonly action?: string }).action === "manualFallback",
      ),
    ).toBe(false);

    await userEvent.fill(ackInput, "I acknowledge");
    await userEvent.click(submitBtn);
    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.challengeStep" &&
            (c.input as { readonly acknowledgementToken?: string }).acknowledgementToken ===
              "I acknowledge",
        ),
      ).toBe(true);
    });
  });

  it("[F7-T2-05] unresolved challenge concern blocks Approve Spec and highlights remaining concern", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f7-block",
        title: "Unresolved Concern Card",
        concerns: [
          { id: "CON-1", summary: "CON-1 Resolved", disposition: "answer", answer: "Handled" },
          { id: "CON-2", summary: "CON-2 Must Resolve", disposition: "unresolved" },
        ],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Unresolved Concern Card",
      stageTab: "Spec",
    });

    await expect.element(page.getByText("CON-2 Must Resolve")).toBeVisible();
    const approveBtn = page.getByRole("button", { name: "Approve spec", exact: true });
    await expect.element(approveBtn).toBeDisabled();
    expect(env.cards.get("card-f7-block")?.specApproval?.status).not.toBe("valid");
  });

  it("[F8-T1-01] Approve Spec dispatches workflowBoard.approveSpec and renders approved revision and digest badge", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f8-approve",
        title: "Ready Spec Card",
        revision: 4,
        acceptanceCriteria: [{ id: "AC-01", text: "Digest bound" }],
        concerns: [],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Ready Spec Card",
      stageTab: "Spec",
    });

    await userEvent.click(page.getByRole("button", { name: "Approve spec", exact: true }));
    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.approveSpec" &&
            (c.input as { readonly expectedRevision?: number }).expectedRevision === 4,
        ),
      ).toBe(true);
    });
    await expect
      .element(
        page
          .getByRole("region", { name: "Card workspace" })
          .getByText("Spec approved", { exact: true })
          .first(),
      )
      .toBeVisible();
    await expect
      .element(page.getByText("Spec approved at revision 5.", { exact: true }))
      .toBeVisible();
    await page.getByRole("tab", { name: "Spec", exact: true }).click();
    await expect.element(page.getByText(/sha256:spec-5/i)).toBeVisible();
  });

  it("[F8-T2-05] editing an approved Spec warns operator and marks both Spec and downstream Plan badges Stale", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f8-stale",
        title: "Approved Spec & Plan Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-4" },
        planApproval: { status: "valid", planDigest: "sha256:plan-4" },
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Approved Spec & Plan Card",
      stageTab: "Spec",
    });

    await page.getByLabelText("Spec brief (Markdown)").fill("Updated Spec requirement");
    await userEvent.click(page.getByRole("button", { name: /save spec/i }));
    const confirmWarn = page.getByRole("dialog");
    if (confirmWarn.elements().length > 0) {
      await expect.element(confirmWarn).toBeVisible();
      await userEvent.click(confirmWarn.getByRole("button", { name: /confirm|save/i }));
    }

    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.updateSpec")).toBe(true);
    });
    const workspace = page.getByRole("region", { name: "Card workspace" });
    await expect
      .element(workspace.getByText("Spec approval is stale", { exact: true }))
      .toBeVisible();
    await expect
      .element(workspace.getByText("Plan approval is stale", { exact: true }))
      .toBeVisible();
  });

  it("[F9-T1-01] Plan tab renders all 6 design lenses with explicit Applicable/Not-Applicable and rationale controls", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f9-lenses",
        title: "Six Lenses Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Six Lenses Card",
      stageTab: "Plan",
    });

    for (const lensName of [
      "UX",
      "Functional",
      "Architecture",
      "Quality",
      "Infrastructure",
      "Delivery",
    ]) {
      const applicable = page.getByRole("button", { name: `${lensName} applicable`, exact: true });
      const notApplicable = page.getByRole("button", {
        name: `${lensName} not applicable`,
        exact: true,
      });
      await expect.element(applicable).toBeVisible();
      await expect.element(notApplicable).toBeVisible();
      await applicable.click();
      await expect.element(page.getByLabelText(`${lensName} summary`)).toBeVisible();
      await notApplicable.click();
      await expect.element(page.getByLabelText(`${lensName} non-applicable reason`)).toBeVisible();
    }
  });

  it("[F9-T1-02] marking UX lens Not-Applicable with reason passes backend Plan lens check", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f9-ux-na",
        title: "UX Non-Applicable Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "UX Non-Applicable Card",
      stageTab: "Plan",
    });

    await userEvent.click(page.getByRole("button", { name: "UX not applicable", exact: true }));
    await userEvent.fill(page.getByLabelText("UX non-applicable reason"), "Backend-only service");
    await userEvent.click(page.getByRole("button", { name: /save plan|save lenses/i }).first());

    await vi.waitFor(() => {
      expect(
        env.stub.calls.some((c) => {
          if (c.method !== "workflowBoard.updatePlan") return false;
          const lenses = (c.input as { readonly lenses?: ReadonlyArray<TestLensSeed> }).lenses;
          const ux = lenses?.find((l) => l.lens === "ux");
          return ux?.applicable === false && ux.reason === "Backend-only service";
        }),
      ).toBe(true);
    });
    expect(page.getByRole("alert").elements()).toHaveLength(0);
  });

  it("[F9-T1-05] Plan-only lens edit flips Plan approval badge to Stale while keeping Spec Approved badge valid", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f9-plan-only",
        title: "Plan-Only Edit Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-9" },
        planApproval: { status: "valid", planDigest: "sha256:plan-9" },
        lenses: defaultSixLenses(true),
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Plan-Only Edit Card",
      stageTab: "Plan",
    });

    await userEvent.fill(page.getByLabelText("Quality summary"), "Updated Quality lens coverage");
    await userEvent.click(page.getByRole("button", { name: /save plan/i }).first());

    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.updatePlan")).toBe(true);
    });
    expect(env.stub.calls.some((c) => c.method === "workflowBoard.updateSpec")).toBe(false);
    await expect
      .element(
        page
          .getByRole("region", { name: "Card workspace" })
          .getByText("Spec approved", { exact: true })
          .first(),
      )
      .toBeVisible();
    await expect.element(page.getByText("Plan approval is stale", { exact: true })).toBeVisible();
  });

  it("[F9-T2-03] blocks Approve Plan in UI while any of the 6 lenses remains undecided or incomplete", async () => {
    const incompleteLenses = defaultSixLenses(true);
    incompleteLenses[5] = { lens: "delivery", applicable: null };
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f9-incomplete",
        title: "Undecided Delivery Lens Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: incompleteLenses,
        selectedAlternativeId: "alt-a",
        selectionRationale: "Alt A",
        acMappings: [{ criterionId: "AC-01", method: "automated", command: ["pnpm", "test"] }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Undecided Delivery Lens Card",
      stageTab: "Plan",
    });

    const approveBtn = page.getByRole("button", { name: "Approve plan", exact: true });
    await expect.element(approveBtn).toBeDisabled();
    expect(env.cards.get("card-f9-incomplete")?.planApproval?.status).not.toBe("valid");
  });

  it("[F10-T1-01] displays 2 materially different design alternatives and persists selection plus rationale", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f10-alt",
        title: "Design Comparison Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Design Comparison Card",
      stageTab: "Plan",
    });

    await expect
      .element(page.getByLabelText("Alternative 1 title"))
      .toHaveValue("Transactional ledger");
    await expect
      .element(page.getByLabelText("Alternative 2 title"))
      .toHaveValue("Sidecar snapshot");

    await page.getByRole("button", { name: "Select alternative", exact: true }).first().click();
    await page.getByLabelText("Selection rationale").fill("Matches R2 durability");
    await userEvent.click(page.getByRole("button", { name: /save plan|save selection/i }).first());

    await vi.waitFor(() => {
      expect(
        env.stub.calls.some((c) => {
          if (c.method !== "workflowBoard.updatePlan") return false;
          const p = c.input as {
            readonly selectedAlternativeId?: string;
            readonly selectionRationale?: string;
          };
          return (
            p.selectedAlternativeId === "alt-a" && p.selectionRationale === "Matches R2 durability"
          );
        }),
      ).toBe(true);
    });
  });

  it("[F10-T1-02] allows single comparison revision on explicit human feedback before selection", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f10-rev",
        title: "Comparison Revision Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        alternativesRevisionCount: 0,
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Comparison Revision Card",
      stageTab: "Plan",
    });

    await userEvent.fill(page.getByLabelText(/feedback/i), "Clarify Windows locks");
    await page.getByRole("button", { name: "Save plan", exact: true }).click();

    await vi.waitFor(() => {
      expect(
        env.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.updatePlan" &&
            (c.input as { readonly reviseAlternativesFeedback?: string })
              .reviseAlternativesFeedback === "Clarify Windows locks",
        ),
      ).toBe(true);
    });
    await expect
      .element(page.getByLabelText("Alternative 1 summary"))
      .toHaveValue("SQLite ledger with revision checks. (Revised for: Clarify Windows locks)");
    await expect.element(page.getByLabelText(/Revise alternatives feedback/)).toBeDisabled();
  });

  it("[F10-T1-05] selecting a design option with rationale unlocks Plan task generation and Approve Plan progression", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f10-unlock",
        title: "Unlock Plan Progression Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: null,
        acMappings: [{ criterionId: "AC-01", method: "automated", command: ["pnpm", "test"] }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Unlock Plan Progression Card",
      stageTab: "Plan",
    });

    const approveBtn = page.getByRole("button", { name: "Approve plan", exact: true });
    await expect.element(approveBtn).toBeDisabled();

    await userEvent.click(
      page.getByRole("button", { name: "Select alternative", exact: true }).first(),
    );
    await userEvent.fill(page.getByLabelText("Selection rationale"), "SQLite durability");
    await userEvent.click(page.getByRole("button", { name: /save plan|save selection/i }).first());

    await expect.element(approveBtn).toBeEnabled();
  });

  it("[F10-T2-04] blocks Approve Plan in UI and highlights unresolved design comparison when unselected", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f10-unselected",
        title: "Unselected Alternative Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: null,
        acMappings: [{ criterionId: "AC-01", method: "automated", command: ["pnpm", "test"] }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Unselected Alternative Card",
      stageTab: "Plan",
    });

    await expect
      .element(
        page.getByText(
          "Provide 2 distinct alternatives, pick one, and record the selection rationale.",
          { exact: true },
        ),
      )
      .toBeVisible();
    const approveBtn = page.getByRole("button", { name: "Approve plan", exact: true });
    await expect.element(approveBtn).toBeDisabled();
    expect(env.cards.get("card-f10-unselected")?.planApproval?.status).not.toBe("valid");
  });

  it("[F11-T1-01] switches planning strategy and granularity controls and shows mandatory rationale input for custom", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f11-strat",
        title: "Strategy & Granularity Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Alt A",
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Strategy & Granularity Card",
      stageTab: "Plan",
    });

    const strategy = page.getByRole("combobox", { name: "Planning strategy" });
    for (const [label, field] of [
      ["Risk-first", "Key uncertainty"],
      ["Expand-contract", "Compatibility window"],
      ["Custom", "Custom strategy rationale"],
    ] as const) {
      await strategy.click();
      await page.getByRole("option", { name: label, exact: true }).click();
      await expect.element(strategy).toHaveTextContent(label);
      await expect.element(page.getByLabelText(field, { exact: true })).toBeVisible();
    }
    const customRationale = page.getByLabelText("Custom strategy rationale", { exact: true });
    await expect.element(customRationale).toBeVisible();
    await userEvent.fill(customRationale, "Hybrid migration phases");
    await page.getByRole("combobox", { name: "Task granularity" }).click();
    await page.getByRole("option", { name: "Fine", exact: true }).click();
    await expect
      .element(page.getByRole("combobox", { name: "Task granularity" }))
      .toHaveTextContent("Fine");
    await userEvent.click(page.getByRole("button", { name: /save plan/i }).first());

    await vi.waitFor(() => {
      expect(
        env.stub.calls.some((c) => {
          if (c.method !== "workflowBoard.updatePlan") return false;
          const p = c.input as {
            readonly strategy?: string;
            readonly granularity?: string;
            readonly strategyRationale?: string;
          };
          return (
            p.strategy === "custom" &&
            p.granularity === "fine" &&
            p.strategyRationale === "Hybrid migration phases"
          );
        }),
      ).toBe(true);
    });
  });

  it("[F11-T1-03] renders task DAG dependencies, exclusive filesAllowed/filesForbidden badges, and testing method per task", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f11-tasks",
        title: "Task DAG Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        tasks: defaultDisjointTasks(),
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Task DAG Card",
      stageTab: "Plan",
    });

    await expect.element(page.getByLabelText("Task 1 ID")).toHaveValue("TSK-0001");
    await expect.element(page.getByLabelText("Task 2 ID")).toHaveValue("TSK-0002");
    await expect.element(page.getByLabelText("Task 2 blocked by")).toHaveValue("TSK-0001");
    await expect
      .element(page.getByLabelText("Task 1 allowed files"))
      .toHaveValue("src/validator.ts");
    await expect
      .element(page.getByLabelText("Task 1 forbidden files"))
      .toHaveValue("src/secrets.ts");
    await expect
      .element(page.getByRole("combobox", { name: "Task 1 testing method" }))
      .toHaveTextContent("Test-first");
    await expect
      .element(page.getByRole("combobox", { name: "Task 2 testing method" }))
      .toHaveTextContent("Regression-first");
  });

  it("[F11-T2-05] blocks Approve Plan and prompts for rationale when custom strategy rationale is empty", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f11-custom-empty",
        title: "Empty Custom Rationale Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Alt A",
        strategy: "custom",
        strategyRationale: "",
        acMappings: [{ criterionId: "AC-01", method: "automated", command: ["pnpm", "test"] }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Empty Custom Rationale Card",
      stageTab: "Plan",
    });

    const approveBtn = page.getByRole("button", { name: "Approve plan", exact: true });
    await expect.element(approveBtn).toBeDisabled();
    expect(env.cards.get("card-f11-custom-empty")?.planApproval?.status).not.toBe("valid");
  });

  it("[F12-T1-01] maps every AC-* to argv-only check or manual/external procedure and enables Approve Plan at 100% coverage", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f12-map",
        title: "Evidence Mapping Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Alt A",
        acceptanceCriteria: [
          { id: "AC-01", text: "Automated suite passes" },
          { id: "AC-02", text: "Manual badge inspection" },
        ],
        acMappings: [],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Evidence Mapping Card",
      stageTab: "Plan",
    });

    await page.getByRole("button", { name: "Add check" }).click();
    await page.getByLabelText("Check 1 ID").fill("CHK-01");
    await page.getByLabelText("Check 1 argv command").fill("node --test src/validator.test.ts");
    await page.getByRole("combobox", { name: "Evidence method for AC-01" }).click();
    await page.getByRole("option", { name: "Automated", exact: true }).click();
    await page.getByLabelText("Check IDs for AC-01").fill("CHK-01");
    await page.getByRole("combobox", { name: "Evidence method for AC-02" }).click();
    await page.getByRole("option", { name: "Manual", exact: true }).click();
    await page
      .getByLabelText("Manual verification procedure for AC-02")
      .fill("Inspect blocker badge in board view.");
    await userEvent.click(page.getByRole("button", { name: /save plan|save mapping/i }).first());

    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.updatePlan")).toBe(true);
    });
    await expect.element(page.getByText("Mapped 2/2", { exact: true })).toBeVisible();
    await expect
      .element(page.getByRole("button", { name: "Approve plan", exact: true }))
      .toBeEnabled();
  });

  it("[F12-T1-05] Approve Plan renders Plan Approved alongside Spec Approved and unlocks Execute transition", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f12-approve",
        title: "Ready For Execute Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Alt A",
        acceptanceCriteria: [{ id: "AC-01", text: "Checks pass" }],
        acMappings: [{ criterionId: "AC-01", method: "automated", command: ["pnpm", "test"] }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Ready For Execute Card",
      stageTab: "Plan",
    });

    await userEvent.click(page.getByRole("button", { name: "Approve plan", exact: true }));
    await vi.waitFor(() => {
      expect(env.stub.calls.some((c) => c.method === "workflowBoard.approvePlan")).toBe(true);
    });
    await expect
      .element(
        page
          .getByRole("region", { name: "Card workspace" })
          .getByText("Spec approved", { exact: true })
          .first(),
      )
      .toBeVisible();
    await expect.element(page.getByText("Plan approved", { exact: true })).toBeVisible();
    await page.getByRole("tab", { name: "Overview", exact: true }).click();
    await expect
      .element(
        page
          .getByRole("region", { name: "Execute", exact: true })
          .getByText("Ready For Execute Card"),
      )
      .toBeVisible();
  });

  it("[F12-T2-05] unmapped AC-* row is highlighted as missing evidence and blocks Approve Plan in UI", async () => {
    using env = createStatefulBoardSpecPlanEnv([
      {
        cardId: "card-f12-unmapped",
        title: "Missing AC-02 Evidence Card",
        stage: "plan",
        specApproval: { status: "valid", specDigest: "sha256:spec-1" },
        lenses: defaultSixLenses(true),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Alt A",
        acceptanceCriteria: [
          { id: "AC-01", text: "Mapped check" },
          { id: "AC-02", text: "Unmapped check" },
        ],
        acMappings: [{ criterionId: "AC-01", method: "automated", command: ["pnpm", "test"] }],
      },
    ]);
    await using _screen = await openBoardAndSelectCard(env.stub, {
      cardTitle: "Missing AC-02 Evidence Card",
      stageTab: "Plan",
    });

    await expect.element(page.getByText("AC-02", { exact: true })).toBeVisible();
    await expect.element(page.getByText("Unmapped", { exact: true })).toBeVisible();
    await expect
      .element(
        page.getByText(
          "Map 100% of acceptance criteria to automated, manual, or external evidence.",
        ),
      )
      .toBeVisible();
    const approveBtn = page.getByRole("button", { name: "Approve plan", exact: true });
    await expect.element(approveBtn).toBeDisabled();
    expect(env.cards.get("card-f12-unmapped")?.planApproval?.status).not.toBe("valid");
  });
});
