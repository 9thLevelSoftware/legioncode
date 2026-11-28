import { describe, expect, it } from "@effect/vitest";
import {
  AuthOrchestrationOperateScope,
  AuthOrchestrationReadScope,
  type AuthEnvironmentScope,
  type AuthSessionState,
  EnvironmentId,
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProjectId,
  ProviderInstanceId,
  RunId,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WorkflowBoardRequestError,
  WorkflowBoardStageRunId,
  WorkflowCardId,
  WS_METHODS,
  type ProjectWorkflowBoardSnapshot,
  type WorkflowBoardArtifact,
  type WorkflowBoardGetCardDiffResult,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Deferred from "effect/Deferred";
import * as Effect from "effect/Effect";
import * as Fiber from "effect/Fiber";
import * as Layer from "effect/Layer";
import * as Option from "effect/Option";
import * as Stream from "effect/Stream";
import * as SubscriptionRef from "effect/SubscriptionRef";
import { AsyncResult, Atom, AtomRegistry } from "effect/reactivity";
import { vi } from "vite-plus/test";

import {
  AVAILABLE_CONNECTION_STATE,
  PrimaryConnectionTarget,
  type SupervisorConnectionState,
} from "../connection/model.ts";
import { EnvironmentRegistry } from "../connection/registry.ts";
import { EnvironmentSupervisor } from "../connection/supervisor.ts";
import type { RpcSession } from "../rpc/session.ts";
import { runAtomCommand } from "./runtime.ts";
import { createWorkflowBoardEnvironmentAtoms } from "./workflowBoard.ts";

vi.mock("./session.ts", () => ({
  createEnvironmentSessionAtoms: () => ({ sessionStateAtom: sessions }),
}));

const sessions = Atom.family((_id: EnvironmentId) =>
  Atom.make<AsyncResult.AsyncResult<AuthSessionState, string>>(AsyncResult.initial()),
);

const ENV_ID = EnvironmentId.make("env-board");
const PROJECT_ID = ProjectId.make("proj-1");
const CARD_ID = WorkflowCardId.make("card-1");

function authState(scopes: ReadonlyArray<AuthEnvironmentScope>): AuthSessionState {
  return {
    authenticated: true,
    auth: {
      policy: "remote-reachable",
      bootstrapMethods: [],
      sessionMethods: [],
      sessionCookieName: "test",
    },
    scopes: [...scopes],
    permissions: [...scopes],
  };
}

function makeCardDetail(
  revision: number,
  specDigest: string | null = "sha256:abc",
): WorkflowCardDetail {
  return {
    cardId: CARD_ID,
    projectId: PROJECT_ID,
    title: `Feature rev ${revision}`,
    stage: revision >= 3 ? "plan" : "spec",
    status: "active",
    revision: PositiveInt.make(revision),
    currentTask: null,
    blockerSummary: null,
    nextAction: revision >= 3 ? "start_plan" : "approve_spec",
    specApprovedRevision: revision >= 3 ? PositiveInt.make(revision) : null,
    specApprovalStatus: revision >= 3 ? "valid" : null,
    activeThreadId: revision >= 4 ? ThreadId.make("thread-plan") : null,
    createdAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
    updatedAt: IsoDateTime.make(`2026-11-24T01:0${revision}:00.000Z`),
    workspaceStrategy: { type: "root" },
    modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
    runtimeMode: "full-access",
    interactionMode: "default",
    spec: {
      title: `Feature rev ${revision}`,
      brief: "Build board",
      sourceType: "idea",
      assistanceMode: "balanced",
      acceptanceCriteria: [],
      constraints: [],
      sourceRefs: [],
      wireframesIndex: null,
      specDigest,
      updatedAt: IsoDateTime.make(`2026-11-24T01:0${revision}:00.000Z`),
    },
    specApproval:
      revision >= 3
        ? {
            id: WorkflowBoardApprovalId.make("appr-1"),
            gate: "spec",
            status: "valid",
            revision: PositiveInt.make(revision),
            artifactDigest: specDigest ?? "sha256:abc",
            specDigest,
            wireframesDigest: null,
            approvedAt: IsoDateTime.make("2026-11-24T01:03:00.000Z"),
            approvedBy: "client",
          }
        : null,
    stageRuns: [],
    revisions: [],
    artifactKeys: [
      WorkflowBoardArtifactKey.make("spec.md"),
      ...(revision >= 4 ? [WorkflowBoardArtifactKey.make("plan-brief.md")] : []),
    ],
  };
}

function makeSnapshot(revision: number): ProjectWorkflowBoardSnapshot {
  const detail = makeCardDetail(revision);
  return {
    projectId: PROJECT_ID,
    revision: NonNegativeInt.make(revision),
    cards: [
      {
        cardId: detail.cardId,
        projectId: detail.projectId,
        title: detail.title,
        stage: detail.stage,
        status: detail.status,
        revision: detail.revision,
        currentTask: detail.currentTask,
        blockerSummary: detail.blockerSummary,
        nextAction: detail.nextAction,
        specApprovedRevision: detail.specApprovedRevision,
        specApprovalStatus: detail.specApprovalStatus,
        activeThreadId: detail.activeThreadId,
        createdAt: detail.createdAt,
        updatedAt: detail.updatedAt,
      },
    ],
    generatedAt: detail.updatedAt,
  };
}

function makeArtifact(revision: number, content: string): WorkflowBoardArtifact {
  return {
    cardId: CARD_ID,
    key: WorkflowBoardArtifactKey.make("plan-brief.md"),
    kind: "plan",
    relativePath: WorkflowBoardArtifactKey.make("plan-brief.md"),
    digest: `sha256:art-${revision}`,
    content,
    revision: PositiveInt.make(revision),
    updatedAt: IsoDateTime.make(`2026-11-24T01:0${revision}:00.000Z`),
  };
}

function waitForAtomSuccess<A, E>(
  registry: AtomRegistry.AtomRegistry,
  atom: Atom.Atom<AsyncResult.AsyncResult<A, E>>,
  predicate: (value: A) => boolean,
): Promise<A> {
  const current = registry.get(atom);
  if (AsyncResult.isSuccess(current) && predicate(current.value)) {
    return Promise.resolve(current.value);
  }
  return new Promise<A>((resolve) => {
    const unsub = registry.subscribe(atom, (res) => {
      if (AsyncResult.isSuccess(res) && predicate(res.value)) {
        unsub();
        resolve(res.value);
      }
    });
  });
}

const makeHarness = Effect.fn("WorkflowBoardAtoms.makeHarness")(function* (
  client: Record<string, (...args: never[]) => unknown>,
) {
  const connected: SupervisorConnectionState = {
    ...AVAILABLE_CONNECTION_STATE,
    desired: true,
    network: "online",
    phase: "connected",
    attempt: 1,
    generation: 1,
  };
  const supervisorState = yield* SubscriptionRef.make(connected);
  const supervisorSession = yield* SubscriptionRef.make(
    Option.some({ client } as unknown as RpcSession),
  );
  const supervisor = EnvironmentSupervisor.of({
    target: new PrimaryConnectionTarget({
      environmentId: ENV_ID,
      label: "Local",
      httpBaseUrl: "http://127.0.0.1:3773",
      wsBaseUrl: "ws://127.0.0.1:3773",
    }),
    state: supervisorState,
    session: supervisorSession,
    prepared: yield* SubscriptionRef.make(Option.none()),
    connect: Effect.void,
    disconnect: Effect.void,
    retryNow: Effect.void,
  } as unknown as EnvironmentSupervisor["Service"]);

  const registryService = EnvironmentRegistry.of({
    run: (_id: EnvironmentId, effect: Effect.Effect<unknown>) =>
      Effect.provideService(effect, EnvironmentSupervisor, supervisor),
    followStream: (_id: EnvironmentId, stream: Stream.Stream<unknown>) =>
      Stream.provideService(stream, EnvironmentSupervisor, supervisor),
    stateChanges: () => SubscriptionRef.changes(supervisorState),
  } as unknown as EnvironmentRegistry["Service"]);

  const runtime = Atom.runtime(Layer.succeed(EnvironmentRegistry, registryService));
  const atoms = createWorkflowBoardEnvironmentAtoms(runtime);
  const registry = AtomRegistry.make();
  registry.mount(sessions(ENV_ID));
  yield* Effect.addFinalizer(() => Effect.sync(() => registry.dispose()));
  return { atoms, registry };
});

describe("workflowBoard state atoms", () => {
  it.effect("enforces read permissions on cardDetail & artifact and retries when granted", () =>
    Effect.gen(function* () {
      let getCardCalls = 0;
      let getArtifactCalls = 0;
      const { atoms, registry } = yield* makeHarness({
        [WS_METHODS.workflowBoardGetCard]: () =>
          Effect.sync(() => {
            getCardCalls += 1;
            return makeCardDetail(1);
          }),
        [WS_METHODS.workflowBoardGetArtifact]: () =>
          Effect.sync(() => {
            getArtifactCalls += 1;
            return makeArtifact(1, "# Brief");
          }),
      });

      registry.set(sessions(ENV_ID), AsyncResult.success(authState([])));
      const cardAtom = atoms.cardDetail({ environmentId: ENV_ID, input: { cardId: CARD_ID } });
      const artifactAtom = atoms.artifact({
        environmentId: ENV_ID,
        input: { cardId: CARD_ID, key: WorkflowBoardArtifactKey.make("plan-brief.md") },
      });
      const unmountCard = registry.mount(cardAtom);
      const unmountArtifact = registry.mount(artifactAtom);
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          unmountCard();
          unmountArtifact();
        }),
      );

      const deniedCard = yield* AtomRegistry.getResult(registry, cardAtom, {
        suspendOnWaiting: true,
      }).pipe(Effect.flip);
      const deniedArtifact = yield* AtomRegistry.getResult(registry, artifactAtom, {
        suspendOnWaiting: true,
      }).pipe(Effect.flip);
      expect(deniedCard).toMatchObject({ _tag: "EnvironmentAuthorizationError" });
      expect(deniedArtifact).toMatchObject({ _tag: "EnvironmentAuthorizationError" });
      expect(getCardCalls).toBe(0);
      expect(getArtifactCalls).toBe(0);
      expect(registry.get(atoms.readPermissionAtom(ENV_ID))).toBe(false);

      const cardWaiter = waitForAtomSuccess(registry, cardAtom, (card) => card.revision === 1);
      const artifactWaiter = waitForAtomSuccess(
        registry,
        artifactAtom,
        (artifact) => artifact.revision === 1,
      );

      registry.set(sessions(ENV_ID), AsyncResult.success(authState([AuthOrchestrationReadScope])));

      expect((yield* Effect.promise(() => cardWaiter)).revision).toBe(1);
      expect((yield* Effect.promise(() => artifactWaiter)).content).toBe("# Brief");
      expect(getCardCalls).toBe(1);
      expect(getArtifactCalls).toBe(1);
    }),
  );

  it.effect(
    "guards operate mutations and refreshes card/artifact on mutations, conflicts, and snapshots",
    () =>
      Effect.gen(function* () {
        let currentRevision = 1;
        let startStageCalls = 0;
        const startEntered = yield* Deferred.make<void>();
        const startGate = yield* Deferred.make<void>();
        const snapshotRef = yield* SubscriptionRef.make(makeSnapshot(1));

        const { atoms, registry } = yield* makeHarness({
          [WS_METHODS.workflowBoardSubscribe]: () => SubscriptionRef.changes(snapshotRef),
          [WS_METHODS.workflowBoardGetCard]: () =>
            Effect.sync(() => makeCardDetail(currentRevision)),
          [WS_METHODS.workflowBoardGetArtifact]: () =>
            Effect.sync(() => makeArtifact(currentRevision, `brief rev ${currentRevision}`)),
          [WS_METHODS.workflowBoardUpdateSpec]: () =>
            Effect.sync(() => {
              currentRevision = 2;
              const card = makeCardDetail(2, "sha256:rev2");
              return {
                cardId: CARD_ID,
                revision: card.revision,
                specDigest: card.spec.specDigest,
                specApprovalStatus: null,
                card,
              };
            }),
          [WS_METHODS.workflowBoardApproveSpec]: () =>
            Effect.sync(() => {
              currentRevision = 3;
              const card = makeCardDetail(3, "sha256:rev2");
              return {
                cardId: CARD_ID,
                revision: card.revision,
                specApproval: card.specApproval!,
                card,
              };
            }),
          [WS_METHODS.workflowBoardStartStage]: () =>
            Effect.gen(function* () {
              startStageCalls += 1;
              yield* Deferred.succeed(startEntered, undefined);
              yield* Deferred.await(startGate);
              currentRevision = 4;
              return {
                cardId: CARD_ID,
                revision: PositiveInt.make(4),
                stage: "plan" as const,
                stageRunId: WorkflowBoardStageRunId.make("sr-1"),
                runId: RunId.make("run-1"),
                threadId: ThreadId.make("thread-plan"),
                resumed: false,
              };
            }),
        });

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(authState([AuthOrchestrationReadScope])),
        );
        expect(registry.get(atoms.updateSpec.permissionAtom(ENV_ID))).toBe(false);
        const deniedUpdate = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.updateSpec, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(1),
              title: "Updated",
            },
          }),
        );
        expect(deniedUpdate._tag).toBe("Failure");

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(
            authState([AuthOrchestrationReadScope, AuthOrchestrationOperateScope]),
          ),
        );
        expect(registry.get(atoms.startStage.permissionAtom(ENV_ID))).toBe(true);

        const snapshotAtom = atoms.boardSnapshot({
          environmentId: ENV_ID,
          input: { projectId: PROJECT_ID },
        });
        const cardAtom = atoms.cardDetail({
          environmentId: ENV_ID,
          input: { cardId: CARD_ID, projectId: PROJECT_ID },
        });
        const artifactAtom = atoms.artifact({
          environmentId: ENV_ID,
          input: { cardId: CARD_ID, key: WorkflowBoardArtifactKey.make("plan-brief.md") },
        });
        const u1 = registry.mount(snapshotAtom);
        const u2 = registry.mount(cardAtom);
        const u3 = registry.mount(artifactAtom);
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            u1();
            u2();
            u3();
          }),
        );

        expect(
          (yield* AtomRegistry.getResult(registry, cardAtom, { suspendOnWaiting: true })).revision,
        ).toBe(1);

        const waitRev = (expected: number) =>
          Promise.all([
            waitForAtomSuccess(registry, cardAtom, (card) => card.revision === expected),
            waitForAtomSuccess(
              registry,
              artifactAtom,
              (artifact) => artifact.revision === expected,
            ),
          ]);

        const rev2Waiter = waitRev(2);
        const upd = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.updateSpec, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(1), title: "Rev 2" },
          }),
        );
        expect(upd._tag).toBe("Success");
        yield* Effect.promise(() => rev2Waiter);

        const rev3Waiter = waitRev(3);
        const appr = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.approveSpec, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(2) },
          }),
        );
        expect(appr._tag).toBe("Success");
        yield* Effect.promise(() => rev3Waiter);

        const rev4Waiter = waitRev(4);
        const f1 = yield* Effect.forkChild(
          atoms.startStage.run(registry, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(3),
              stage: "plan",
            },
          }),
          { startImmediately: true },
        );
        yield* Deferred.await(startEntered);
        const f2 = yield* Effect.forkChild(
          atoms.startStage.run(registry, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(3),
              stage: "plan",
            },
          }),
          { startImmediately: true },
        );
        yield* Deferred.succeed(startGate, undefined);
        const [r1, r2] = yield* Effect.all([Fiber.join(f1), Fiber.join(f2)]);
        expect(r1.threadId).toBe("thread-plan");
        expect(r2.threadId).toBe("thread-plan");
        expect(startStageCalls).toBe(1);
        yield* Effect.promise(() => rev4Waiter);

        currentRevision = 5;
        const rev5Waiter = waitRev(5);
        yield* SubscriptionRef.set(snapshotRef, makeSnapshot(5));
        yield* Effect.promise(() => rev5Waiter);
      }),
  );

  it.effect("refreshes cardDetail when updateSpec hits revision_conflict", () =>
    Effect.gen(function* () {
      let currentRevision = 1;
      const { atoms, registry } = yield* makeHarness({
        [WS_METHODS.workflowBoardGetCard]: () => Effect.sync(() => makeCardDetail(currentRevision)),
        [WS_METHODS.workflowBoardUpdateSpec]: () =>
          Effect.fail(
            new WorkflowBoardRequestError({
              code: "revision_conflict",
              message: "Conflict",
              currentRevision: PositiveInt.make(9),
            }),
          ),
      });

      registry.set(
        sessions(ENV_ID),
        AsyncResult.success(authState([AuthOrchestrationReadScope, AuthOrchestrationOperateScope])),
      );

      const cardAtom = atoms.cardDetail({ environmentId: ENV_ID, input: { cardId: CARD_ID } });
      const unmount = registry.mount(cardAtom);
      yield* Effect.addFinalizer(() => Effect.sync(unmount));

      expect(
        (yield* AtomRegistry.getResult(registry, cardAtom, { suspendOnWaiting: true })).revision,
      ).toBe(1);

      currentRevision = 9;
      const refreshed = waitForAtomSuccess(registry, cardAtom, (card) => card.revision === 9);

      const conflict = yield* Effect.promise(() =>
        runAtomCommand(registry, atoms.updateSpec, {
          environmentId: ENV_ID,
          input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(1), title: "Stale" },
        }),
      );
      expect(conflict._tag).toBe("Failure");
      expect((yield* Effect.promise(() => refreshed)).revision).toBe(9);
    }),
  );

  it.effect(
    "runs M2 planning commands with permissionAtom guards, refreshes cardDetail/artifact, refreshes on external_edit_detected/digest_mismatch, and serializes per card",
    () =>
      Effect.gen(function* () {
        let currentRevision = 1;
        let currentPlanDigest: string | null = null;
        let currentWireframesDigest: string | null = null;
        const callOrder: string[] = [];
        const updateEntered = yield* Deferred.make<void>();
        const updateGate = yield* Deferred.make<void>();

        const makeM2Card = (revision: number): WorkflowCardDetail => {
          const base = makeCardDetail(revision, "sha256:spec-m2");
          return {
            ...base,
            wireframesDigest: currentWireframesDigest,
            plan:
              currentPlanDigest !== null
                ? {
                    lenses: [],
                    alternatives: [],
                    alternativesRevisionCount: NonNegativeInt.make(0),
                    selectedAlternativeId: null,
                    selectionRationale: null,
                    strategy: "outcomes",
                    strategyRationale: null,
                    strategyMetadata: null,
                    granularity: "balanced",
                    tasks: [],
                    checks: [],
                    acMappings: [],
                    planDigest: currentPlanDigest,
                    tasksDigest: null,
                    updatedAt: IsoDateTime.make("2026-11-24T01:05:00.000Z"),
                  }
                : null,
            artifactKeys: [
              WorkflowBoardArtifactKey.make("spec.md"),
              WorkflowBoardArtifactKey.make("plan.md"),
              WorkflowBoardArtifactKey.make("wireframes/index.html"),
            ],
          };
        };

        const { atoms, registry } = yield* makeHarness({
          [WS_METHODS.workflowBoardGetCard]: () => Effect.sync(() => makeM2Card(currentRevision)),
          [WS_METHODS.workflowBoardGetArtifact]: () =>
            Effect.sync(() => ({
              ...makeArtifact(currentRevision, `<html>rev ${currentRevision}</html>`),
              key: WorkflowBoardArtifactKey.make("wireframes/index.html"),
              kind: "wireframes" as const,
              relativePath: WorkflowBoardArtifactKey.make("wireframes/index.html"),
            })),
          [WS_METHODS.workflowBoardExploreStep]: () =>
            Effect.sync(() => {
              currentRevision = 2;
              const card = makeM2Card(2);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                roundIndex: NonNegativeInt.make(1),
                pendingRoundPrompt: false,
                paused: false,
                exploration: {
                  roundIndex: NonNegativeInt.make(1),
                  pendingRoundPrompt: false,
                  paused: false,
                  unresolvedIds: [],
                  decisions: [],
                },
                decisions: [],
                card,
              };
            }),
          [WS_METHODS.workflowBoardChallengeStep]: () =>
            Effect.sync(() => {
              currentRevision = 3;
              const card = makeM2Card(3);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                challengeReceipt: {
                  status: "complete" as const,
                  concerns: [],
                  synthesisCount: NonNegativeInt.make(0),
                  specDigest: "sha256:spec-m2",
                  interviewDigest: null,
                  manualFallback: null,
                  updatedAt: IsoDateTime.make("2026-11-24T01:03:00.000Z"),
                },
                concerns: [],
                card,
              };
            }),
          [WS_METHODS.workflowBoardUpdatePlan]: () =>
            Effect.gen(function* () {
              callOrder.push("updatePlan:start");
              if (callOrder.length === 1) {
                yield* Deferred.succeed(updateEntered, undefined);
                yield* Deferred.await(updateGate);
              }
              currentRevision = currentRevision === 3 ? 4 : 5;
              currentPlanDigest = `sha256:plan-rev-${currentRevision}`;
              currentWireframesDigest = `sha256:wf-rev-${currentRevision}`;
              callOrder.push(`updatePlan:end:${currentRevision}`);
              const card = makeM2Card(currentRevision);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                planDigest: currentPlanDigest,
                specApproval: card.specApproval,
                planApproval: null,
                lenses: [],
                alternatives: [],
                selectedAlternativeId: null,
                selectionRationale: null,
                strategy: "outcomes" as const,
                strategyMetadata: null,
                granularity: "balanced" as const,
                tasks: [],
                acMappings: [],
                card,
              };
            }),
          [WS_METHODS.workflowBoardApprovePlan]: () =>
            Effect.fail(
              new WorkflowBoardRequestError({
                code: "external_edit_detected",
                message: "External edit on plan.md",
              }),
            ),
          [WS_METHODS.workflowBoardReturnForRevision]: () =>
            Effect.fail(
              new WorkflowBoardRequestError({
                code: "digest_mismatch",
                message: "Digest mismatch on disk",
              }),
            ),
        });

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(authState([AuthOrchestrationReadScope])),
        );
        expect(registry.get(atoms.exploreStep.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.challengeStep.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.updatePlan.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.approvePlan.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.returnForRevision.permissionAtom(ENV_ID))).toBe(false);

        const deniedApprovePlan = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.approvePlan, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(1) },
          }),
        );
        expect(deniedApprovePlan._tag).toBe("Failure");

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(
            authState([AuthOrchestrationReadScope, AuthOrchestrationOperateScope]),
          ),
        );
        expect(registry.get(atoms.exploreStep.permissionAtom(ENV_ID))).toBe(true);
        expect(registry.get(atoms.challengeStep.permissionAtom(ENV_ID))).toBe(true);
        expect(registry.get(atoms.updatePlan.permissionAtom(ENV_ID))).toBe(true);
        expect(registry.get(atoms.approvePlan.permissionAtom(ENV_ID))).toBe(true);
        expect(registry.get(atoms.returnForRevision.permissionAtom(ENV_ID))).toBe(true);

        const cardAtom = atoms.cardDetail({ environmentId: ENV_ID, input: { cardId: CARD_ID } });
        const artifactAtom = atoms.artifact({
          environmentId: ENV_ID,
          input: { cardId: CARD_ID, key: WorkflowBoardArtifactKey.make("wireframes/index.html") },
        });
        const u1 = registry.mount(cardAtom);
        const u2 = registry.mount(artifactAtom);
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            u1();
            u2();
          }),
        );

        expect(
          (yield* AtomRegistry.getResult(registry, cardAtom, { suspendOnWaiting: true })).revision,
        ).toBe(1);

        const rev2Waiter = waitForAtomSuccess(registry, cardAtom, (c) => c.revision === 2);
        const expRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.exploreStep, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(1), action: "step" },
          }),
        );
        expect(expRes._tag).toBe("Success");
        yield* Effect.promise(() => rev2Waiter);

        const rev3Waiter = waitForAtomSuccess(registry, cardAtom, (c) => c.revision === 3);
        const chRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.challengeStep, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(2), action: "complete" },
          }),
        );
        expect(chRes._tag).toBe("Success");
        yield* Effect.promise(() => rev3Waiter);

        // Concurrent updatePlan commands on the same card execute strictly serially
        const rev5CardWaiter = waitForAtomSuccess(registry, cardAtom, (c) => c.revision === 5);
        const rev5ArtifactWaiter = waitForAtomSuccess(
          registry,
          artifactAtom,
          (a) => a.revision === 5,
        );
        const firstPlan = yield* Effect.forkChild(
          atoms.updatePlan.run(registry, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(3), strategy: "outcomes" },
          }),
          { startImmediately: true },
        );
        yield* Deferred.await(updateEntered);
        const secondPlan = yield* Effect.forkChild(
          atoms.updatePlan.run(registry, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(4),
              granularity: "balanced",
            },
          }),
          { startImmediately: true },
        );
        yield* Deferred.succeed(updateGate, undefined);
        yield* Effect.all([Fiber.join(firstPlan), Fiber.join(secondPlan)]);
        expect(callOrder).toEqual([
          "updatePlan:start",
          "updatePlan:end:4",
          "updatePlan:start",
          "updatePlan:end:5",
        ]);
        yield* Effect.promise(() => rev5CardWaiter);
        yield* Effect.promise(() => rev5ArtifactWaiter);

        // external_edit_detected triggers cardDetail refresh
        currentRevision = 6;
        const extWaiter = waitForAtomSuccess(registry, cardAtom, (c) => c.revision === 6);
        const extErr = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.approvePlan, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(5) },
          }),
        );
        expect(extErr._tag).toBe("Failure");
        expect((yield* Effect.promise(() => extWaiter)).revision).toBe(6);

        // digest_mismatch triggers cardDetail refresh
        currentRevision = 7;
        const digestWaiter = waitForAtomSuccess(registry, cardAtom, (c) => c.revision === 7);
        const digErr = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.returnForRevision, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(6),
              targetStage: "spec",
              reason: "Retry",
            },
          }),
        );
        expect(digErr._tag).toBe("Failure");
        expect((yield* Effect.promise(() => digestWaiter)).revision).toBe(7);
      }),
  );

  it.effect(
    "wires M3 cardDiff query, taskId-scoped startStage singleFlight, and all 7 serialized M3 commands",
    () =>
      Effect.gen(function* () {
        let currentRevision = 1;
        let currentSnapshotDigest = "sha256:snap-1";
        const callOrder: string[] = [];
        const startedTasks: Array<string | undefined> = [];
        const pauseEntered = yield* Deferred.make<void>();
        const pauseGate = yield* Deferred.make<void>();

        const makeM3Card = (revision: number): WorkflowCardDetail => ({
          ...makeCardDetail(revision, "sha256:spec-m3"),
          stage: "execute",
          shipReadiness: {
            status: "blocked",
            ready: false,
            productSnapshotDigest: currentSnapshotDigest,
            blockers: [],
            deliveryReceipt: null,
          },
        });

        const makeCardDiff = (digest: string): WorkflowBoardGetCardDiffResult => ({
          cardId: CARD_ID,
          worktreePath: "/tmp/wt",
          baseRef: "main",
          productSnapshotDigest: digest,
          diffDigest: `diff:${digest}`,
          files: [],
          fileContractReport: null,
          truncated: false,
          generatedAt: IsoDateTime.make("2026-11-24T01:10:00.000Z"),
        });

        const { atoms, registry } = yield* makeHarness({
          [WS_METHODS.workflowBoardGetCard]: () => Effect.sync(() => makeM3Card(currentRevision)),
          [WS_METHODS.workflowBoardGetCardDiff]: () =>
            Effect.sync(() => makeCardDiff(currentSnapshotDigest)),
          [WS_METHODS.workflowBoardStartStage]: (input: { readonly taskId?: string }) =>
            Effect.sync(() => {
              startedTasks.push(input.taskId);
              currentRevision += 1;
              return {
                cardId: CARD_ID,
                revision: PositiveInt.make(currentRevision),
                stage: "execute" as const,
                taskId: input.taskId ?? null,
                stageRunId: WorkflowBoardStageRunId.make(`sr-${currentRevision}`),
                runId: RunId.make(`run-${currentRevision}`),
                threadId: ThreadId.make(`thread-${input.taskId ?? "review"}`),
                resumed: false,
              };
            }),
          [WS_METHODS.workflowBoardPauseCard]: () =>
            Effect.gen(function* () {
              callOrder.push("pause:start");
              yield* Deferred.succeed(pauseEntered, undefined);
              yield* Deferred.await(pauseGate);
              currentRevision += 1;
              callOrder.push("pause:end");
              const card = makeM3Card(currentRevision);
              return { cardId: CARD_ID, revision: card.revision, status: "paused" as const, card };
            }),
          [WS_METHODS.workflowBoardResumeCard]: () =>
            Effect.sync(() => {
              callOrder.push("resume");
              currentRevision += 1;
              currentSnapshotDigest = "sha256:snap-2";
              const card = makeM3Card(currentRevision);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                status: "active" as const,
                reconciledCount: NonNegativeInt.make(1),
                card,
              };
            }),
          [WS_METHODS.workflowBoardRetryStep]: () =>
            Effect.sync(() => {
              currentRevision += 1;
              const card = makeM3Card(currentRevision);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                stepKind: "check" as const,
                targetId: "CHK-01",
                card,
              };
            }),
          [WS_METHODS.workflowBoardProposeAmendment]: () =>
            Effect.sync(() => {
              currentRevision += 1;
              const card = makeM3Card(currentRevision);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                stage: "plan" as const,
                amendment: {
                  id: "amd-1",
                  cardRevision: card.revision,
                  targetStage: "plan" as const,
                  status: "applied" as const,
                  reason: "Allow extra file",
                  summary: null,
                  sourceTaskId: "TSK-01",
                  unexpectedFiles: [],
                  createdAt: IsoDateTime.make("2026-11-24T01:15:00.000Z"),
                },
                specApproval: null,
                planApproval: null,
                card,
              };
            }),
          [WS_METHODS.workflowBoardResolveFinding]: () =>
            Effect.sync(() => {
              currentRevision += 1;
              const card = makeM3Card(currentRevision);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                finding: {
                  id: "fnd-1",
                  severity: "minor" as const,
                  status: "resolved" as const,
                  criterionId: null,
                  file: null,
                  line: null,
                  message: "Minor",
                  disposition: "dismiss" as const,
                  resolutionNote: "False positive",
                  resolvedBy: "client",
                  resolvedAt: IsoDateTime.make("2026-11-24T01:16:00.000Z"),
                },
                reviewReport: {
                  reviewId: "rev-1",
                  stageRunId: "sr-rev",
                  threadId: ThreadId.make("thread-rev"),
                  status: "completed" as const,
                  verdict: "pass" as const,
                  summary: "Clean",
                  productSnapshotDigest: currentSnapshotDigest,
                  findings: [],
                  createdAt: IsoDateTime.make("2026-11-24T01:15:00.000Z"),
                  updatedAt: IsoDateTime.make("2026-11-24T01:16:00.000Z"),
                  completedAt: IsoDateTime.make("2026-11-24T01:16:00.000Z"),
                },
                card,
              };
            }),
          [WS_METHODS.workflowBoardRecordManualAcceptance]: () =>
            Effect.sync(() => {
              currentRevision += 1;
              const card = makeM3Card(currentRevision);
              return {
                cardId: CARD_ID,
                revision: card.revision,
                observation: {
                  criterionId: "AC-01",
                  method: "manual" as const,
                  status: "pass" as const,
                  checkIds: [],
                  procedure: "Verify",
                  observationNote: "Verified",
                  evidenceRef: null,
                  evidenceDigest: null,
                  productSnapshotDigest: currentSnapshotDigest,
                  observedBy: "client",
                  observedAt: IsoDateTime.make("2026-11-24T01:17:00.000Z"),
                  updatedAt: IsoDateTime.make("2026-11-24T01:17:00.000Z"),
                },
                shipReadiness: {
                  status: "ready" as const,
                  ready: true,
                  productSnapshotDigest: currentSnapshotDigest,
                  blockers: [],
                  deliveryReceipt: null,
                },
                card,
              };
            }),
          [WS_METHODS.workflowBoardApproveDelivery]: () =>
            Effect.sync(() => {
              currentRevision += 1;
              const card: WorkflowCardDetail = {
                ...makeM3Card(currentRevision),
                stage: "ship",
                status: "completed",
                deliveryStatus: "shipped",
              };
              const deliveryReceipt = {
                id: "del-1",
                approvalId: WorkflowBoardApprovalId.make("appr-del"),
                cardRevision: card.revision,
                productSnapshotDigest: currentSnapshotDigest,
                commitSha: null,
                specDigest: "sha256:spec-m3",
                planDigest: "sha256:plan",
                tasksDigest: "sha256:tasks",
                checksDigest: "sha256:checks",
                reviewId: "rev-1",
                acVerdictsSummary: "1/1 pass",
                approvedBy: "client",
                approvedAt: IsoDateTime.make("2026-11-24T01:18:00.000Z"),
                status: "shipped" as const,
              };
              return {
                cardId: CARD_ID,
                revision: card.revision,
                status: "completed" as const,
                deliveryStatus: "shipped" as const,
                deliveryReceipt,
                deliveryApproval: {
                  id: WorkflowBoardApprovalId.make("appr-del"),
                  gate: "delivery" as const,
                  status: "valid" as const,
                  revision: card.revision,
                  artifactDigest: currentSnapshotDigest,
                  specDigest: "sha256:spec-m3",
                  wireframesDigest: null,
                  approvedAt: IsoDateTime.make("2026-11-24T01:18:00.000Z"),
                  approvedBy: "client",
                },
                card,
              };
            }),
        });

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(authState([AuthOrchestrationReadScope])),
        );
        expect(registry.get(atoms.pauseCard.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.resumeCard.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.retryStep.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.proposeAmendment.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.resolveFinding.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.recordManualAcceptance.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.approveDelivery.permissionAtom(ENV_ID))).toBe(false);

        const diffAtom = atoms.cardDiff({
          environmentId: ENV_ID,
          input: { cardId: CARD_ID, includePatches: true },
        });
        const cardAtom = atoms.cardDetail({ environmentId: ENV_ID, input: { cardId: CARD_ID } });
        const uDiff = registry.mount(diffAtom);
        const uCard = registry.mount(cardAtom);
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            uDiff();
            uCard();
          }),
        );

        expect(
          (yield* AtomRegistry.getResult(registry, diffAtom, { suspendOnWaiting: true }))
            .productSnapshotDigest,
        ).toBe("sha256:snap-1");

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(
            authState([AuthOrchestrationReadScope, AuthOrchestrationOperateScope]),
          ),
        );
        expect(registry.get(atoms.approveDelivery.permissionAtom(ENV_ID))).toBe(true);

        // Distinct taskId values on startStage must not coalesce at the same expectedRevision
        const t1 = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.startStage, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(1),
              stage: "execute",
              taskId: "TSK-01",
            },
          }),
        );
        const t2 = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.startStage, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(1),
              stage: "execute",
              taskId: "TSK-02",
            },
          }),
        );
        expect(t1._tag).toBe("Success");
        expect(t2._tag).toBe("Success");
        expect(startedTasks).toEqual(["TSK-01", "TSK-02"]);

        // Serial execution between pauseCard and resumeCard, plus cardDiff refresh on snapshot change
        const snap2Waiter = waitForAtomSuccess(
          registry,
          diffAtom,
          (diff) => diff.productSnapshotDigest === "sha256:snap-2",
        );
        const pauseFiber = yield* Effect.forkChild(
          atoms.pauseCard.run(registry, {
            environmentId: ENV_ID,
            input: { cardId: CARD_ID, expectedRevision: PositiveInt.make(3), reason: "Hold" },
          }),
          { startImmediately: true },
        );
        yield* Deferred.await(pauseEntered);
        const resumeFiber = yield* Effect.forkChild(
          atoms.resumeCard.run(registry, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(4),
              reconcileInterrupted: true,
            },
          }),
          { startImmediately: true },
        );
        yield* Deferred.succeed(pauseGate, undefined);
        yield* Effect.all([Fiber.join(pauseFiber), Fiber.join(resumeFiber)]);
        expect(callOrder).toEqual(["pause:start", "pause:end", "resume"]);
        expect((yield* Effect.promise(() => snap2Waiter)).productSnapshotDigest).toBe(
          "sha256:snap-2",
        );

        // Remaining M3 commands succeed and bump card revision
        const retryRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.retryStep, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(5),
              stepKind: "check",
              targetId: "CHK-01",
            },
          }),
        );
        const amendRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.proposeAmendment, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(6),
              targetStage: "plan",
              reason: "Allow extra file",
            },
          }),
        );
        const resolveRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.resolveFinding, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(7),
              reviewId: "rev-1",
              findingId: "fnd-1",
              disposition: "dismiss",
              reason: "False positive",
            },
          }),
        );
        const acceptRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.recordManualAcceptance, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(8),
              criterionId: "AC-01",
              verdict: "pass",
              note: "Verified",
            },
          }),
        );
        const shipRes = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.approveDelivery, {
            environmentId: ENV_ID,
            input: {
              cardId: CARD_ID,
              expectedRevision: PositiveInt.make(9),
              expectedProductSnapshotDigest: "sha256:snap-2",
            },
          }),
        );
        expect(retryRes._tag).toBe("Success");
        expect(amendRes._tag).toBe("Success");
        expect(resolveRes._tag).toBe("Success");
        expect(acceptRes._tag).toBe("Success");
        expect(shipRes._tag).toBe("Success");

        // invalidateCard refreshes cardDiff directly
        currentSnapshotDigest = "sha256:snap-3";
        const snap3Waiter = waitForAtomSuccess(
          registry,
          diffAtom,
          (diff) => diff.productSnapshotDigest === "sha256:snap-3",
        );
        atoms.invalidateCard(registry, ENV_ID, CARD_ID);
        expect((yield* Effect.promise(() => snap3Waiter)).productSnapshotDigest).toBe(
          "sha256:snap-3",
        );
      }),
  );

  it.effect(
    "guards previewCliImport/applyCliImport permissions, serializes apply, refreshes cardDetail/artifact for result.cardId, and separates startStage singleFlight by policy acknowledgement",
    () =>
      Effect.gen(function* () {
        const importedCardId = WorkflowCardId.make("cli-spec-t3-code");
        let currentRevision = 1;
        let currentBrief = "Imported rev 1";
        const applyOrder: string[] = [];
        const startStageCalls: Array<{
          readonly acknowledgeUnsupportedPolicies?: boolean;
          readonly acknowledgedPolicies?: ReadonlyArray<string>;
        }> = [];
        const applyEntered = yield* Deferred.make<void>();
        const applyGate = yield* Deferred.make<void>();

        const makeImportedCard = (revision: number, acknowledged: boolean): WorkflowCardDetail => ({
          ...makeCardDetail(revision, "sha256:cli-spec"),
          cardId: importedCardId,
          stage: "execute",
          specApprovalStatus: "historical",
          planApprovalStatus: "historical",
          nextAction: acknowledged ? "resolve_blocked_task" : "acknowledge_policies",
          cliImport: {
            sourceSystem: "legion-cli",
            sourceCliDir: WorkflowBoardArtifactKey.make(".legion-cli"),
            sourceSpecId: "spec-t3-code",
            owner: "legioncode",
            importedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
            lastSyncedAt: IsoDateTime.make(`2026-11-27T00:0${revision}:00.000Z`),
            sourceDigest: `sha256:src-${revision}`,
            decisionIds: ["D-001"],
            assumptionIds: ["ASM-0001"],
            acceptanceCriterionIds: ["AC-P0-01"],
            historicalReceipts: [
              {
                id: "hist-1",
                kind: "spec_approval",
                status: "historical",
                sourcePath: WorkflowBoardArtifactKey.make(
                  ".legion-cli/workflow/spec-approval.yaml",
                ),
                recordedAt: null,
                recordedBy: "user",
                summary: "Historical spec approval",
                requiresNativeRevalidation: true,
              },
            ],
            unsupportedPolicies: [
              {
                key: "sandbox.requireHardened",
                category: "sandbox",
                summary: "Requires hardened sandbox",
                configuredValue: "true",
                status: acknowledged ? "acknowledged" : "unsupported",
                blocksExecution: true,
                sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
              },
            ],
            acknowledgedPolicies: acknowledged ? ["sandbox.requireHardened"] : [],
            missingLogNotes: [],
            preservedArtifactPaths: [WorkflowBoardArtifactKey.make("spec.md")],
            assurancePolicySummary: null,
          },
        });

        const { atoms, registry } = yield* makeHarness({
          [WS_METHODS.workflowBoardGetCard]: () =>
            Effect.sync(() => makeImportedCard(currentRevision, currentRevision > 1)),
          [WS_METHODS.workflowBoardGetArtifact]: () =>
            Effect.sync(() => ({
              ...makeArtifact(currentRevision, currentBrief),
              cardId: importedCardId,
              key: WorkflowBoardArtifactKey.make("spec.md"),
              kind: "spec" as const,
              relativePath: WorkflowBoardArtifactKey.make("spec.md"),
            })),
          [WS_METHODS.workflowBoardPreviewCliImport]: (input: { readonly cliDir?: string }) =>
            Effect.sync(() => {
              const blocked = input.cliDir === ".legion-cli-locked";
              return {
                projectId: PROJECT_ID,
                sourcePath: WorkflowBoardArtifactKey.make(input.cliDir ?? ".legion-cli"),
                specId: "spec-t3-code",
                title: "T3 Code",
                stage: "execute" as const,
                status: "blocked" as const,
                existingCardId: blocked ? null : importedCardId,
                alreadyImported: !blocked,
                activeLocks: blocked
                  ? [
                      {
                        kind: "engine_lock" as const,
                        relativePath: WorkflowBoardArtifactKey.make(
                          ".legion-cli-locked/index/engine.lock",
                        ),
                        status: "active" as const,
                        pid: PositiveInt.make(4242),
                        runId: "run-live",
                        startedAt: null,
                        message: "Active CLI run",
                      },
                    ]
                  : [],
                staleLocks: [],
                canApply: !blocked,
                refusalReason: blocked ? "Active CLI run lock conflict detected" : null,
                tasks: [],
                decisionIds: ["D-001"],
                assumptionIds: ["ASM-0001"],
                acceptanceCriterionIds: ["AC-P0-01"],
                historicalReceipts: [],
                unsupportedPolicies: [],
                missingLogNotes: [],
                warnings: [],
              };
            }),
          [WS_METHODS.workflowBoardApplyCliImport]: (input: {
            readonly cliDir?: string;
            readonly acknowledgeUnsupportedPolicies?: boolean;
          }) =>
            Effect.gen(function* () {
              if (input.cliDir === ".legion-cli-locked") {
                return yield* new WorkflowBoardRequestError({
                  code: "contract_conflict",
                  message: "Refusing CLI import takeover while active CLI run lock is held",
                });
              }
              applyOrder.push("apply:start");
              if (applyOrder.length === 1) {
                yield* Deferred.succeed(applyEntered, undefined);
                yield* Deferred.await(applyGate);
              }
              const created = applyOrder.length === 1;
              currentRevision = created ? 1 : 2;
              currentBrief = `Imported rev ${currentRevision}`;
              applyOrder.push(`apply:end:${currentRevision}`);
              const card = makeImportedCard(
                currentRevision,
                Boolean(input.acknowledgeUnsupportedPolicies),
              );
              return {
                cardId: importedCardId,
                projectId: PROJECT_ID,
                revision: card.revision,
                created,
                sourceSpecId: "spec-t3-code",
                tasks: [],
                historicalReceipts: card.cliImport!.historicalReceipts,
                unsupportedPolicies: card.cliImport!.unsupportedPolicies,
                missingLogNotes: [],
                card,
              };
            }),
          [WS_METHODS.workflowBoardStartStage]: (input: {
            readonly taskId?: string;
            readonly acknowledgeUnsupportedPolicies?: boolean;
            readonly acknowledgedPolicies?: ReadonlyArray<string>;
          }) =>
            Effect.gen(function* () {
              if (input.taskId === "TSK-0002") {
                currentRevision += 1;
                return yield* new WorkflowBoardRequestError({
                  code: "contract_conflict",
                  message: "Task TSK-0002 is blocked",
                });
              }
              startStageCalls.push({
                ...(input.acknowledgeUnsupportedPolicies !== undefined
                  ? { acknowledgeUnsupportedPolicies: input.acknowledgeUnsupportedPolicies }
                  : {}),
                ...(input.acknowledgedPolicies !== undefined
                  ? { acknowledgedPolicies: input.acknowledgedPolicies }
                  : {}),
              });
              currentRevision += 1;
              return {
                cardId: importedCardId,
                revision: PositiveInt.make(currentRevision),
                stage: "execute" as const,
                taskId: input.taskId ?? "TSK-0001",
                stageRunId: WorkflowBoardStageRunId.make(`sr-${currentRevision}`),
                runId: RunId.make(`run-${currentRevision}`),
                threadId: ThreadId.make(`thread-${currentRevision}`),
                resumed: false,
              };
            }),
        });

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(authState([AuthOrchestrationReadScope])),
        );
        expect(registry.get(atoms.previewCliImport.permissionAtom(ENV_ID))).toBe(false);
        expect(registry.get(atoms.applyCliImport.permissionAtom(ENV_ID))).toBe(false);
        const deniedPreview = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.previewCliImport, {
            environmentId: ENV_ID,
            input: { projectId: PROJECT_ID },
          }),
        );
        expect(deniedPreview._tag).toBe("Failure");

        registry.set(
          sessions(ENV_ID),
          AsyncResult.success(
            authState([AuthOrchestrationReadScope, AuthOrchestrationOperateScope]),
          ),
        );
        expect(registry.get(atoms.previewCliImport.permissionAtom(ENV_ID))).toBe(true);
        expect(registry.get(atoms.applyCliImport.permissionAtom(ENV_ID))).toBe(true);

        const lockedPreview = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.previewCliImport, {
            environmentId: ENV_ID,
            input: {
              projectId: PROJECT_ID,
              cliDir: WorkflowBoardArtifactKey.make(".legion-cli-locked"),
            },
          }),
        );
        expect(lockedPreview._tag).toBe("Success");
        if (lockedPreview._tag === "Success") {
          expect(lockedPreview.value.canApply).toBe(false);
          expect(lockedPreview.value.activeLocks).toHaveLength(1);
        }

        const conflictApply = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.applyCliImport, {
            environmentId: ENV_ID,
            input: {
              projectId: PROJECT_ID,
              cliDir: WorkflowBoardArtifactKey.make(".legion-cli-locked"),
            },
          }),
        );
        expect(conflictApply._tag).toBe("Failure");

        const cardAtom = atoms.cardDetail({
          environmentId: ENV_ID,
          input: { cardId: importedCardId },
        });
        const artifactAtom = atoms.artifact({
          environmentId: ENV_ID,
          input: { cardId: importedCardId, key: WorkflowBoardArtifactKey.make("spec.md") },
        });
        const uCard = registry.mount(cardAtom);
        const uArtifact = registry.mount(artifactAtom);
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            uCard();
            uArtifact();
          }),
        );

        const rev2CardWaiter = waitForAtomSuccess(registry, cardAtom, (c) => c.revision === 2);
        const rev2ArtifactWaiter = waitForAtomSuccess(
          registry,
          artifactAtom,
          (a) => a.revision === 2,
        );
        const apply1 = yield* Effect.forkChild(
          atoms.applyCliImport.run(registry, {
            environmentId: ENV_ID,
            input: { projectId: PROJECT_ID },
          }),
          { startImmediately: true },
        );
        yield* Deferred.await(applyEntered);
        const apply2 = yield* Effect.forkChild(
          atoms.applyCliImport.run(registry, {
            environmentId: ENV_ID,
            input: { projectId: PROJECT_ID, acknowledgeUnsupportedPolicies: true },
          }),
          { startImmediately: true },
        );
        yield* Deferred.succeed(applyGate, undefined);
        const [res1, res2] = yield* Effect.all([Fiber.join(apply1), Fiber.join(apply2)]);
        expect(res1.created).toBe(true);
        expect(res2.created).toBe(false);
        expect(applyOrder).toEqual(["apply:start", "apply:end:1", "apply:start", "apply:end:2"]);
        expect(
          (yield* Effect.promise(() => rev2CardWaiter)).cliImport?.acknowledgedPolicies,
        ).toEqual(["sandbox.requireHardened"]);
        expect((yield* Effect.promise(() => rev2ArtifactWaiter)).content).toBe("Imported rev 2");

        // Unacknowledged and acknowledged startStage Execute calls at the same expectedRevision must not coalesce
        const s1 = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.startStage, {
            environmentId: ENV_ID,
            input: {
              cardId: importedCardId,
              expectedRevision: PositiveInt.make(2),
              stage: "execute",
              taskId: "TSK-0001",
            },
          }),
        );
        const s2 = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.startStage, {
            environmentId: ENV_ID,
            input: {
              cardId: importedCardId,
              expectedRevision: PositiveInt.make(2),
              stage: "execute",
              taskId: "TSK-0001",
              acknowledgeUnsupportedPolicies: true,
              acknowledgedPolicies: ["sandbox.requireHardened"],
            },
          }),
        );
        expect(s1._tag).toBe("Success");
        expect(s2._tag).toBe("Success");
        expect(startStageCalls).toEqual([
          {},
          {
            acknowledgeUnsupportedPolicies: true,
            acknowledgedPolicies: ["sandbox.requireHardened"],
          },
        ]);

        // contract_conflict on startStage refreshes cardDetail so partial server-side updates remain visible
        const contractConflictWaiter = waitForAtomSuccess(
          registry,
          cardAtom,
          (c) => c.revision === 5,
        );
        const blockedTaskStart = yield* Effect.promise(() =>
          runAtomCommand(registry, atoms.startStage, {
            environmentId: ENV_ID,
            input: {
              cardId: importedCardId,
              expectedRevision: PositiveInt.make(4),
              stage: "execute",
              taskId: "TSK-0002",
              acknowledgeUnsupportedPolicies: true,
              acknowledgedPolicies: ["sandbox.requireHardened"],
            },
          }),
        );
        expect(blockedTaskStart._tag).toBe("Failure");
        expect((yield* Effect.promise(() => contractConflictWaiter)).revision).toBe(5);
      }),
  );
});
