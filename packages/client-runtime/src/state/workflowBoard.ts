import {
  type EnvironmentId,
  type ProjectWorkflowBoardSnapshot,
  WorkflowBoardRequestError,
  type WorkflowCardDetail,
  type WorkflowCardId,
  WS_METHODS,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import * as Effect from "effect/Effect";
import * as Option from "effect/Option";
import * as Schema from "effect/Schema";
import { AsyncResult, Atom, type AtomRegistry } from "effect/reactivity";

import type { EnvironmentRegistry } from "../connection/registry.ts";
import { createCommandPermissions } from "./commandPermissions.ts";
import {
  createAtomCommandScheduler,
  createEnvironmentGuardedRpcQueryAtomFamily,
  createEnvironmentRpcCommand,
  createEnvironmentRpcSubscriptionAtomFamily,
} from "./runtime.ts";

const isWorkflowBoardRequestError = Schema.is(WorkflowBoardRequestError);

function cardKey(environmentId: EnvironmentId, cardId: WorkflowCardId): string {
  return JSON.stringify([environmentId, cardId]);
}

function cardSummaryRefreshKey(
  snapshot: ProjectWorkflowBoardSnapshot,
  cardId: WorkflowCardId,
): string {
  const card = snapshot.cards.find((candidate) => candidate.cardId === cardId);
  return card === undefined ? "missing" : `${card.revision}:${card.updatedAt}`;
}

function cardArtifactVersion(card: WorkflowCardDetail): string {
  return `${card.revision}:${card.spec.specDigest ?? ""}:${card.wireframesDigest ?? ""}:${card.plan?.planDigest ?? ""}:${card.shipReadiness?.productSnapshotDigest ?? ""}:${card.artifactKeys.join(",")}`;
}

export function createWorkflowBoardEnvironmentAtoms<R, E>(
  runtime: Atom.AtomRuntime<EnvironmentRegistry | R, E>,
) {
  const getCardPermissions = createCommandPermissions(runtime, WS_METHODS.workflowBoardGetCard);
  const getArtifactPermissions = createCommandPermissions(
    runtime,
    WS_METHODS.workflowBoardGetArtifact,
  );
  const getCardDiffPermissions = createCommandPermissions(
    runtime,
    WS_METHODS.workflowBoardGetCardDiff,
  );
  const subscribePermissions = createCommandPermissions(runtime, WS_METHODS.workflowBoardSubscribe);

  const cardInvalidationAtom = Atom.family((key: string) =>
    Atom.make(0).pipe(Atom.withLabel(`environment-data:workflow-board:card-invalidation:${key}`)),
  );
  const cardArtifactVersionAtom = Atom.family((key: string) =>
    Atom.make("0").pipe(Atom.withLabel(`environment-data:workflow-board:artifact-version:${key}`)),
  );
  const lastSeenCardVersion = new Map<string, string>();

  const bumpCardInvalidation = (
    registry: AtomRegistry.AtomRegistry,
    environmentId: EnvironmentId,
    cardId: WorkflowCardId,
  ) => {
    const key = cardKey(environmentId, cardId);
    registry.set(cardInvalidationAtom(key), registry.get(cardInvalidationAtom(key)) + 1);
  };

  const setCardArtifactVersion = (
    registry: AtomRegistry.AtomRegistry,
    environmentId: EnvironmentId,
    cardId: WorkflowCardId,
    version: string,
  ) => {
    const key = cardKey(environmentId, cardId);
    lastSeenCardVersion.set(key, version);
    registry.set(cardArtifactVersionAtom(key), version);
  };

  const boardSnapshot = createEnvironmentRpcSubscriptionAtomFamily(runtime, {
    label: "environment-data:workflow-board:snapshot",
    tag: WS_METHODS.workflowBoardSubscribe,
  });

  const cardDetail = createEnvironmentGuardedRpcQueryAtomFamily(runtime, {
    label: "environment-data:workflow-board:card",
    tag: WS_METHODS.workflowBoardGetCard,
    staleTimeMs: 30_000,
    onAnswer: (target, card, registry) =>
      Effect.sync(() => {
        const key = cardKey(target.environmentId, card.cardId);
        const version = cardArtifactVersion(card);
        const previous = lastSeenCardVersion.get(key);
        lastSeenCardVersion.set(key, version);
        if (previous !== undefined && previous !== version) {
          registry.set(cardArtifactVersionAtom(key), version);
        }
      }),
    refreshTrigger: ({ environmentId, input }, query) =>
      Atom.make((get) => {
        const invalidation = get(cardInvalidationAtom(cardKey(environmentId, input.cardId)));
        const permitted = get(getCardPermissions.permissionAtom(environmentId));
        if (input.projectId === undefined) {
          return `${invalidation}:${permitted}`;
        }
        const snapshot = AsyncResult.value(
          get(boardSnapshot({ environmentId, input: { projectId: input.projectId } })),
        );
        const snapshotKey = Option.isSome(snapshot)
          ? cardSummaryRefreshKey(snapshot.value, input.cardId)
          : Option.match(AsyncResult.value(get.once(query)), {
              onNone: () => "pending",
              onSome: (current) => `${current.revision}:${current.updatedAt}`,
            });
        return `${invalidation}:${permitted}:${snapshotKey}`;
      }),
  });

  const artifact = createEnvironmentGuardedRpcQueryAtomFamily(runtime, {
    label: "environment-data:workflow-board:artifact",
    tag: WS_METHODS.workflowBoardGetArtifact,
    staleTimeMs: 30_000,
    refreshTrigger: ({ environmentId, input }) =>
      Atom.make(
        (get) =>
          `${get(cardInvalidationAtom(cardKey(environmentId, input.cardId)))}:${get(cardArtifactVersionAtom(cardKey(environmentId, input.cardId)))}:${get(getArtifactPermissions.permissionAtom(environmentId))}`,
      ),
  });

  const cardDiff = createEnvironmentGuardedRpcQueryAtomFamily(runtime, {
    label: "environment-data:workflow-board:card-diff",
    tag: WS_METHODS.workflowBoardGetCardDiff,
    staleTimeMs: 5_000,
    refreshTrigger: ({ environmentId, input }) =>
      Atom.make(
        (get) =>
          `${get(cardInvalidationAtom(cardKey(environmentId, input.cardId)))}:${get(cardArtifactVersionAtom(cardKey(environmentId, input.cardId)))}:${get(getCardDiffPermissions.permissionAtom(environmentId))}`,
      ),
  });

  const refreshCardOnRevisionConflict = (
    target: {
      readonly environmentId: EnvironmentId;
      readonly input: { readonly cardId: WorkflowCardId };
    },
    registry: AtomRegistry.AtomRegistry,
    exit: { readonly _tag: "Success" | "Failure"; readonly cause?: Cause.Cause<unknown> },
  ) =>
    Effect.sync(() => {
      if (exit._tag !== "Failure" || exit.cause === undefined) return;
      const error = Option.getOrNull(Cause.findErrorOption(exit.cause));
      if (
        isWorkflowBoardRequestError(error) &&
        (error.code === "revision_conflict" ||
          error.code === "external_edit_detected" ||
          error.code === "digest_mismatch" ||
          error.code === "contract_conflict")
      ) {
        bumpCardInvalidation(registry, target.environmentId, target.input.cardId);
      }
    });

  const boardScheduler = createAtomCommandScheduler();

  const cardMutationKey = ({
    environmentId,
    input,
  }: {
    readonly environmentId: EnvironmentId;
    readonly input: { readonly cardId: WorkflowCardId };
  }) => JSON.stringify([environmentId, input.cardId]);

  const onCardMutationSuccess = (
    {
      environmentId,
      input,
    }: {
      readonly environmentId: EnvironmentId;
      readonly input: { readonly cardId: WorkflowCardId };
    },
    registry: AtomRegistry.AtomRegistry,
    result: { readonly card: WorkflowCardDetail },
  ) =>
    Effect.sync(() => {
      setCardArtifactVersion(
        registry,
        environmentId,
        input.cardId,
        cardArtifactVersion(result.card),
      );
      bumpCardInvalidation(registry, environmentId, input.cardId);
    });

  return {
    boardSnapshot,
    cardDetail,
    artifact,
    cardDiff,
    readPermissionAtom: subscribePermissions.permissionAtom,
    invalidateCard: bumpCardInvalidation,
    previewCliImport: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:preview-cli-import",
      tag: WS_METHODS.workflowBoardPreviewCliImport,
      scheduler: boardScheduler,
      concurrency: {
        mode: "latest",
        key: ({ environmentId, input }) =>
          JSON.stringify([environmentId, input.projectId, input.cliDir?.trim() || ".legion-cli"]),
      },
    }),
    applyCliImport: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:apply-cli-import",
      tag: WS_METHODS.workflowBoardApplyCliImport,
      scheduler: boardScheduler,
      concurrency: {
        mode: "serial",
        key: ({ environmentId, input }) => JSON.stringify([environmentId, input.projectId]),
      },
      onSuccess: ({ environmentId }, registry, result) =>
        Effect.sync(() => {
          setCardArtifactVersion(
            registry,
            environmentId,
            result.cardId,
            cardArtifactVersion(result.card),
          );
          bumpCardInvalidation(registry, environmentId, result.cardId);
        }),
    }),
    createCard: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:create-card",
      tag: WS_METHODS.workflowBoardCreateCard,
      scheduler: boardScheduler,
      concurrency: {
        mode: "serial",
        key: ({ environmentId, input }) => JSON.stringify([environmentId, input.projectId]),
      },
      onSuccess: ({ environmentId }, registry, card) =>
        Effect.sync(() => {
          setCardArtifactVersion(registry, environmentId, card.cardId, cardArtifactVersion(card));
          bumpCardInvalidation(registry, environmentId, card.cardId);
        }),
    }),
    updateSpec: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:update-spec",
      tag: WS_METHODS.workflowBoardUpdateSpec,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    approveSpec: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:approve-spec",
      tag: WS_METHODS.workflowBoardApproveSpec,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    exploreStep: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:explore-step",
      tag: WS_METHODS.workflowBoardExploreStep,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    challengeStep: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:challenge-step",
      tag: WS_METHODS.workflowBoardChallengeStep,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    updatePlan: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:update-plan",
      tag: WS_METHODS.workflowBoardUpdatePlan,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    approvePlan: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:approve-plan",
      tag: WS_METHODS.workflowBoardApprovePlan,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    returnForRevision: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:return-for-revision",
      tag: WS_METHODS.workflowBoardReturnForRevision,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    pauseCard: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:pause-card",
      tag: WS_METHODS.workflowBoardPauseCard,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    resumeCard: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:resume-card",
      tag: WS_METHODS.workflowBoardResumeCard,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    retryStep: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:retry-step",
      tag: WS_METHODS.workflowBoardRetryStep,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    proposeAmendment: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:propose-amendment",
      tag: WS_METHODS.workflowBoardProposeAmendment,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    resolveFinding: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:resolve-finding",
      tag: WS_METHODS.workflowBoardResolveFinding,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    recordManualAcceptance: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:record-manual-acceptance",
      tag: WS_METHODS.workflowBoardRecordManualAcceptance,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    approveDelivery: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:approve-delivery",
      tag: WS_METHODS.workflowBoardApproveDelivery,
      scheduler: boardScheduler,
      concurrency: { mode: "serial", key: cardMutationKey },
      onSuccess: onCardMutationSuccess,
      onSettled: refreshCardOnRevisionConflict,
    }),
    startStage: createEnvironmentRpcCommand(runtime, {
      label: "environment-data:workflow-board:start-stage",
      tag: WS_METHODS.workflowBoardStartStage,
      scheduler: boardScheduler,
      concurrency: {
        mode: "singleFlight",
        key: ({ environmentId, input }) =>
          JSON.stringify([
            environmentId,
            input.cardId,
            input.stage,
            input.expectedRevision,
            input.taskId ?? null,
            Boolean(input.acknowledgeUnsupportedPolicies),
            input.acknowledgedPolicies ? [...input.acknowledgedPolicies].sort() : null,
          ]),
      },
      onSuccess: ({ environmentId, input }, registry, result) =>
        Effect.sync(() => {
          setCardArtifactVersion(
            registry,
            environmentId,
            input.cardId,
            `${result.revision}:${result.stage}:${result.stageRunId}:${result.taskId ?? ""}`,
          );
          bumpCardInvalidation(registry, environmentId, input.cardId);
        }),
      onSettled: refreshCardOnRevisionConflict,
    }),
  };
}
