// Real-contract fixtures for the Workflow Board browser E2E suites. Cards are stored as
// `WorkflowCardDetail` and served through the fake environment's RPC table, so the shipped
// board UI under `components/workflow-board/` reads exactly the shapes the server sends.
import {
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProviderInstanceId,
  WorkflowBoardArtifactKey,
  WorkflowBoardRequestError,
  WorkflowCardId,
  WS_METHODS,
  type ProjectWorkflowBoardSnapshot,
  type WorkflowBoardArtifact,
  type WorkflowBoardCardSummary,
  type WorkflowBoardGetCardDiffResult,
  type WorkflowBoardRequestErrorCode,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Effect from "effect/Effect";
import * as Queue from "effect/Queue";
import * as Schema from "effect/Schema";
import * as Stream from "effect/Stream";

import { PROJECT_ID, answerRpc } from "../../../../test/fakeEnvironment";

export const BOARD_AT = IsoDateTime.make("2026-11-24T00:00:00.000Z");
const isWorkflowBoardRequestError = Schema.is(WorkflowBoardRequestError);

export type BoardCardOverrides = Partial<Omit<WorkflowCardDetail, "cardId" | "title">> & {
  readonly cardId: string;
  readonly title: string;
};

export function makeBoardCardDetail(overrides: BoardCardOverrides): WorkflowCardDetail {
  const { cardId, title, spec, ...rest } = overrides;
  return {
    cardId: WorkflowCardId.make(cardId),
    projectId: PROJECT_ID,
    title,
    stage: "spec",
    status: "active",
    revision: PositiveInt.make(1),
    currentTask: null,
    blockerSummary: null,
    nextAction: "draft_spec",
    specApprovedRevision: null,
    specApprovalStatus: null,
    activeThreadId: null,
    createdAt: BOARD_AT,
    updatedAt: BOARD_AT,
    workspaceStrategy: { type: "root" },
    modelSelection: { instanceId: ProviderInstanceId.make("codex"), model: "gpt-5.4" },
    runtimeMode: "full-access",
    interactionMode: "default",
    specApproval: null,
    stageRuns: [],
    revisions: [],
    artifactKeys: [WorkflowBoardArtifactKey.make("spec.md")],
    ...rest,
    spec: {
      title,
      brief: `${title} brief`,
      sourceType: "idea",
      assistanceMode: "balanced",
      // oxlint-disable-next-line legioncode/no-untranslated-copy -- Test fixture data comes from the simulated server.
      acceptanceCriteria: [{ id: "AC-01", text: "Board detail loads cleanly." }],
      constraints: [],
      sourceRefs: [],
      wireframesIndex: null,
      specDigest: `sha256:spec-${cardId}`,
      updatedAt: BOARD_AT,
      ...spec,
    },
  };
}

function toBoardCardSummary(card: WorkflowCardDetail): WorkflowBoardCardSummary {
  return {
    cardId: card.cardId,
    projectId: card.projectId,
    title: card.title,
    stage: card.stage,
    status: card.status,
    revision: card.revision,
    currentTask: card.currentTask,
    blockerSummary: card.blockerSummary,
    nextAction: card.nextAction,
    specApprovedRevision: card.specApprovedRevision,
    specApprovalStatus: card.specApprovalStatus,
    activeThreadId: card.activeThreadId,
    createdAt: card.createdAt,
    updatedAt: card.updatedAt,
    ...(card.planApprovedRevision !== undefined
      ? { planApprovedRevision: card.planApprovedRevision }
      : {}),
    ...(card.planApprovalStatus !== undefined
      ? { planApprovalStatus: card.planApprovalStatus }
      : {}),
    ...(card.incompleteJournalCount !== undefined
      ? { incompleteJournalCount: card.incompleteJournalCount }
      : {}),
    ...(card.hasUnreconciledEdits !== undefined
      ? { hasUnreconciledEdits: card.hasUnreconciledEdits }
      : {}),
    ...(card.deliveryStatus !== undefined ? { deliveryStatus: card.deliveryStatus } : {}),
  };
}

export function boardRequestError(
  code: WorkflowBoardRequestErrorCode,
  message: string,
): WorkflowBoardRequestError {
  return new WorkflowBoardRequestError({ code, message });
}

export interface BoardRpcCall {
  readonly method: string;
  readonly input: Record<string, unknown>;
}

/** A mutation handler: return the RPC success value, or throw a `WorkflowBoardRequestError`. */
export type BoardMutationHandler = (
  input: Record<string, unknown>,
  store: WorkflowBoardFixtureStore,
) => unknown | Promise<unknown>;

export interface WorkflowBoardFixtureStore {
  readonly calls: Array<BoardRpcCall>;
  readonly cards: Map<string, WorkflowCardDetail>;
  get(cardId: string): WorkflowCardDetail;
  /** Replaces the stored card (bumping `updatedAt`) and pushes a fresh board snapshot. */
  put(card: WorkflowCardDetail): WorkflowCardDetail;
  /** Applies a patch on top of the stored card with `revision + 1`, then pushes a snapshot. */
  advance(cardId: string, patch?: Partial<WorkflowCardDetail>): WorkflowCardDetail;
  on(method: string, handler: BoardMutationHandler): void;
  /** Holds the next call to `method` until the returned gate is resolved or rejected. */
  hold(method: string): {
    resolve(value?: unknown): void;
    reject(error: unknown): void;
  };
  artifacts: Map<string, string>;
  callsTo(method: string): ReadonlyArray<Record<string, unknown>>;
  pushSnapshot(): void;
}

const MUTATION_METHODS = [
  WS_METHODS.workflowBoardCreateCard,
  WS_METHODS.workflowBoardUpdateSpec,
  WS_METHODS.workflowBoardApproveSpec,
  WS_METHODS.workflowBoardExploreStep,
  WS_METHODS.workflowBoardChallengeStep,
  WS_METHODS.workflowBoardUpdatePlan,
  WS_METHODS.workflowBoardApprovePlan,
  WS_METHODS.workflowBoardReturnForRevision,
  WS_METHODS.workflowBoardStartStage,
  WS_METHODS.workflowBoardPauseCard,
  WS_METHODS.workflowBoardResumeCard,
  WS_METHODS.workflowBoardRetryStep,
  WS_METHODS.workflowBoardProposeAmendment,
  WS_METHODS.workflowBoardResolveFinding,
  WS_METHODS.workflowBoardRecordManualAcceptance,
  WS_METHODS.workflowBoardApproveDelivery,
  WS_METHODS.workflowBoardPreviewCliImport,
  WS_METHODS.workflowBoardApplyCliImport,
] as const;

/**
 * Serves `workflowBoard.*` from an in-memory card store until disposed. Reads are answered from
 * the store. Each mutation requires an explicit handler for its returned state.
 */
export function installWorkflowBoardFixtures(
  initialCards: ReadonlyArray<WorkflowCardDetail>,
  options?: { readonly initialSnapshotRevision?: number },
): WorkflowBoardFixtureStore & Disposable {
  const calls: Array<BoardRpcCall> = [];
  const cards = new Map<string, WorkflowCardDetail>(initialCards.map((c) => [c.cardId, c]));
  const artifacts = new Map<string, string>();
  const handlers = new Map<string, BoardMutationHandler>();
  const holds = new Map<string, Array<PromiseWithResolvers<unknown>>>();
  const subscribers = new Set<Queue.Queue<ProjectWorkflowBoardSnapshot>>();
  let snapshotRevision = options?.initialSnapshotRevision ?? 1;
  let clock = 1;

  const snapshot = (): ProjectWorkflowBoardSnapshot => ({
    projectId: PROJECT_ID,
    revision: NonNegativeInt.make(snapshotRevision),
    cards: [...cards.values()].map(toBoardCardSummary),
    generatedAt: BOARD_AT,
  });

  const store: WorkflowBoardFixtureStore = {
    calls,
    cards,
    artifacts,
    get(cardId) {
      const card = cards.get(cardId);
      if (!card) throw boardRequestError("card_not_found", `Card ${cardId} not found`);
      return card;
    },
    put(card) {
      clock += 1;
      const next = {
        ...card,
        updatedAt: IsoDateTime.make(new Date(Date.parse(BOARD_AT) + clock * 1000).toISOString()),
      };
      cards.set(next.cardId, next);
      store.pushSnapshot();
      return next;
    },
    advance(cardId, patch = {}) {
      const current = store.get(cardId);
      return store.put({
        ...current,
        ...patch,
        revision: PositiveInt.make(current.revision + 1),
      });
    },
    on(method, handler) {
      handlers.set(method, handler);
    },
    hold(method) {
      const gate = Promise.withResolvers<unknown>();
      const queue = holds.get(method) ?? [];
      queue.push(gate);
      holds.set(method, queue);
      return { resolve: (value) => gate.resolve(value), reject: (error) => gate.reject(error) };
    },
    callsTo(method) {
      return calls.filter((call) => call.method === method).map((call) => call.input);
    },
    pushSnapshot() {
      snapshotRevision += 1;
      const next = snapshot();
      for (const queue of subscribers) Queue.offerUnsafe(queue, next);
    },
  };

  const run = (method: string, input: Record<string, unknown>, fallback: () => unknown) =>
    Effect.tryPromise({
      try: async () => {
        calls.push({ method, input });
        const gate = holds.get(method)?.shift();
        if (gate) await gate.promise;
        const handler = handlers.get(method);
        return handler ? await handler(input, store) : fallback();
      },
      catch: (error) => {
        if (isWorkflowBoardRequestError(error)) return error;
        throw error;
      },
    });

  const disposables: Array<Disposable> = [
    answerRpc(WS_METHODS.workflowBoardSubscribe, (input: Record<string, unknown>) => {
      calls.push({ method: WS_METHODS.workflowBoardSubscribe, input });
      return Stream.unwrap(
        Effect.gen(function* () {
          const queue = yield* Queue.unbounded<ProjectWorkflowBoardSnapshot>();
          subscribers.add(queue);
          Queue.offerUnsafe(queue, snapshot());
          yield* Effect.addFinalizer(() => Effect.sync(() => subscribers.delete(queue)));
          return Stream.fromQueue(queue);
        }),
      );
    }),
    answerRpc(WS_METHODS.workflowBoardGetCard, (input: Record<string, unknown>) =>
      run(WS_METHODS.workflowBoardGetCard, input, () => store.get(String(input.cardId))),
    ),
    answerRpc(WS_METHODS.workflowBoardGetArtifact, (input: Record<string, unknown>) =>
      run(WS_METHODS.workflowBoardGetArtifact, input, (): WorkflowBoardArtifact => {
        const card = store.get(String(input.cardId));
        const key = String(input.key ?? `${String(input.kind ?? "spec")}.md`);
        const content = artifacts.get(`${card.cardId}:${key}`);
        if (content === undefined) {
          throw boardRequestError("artifact_not_found", `Artifact ${key} not found`);
        }
        return {
          cardId: card.cardId,
          key: WorkflowBoardArtifactKey.make(key),
          kind:
            (input.kind as WorkflowBoardArtifact["kind"] | undefined) ??
            (key === "plan-brief.md" || key === "plan.md" ? "plan" : "spec"),
          relativePath: WorkflowBoardArtifactKey.make(key),
          digest: `sha256:${card.cardId}-${key}-${card.revision}`,
          content,
          revision: card.revision,
          updatedAt: card.updatedAt,
        };
      }),
    ),
    answerRpc(WS_METHODS.workflowBoardGetCardDiff, (input: Record<string, unknown>) =>
      run(WS_METHODS.workflowBoardGetCardDiff, input, (): WorkflowBoardGetCardDiffResult => {
        const card = store.get(String(input.cardId));
        return {
          cardId: card.cardId,
          worktreePath: null,
          baseRef: null,
          productSnapshotDigest: null,
          diffDigest: null,
          files: [],
          truncated: false,
          generatedAt: BOARD_AT,
        };
      }),
    ),
    ...MUTATION_METHODS.map((method) =>
      answerRpc(method, (input: Record<string, unknown>) =>
        run(method, input, () => {
          throw new Error(`Missing workflow board fixture handler for ${method}`);
        }),
      ),
    ),
  ];

  return {
    ...store,
    [Symbol.dispose]() {
      for (let i = disposables.length - 1; i >= 0; i -= 1) disposables[i]?.[Symbol.dispose]();
    },
  };
}
