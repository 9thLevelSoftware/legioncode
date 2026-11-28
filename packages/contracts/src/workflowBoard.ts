import * as Schema from "effect/Schema";

import {
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProjectId,
  RunId,
  ThreadId,
  TrimmedNonEmptyString,
} from "./baseSchemas.ts";
import { ModelSelection } from "./modelSelection.ts";
import { OrchestrationV2ThreadLaunchWorkspaceStrategy } from "./orchestrationV2.ts";
import { ProviderInteractionMode, RuntimeMode } from "./providerPolicy.ts";
import {
  WorkflowBoardAcObservationRecord,
  WorkflowBoardAmendmentTargetStage,
  WorkflowBoardDeliveryStatus,
  WorkflowBoardDiffFileEntry,
  WorkflowBoardFindingResolveDisposition,
  WorkflowBoardManualAcceptanceVerdict,
  WorkflowBoardPlanAmendmentRecord,
  WorkflowBoardPlannedCheckReceipt,
  WorkflowBoardRecoveryState,
  WorkflowBoardRetryStepKind,
  WorkflowBoardReviewFinding,
  WorkflowBoardReviewReport,
  WorkflowBoardShipBlocker,
  WorkflowBoardStageRunStage,
  WorkflowBoardTaskExecutionState,
  WorkflowBoardTaskFileContractReport,
} from "./workflowBoardM3.ts";
import {
  WorkflowBoardApplyCliImportInput,
  WorkflowBoardCliImportHistoricalReceipt,
  WorkflowBoardCliImportLockEntry,
  WorkflowBoardCliImportMissingLogNote,
  WorkflowBoardCliImportProvenance,
  WorkflowBoardCliImportTaskPreview,
  WorkflowBoardCliImportUnsupportedPolicy,
  WorkflowBoardPreviewCliImportInput,
} from "./workflowBoardM4.ts";
import {
  WorkflowBoardAcEvidenceMapping,
  WorkflowBoardAcEvidenceMappingsArray,
  WorkflowBoardArtifactKey,
  WorkflowBoardChallengeConcern,
  WorkflowBoardChallengeConcernsArray,
  WorkflowBoardChallengeConcernResponse,
  WorkflowBoardChallengeReceipt,
  WorkflowBoardChallengeStepAction,
  WorkflowBoardExplorationDecision,
  WorkflowBoardExplorationState,
  WorkflowBoardExploreAnswersArray,
  WorkflowBoardExploreStepAction,
  WorkflowBoardIntakeGoal,
  WorkflowBoardNonGitArtifactKey,
  WorkflowBoardPlanAlternative,
  WorkflowBoardPlanAlternativesPair,
  WorkflowBoardPlanDocument,
  WorkflowBoardPlanLensEntry,
  WorkflowBoardPlanLensesArray,
  WorkflowBoardPlannedCheck,
  WorkflowBoardPlanningStrategy,
  WorkflowBoardReturnTargetStage,
  WorkflowBoardSafeInputPath,
  WorkflowBoardSourceInputDigest,
  WorkflowBoardSpecDiff,
  WorkflowBoardSpecIntakeAnalysis,
  WorkflowBoardStrategyMetadata,
  WorkflowBoardTaskContract,
  WorkflowBoardTaskContractsArray,
  WorkflowBoardTaskGranularity,
} from "./workflowBoardPlanning.ts";

export { WorkflowBoardArtifactKey };

export const WorkflowCardId = TrimmedNonEmptyString.pipe(Schema.brand("WorkflowCardId"));
export type WorkflowCardId = typeof WorkflowCardId.Type;

export const WorkflowBoardApprovalId = TrimmedNonEmptyString.pipe(
  Schema.brand("WorkflowBoardApprovalId"),
);
export type WorkflowBoardApprovalId = typeof WorkflowBoardApprovalId.Type;

export const WorkflowBoardStageRunId = TrimmedNonEmptyString.pipe(
  Schema.brand("WorkflowBoardStageRunId"),
);
export type WorkflowBoardStageRunId = typeof WorkflowBoardStageRunId.Type;

export const WorkflowBoardStage = Schema.Literals(["spec", "plan", "execute", "ship"]);
export type WorkflowBoardStage = typeof WorkflowBoardStage.Type;

export const WorkflowBoardCardStatus = Schema.Literals([
  "active",
  "blocked",
  "paused",
  "completed",
]);
export type WorkflowBoardCardStatus = typeof WorkflowBoardCardStatus.Type;

export const WorkflowBoardSpecSourceType = Schema.Literals(["idea", "file", "codebase"]);
export type WorkflowBoardSpecSourceType = typeof WorkflowBoardSpecSourceType.Type;

export const WorkflowBoardAssistanceMode = Schema.Literals(["guided", "balanced", "direct"]);
export type WorkflowBoardAssistanceMode = typeof WorkflowBoardAssistanceMode.Type;

export const WorkflowBoardApprovalGate = Schema.Literals(["spec", "plan", "delivery"]);
export type WorkflowBoardApprovalGate = typeof WorkflowBoardApprovalGate.Type;

export const WorkflowBoardApprovalStatus = Schema.Literals([
  "valid",
  "stale",
  "superseded",
  "historical",
]);
export type WorkflowBoardApprovalStatus = typeof WorkflowBoardApprovalStatus.Type;

export const WorkflowBoardArtifactKind = Schema.Literals([
  "card",
  "spec",
  "plan",
  "design",
  "wireframes",
]);
export type WorkflowBoardArtifactKind = typeof WorkflowBoardArtifactKind.Type;

export const WorkflowBoardStageRunStatus = Schema.Literals([
  "pending",
  "launching",
  "running",
  "paused",
  "completed",
  "failed",
  "cancelled",
]);
export type WorkflowBoardStageRunStatus = typeof WorkflowBoardStageRunStatus.Type;

export const WorkflowBoardAcceptanceCriterion = Schema.Struct({
  id: TrimmedNonEmptyString,
  text: TrimmedNonEmptyString,
});
export type WorkflowBoardAcceptanceCriterion = typeof WorkflowBoardAcceptanceCriterion.Type;

export const WorkflowBoardSpecDocument = Schema.Struct({
  title: TrimmedNonEmptyString,
  brief: Schema.String,
  sourceType: WorkflowBoardSpecSourceType,
  assistanceMode: WorkflowBoardAssistanceMode,
  acceptanceCriteria: Schema.Array(WorkflowBoardAcceptanceCriterion),
  constraints: Schema.Array(TrimmedNonEmptyString),
  sourceRefs: Schema.Array(TrimmedNonEmptyString),
  wireframesIndex: Schema.NullOr(WorkflowBoardArtifactKey),
  specDigest: Schema.NullOr(TrimmedNonEmptyString),
  updatedAt: IsoDateTime,
  intakeGoal: Schema.optional(Schema.NullOr(WorkflowBoardIntakeGoal)),
  briefPath: Schema.optional(Schema.NullOr(WorkflowBoardArtifactKey)),
  briefDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  sourceDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  sourceInputDigests: Schema.optional(Schema.Array(WorkflowBoardSourceInputDigest)),
  intakeAnalysis: Schema.optional(Schema.NullOr(WorkflowBoardSpecIntakeAnalysis)),
  wireframesDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
});
export type WorkflowBoardSpecDocument = typeof WorkflowBoardSpecDocument.Type;

export const WorkflowBoardApprovalReceipt = Schema.Struct({
  id: WorkflowBoardApprovalId,
  gate: WorkflowBoardApprovalGate,
  status: WorkflowBoardApprovalStatus,
  revision: PositiveInt,
  artifactDigest: TrimmedNonEmptyString,
  specDigest: Schema.NullOr(TrimmedNonEmptyString),
  wireframesDigest: Schema.NullOr(TrimmedNonEmptyString),
  approvedAt: IsoDateTime,
  approvedBy: TrimmedNonEmptyString,
  planDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  tasksDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  checksDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  acceptanceCriteriaIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  productSnapshotDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
});
export type WorkflowBoardApprovalReceipt = typeof WorkflowBoardApprovalReceipt.Type;

export const WorkflowBoardStageRunLink = Schema.Struct({
  id: WorkflowBoardStageRunId,
  stage: WorkflowBoardStageRunStage,
  taskId: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  attempt: Schema.optional(PositiveInt),
  stableLaunchKey: TrimmedNonEmptyString,
  status: WorkflowBoardStageRunStatus,
  cardRevision: PositiveInt,
  runId: Schema.NullOr(RunId),
  threadId: Schema.NullOr(ThreadId),
  projectId: ProjectId,
  workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
  worktreePath: Schema.NullOr(TrimmedNonEmptyString),
  branch: Schema.NullOr(TrimmedNonEmptyString),
  modelSelection: ModelSelection,
  runtimeMode: RuntimeMode,
  interactionMode: ProviderInteractionMode,
  lastError: Schema.NullOr(Schema.String),
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  endedAt: Schema.NullOr(IsoDateTime),
});
export type WorkflowBoardStageRunLink = typeof WorkflowBoardStageRunLink.Type;

export const WorkflowBoardRevisionEntry = Schema.Struct({
  revision: PositiveInt,
  stage: WorkflowBoardStage,
  kind: TrimmedNonEmptyString,
  summary: Schema.String,
  specDigest: Schema.NullOr(TrimmedNonEmptyString),
  createdAt: IsoDateTime,
  planDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
});
export type WorkflowBoardRevisionEntry = typeof WorkflowBoardRevisionEntry.Type;

/**
 * Compact card summary streamed by `workflowBoard.subscribe` without heavy markdown bodies,
 * diffs, or transcripts.
 */
export const WorkflowBoardCardSummary = Schema.Struct({
  cardId: WorkflowCardId,
  projectId: ProjectId,
  title: TrimmedNonEmptyString,
  stage: WorkflowBoardStage,
  status: WorkflowBoardCardStatus,
  revision: PositiveInt,
  currentTask: Schema.NullOr(TrimmedNonEmptyString),
  blockerSummary: Schema.NullOr(TrimmedNonEmptyString),
  nextAction: Schema.NullOr(TrimmedNonEmptyString),
  specApprovedRevision: Schema.NullOr(PositiveInt),
  specApprovalStatus: Schema.NullOr(WorkflowBoardApprovalStatus),
  activeThreadId: Schema.NullOr(ThreadId),
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  planApprovedRevision: Schema.optional(Schema.NullOr(PositiveInt)),
  planApprovalStatus: Schema.optional(Schema.NullOr(WorkflowBoardApprovalStatus)),
  incompleteJournalCount: Schema.optional(NonNegativeInt),
  hasUnreconciledEdits: Schema.optional(Schema.Boolean),
  deliveryStatus: Schema.optional(Schema.NullOr(WorkflowBoardDeliveryStatus)),
});
export type WorkflowBoardCardSummary = typeof WorkflowBoardCardSummary.Type;

export const ProjectWorkflowBoardSnapshot = Schema.Struct({
  projectId: ProjectId,
  revision: NonNegativeInt,
  cards: Schema.Array(WorkflowBoardCardSummary),
  generatedAt: IsoDateTime,
});
export type ProjectWorkflowBoardSnapshot = typeof ProjectWorkflowBoardSnapshot.Type;

export const WorkflowBoardDeliveryReceiptStatus = Schema.Literals([
  "shipped",
  "valid",
  "stale",
  "superseded",
  "historical",
]);
export type WorkflowBoardDeliveryReceiptStatus = typeof WorkflowBoardDeliveryReceiptStatus.Type;

export const WorkflowBoardDeliveryReceipt = Schema.Struct({
  id: TrimmedNonEmptyString,
  approvalId: Schema.NullOr(WorkflowBoardApprovalId),
  cardRevision: PositiveInt,
  productSnapshotDigest: TrimmedNonEmptyString,
  commitSha: Schema.NullOr(TrimmedNonEmptyString),
  specDigest: Schema.NullOr(TrimmedNonEmptyString),
  planDigest: Schema.NullOr(TrimmedNonEmptyString),
  tasksDigest: Schema.NullOr(TrimmedNonEmptyString),
  checksDigest: Schema.NullOr(TrimmedNonEmptyString),
  reviewId: Schema.NullOr(TrimmedNonEmptyString),
  acVerdictsSummary: Schema.String,
  approvedBy: TrimmedNonEmptyString,
  approvedAt: IsoDateTime,
  status: WorkflowBoardDeliveryReceiptStatus,
});
export type WorkflowBoardDeliveryReceipt = typeof WorkflowBoardDeliveryReceipt.Type;

export const WorkflowBoardShipReadiness = Schema.Struct({
  status: WorkflowBoardDeliveryStatus,
  ready: Schema.Boolean,
  productSnapshotDigest: Schema.NullOr(TrimmedNonEmptyString),
  blockers: Schema.Array(WorkflowBoardShipBlocker),
  deliveryReceipt: Schema.NullOr(WorkflowBoardDeliveryReceipt),
});
export type WorkflowBoardShipReadiness = typeof WorkflowBoardShipReadiness.Type;

export const WorkflowCardDetail = Schema.Struct({
  ...WorkflowBoardCardSummary.fields,
  workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
  modelSelection: Schema.NullOr(ModelSelection),
  runtimeMode: Schema.NullOr(RuntimeMode),
  interactionMode: Schema.NullOr(ProviderInteractionMode),
  spec: WorkflowBoardSpecDocument,
  specApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  stageRuns: Schema.Array(WorkflowBoardStageRunLink),
  revisions: Schema.Array(WorkflowBoardRevisionEntry),
  artifactKeys: Schema.Array(WorkflowBoardArtifactKey),
  wireframesIndex: Schema.optional(Schema.NullOr(WorkflowBoardArtifactKey)),
  wireframesDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  exploration: Schema.optional(Schema.NullOr(WorkflowBoardExplorationState)),
  challengeReceipt: Schema.optional(Schema.NullOr(WorkflowBoardChallengeReceipt)),
  maxChallengeConcerns: Schema.optional(NonNegativeInt),
  plan: Schema.optional(Schema.NullOr(WorkflowBoardPlanDocument)),
  planApproval: Schema.optional(Schema.NullOr(WorkflowBoardApprovalReceipt)),
  lenses: Schema.optional(Schema.Array(WorkflowBoardPlanLensEntry)),
  alternatives: Schema.optional(Schema.Array(WorkflowBoardPlanAlternative)),
  alternativesRevisionCount: Schema.optional(NonNegativeInt),
  selectedAlternativeId: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  selectionRationale: Schema.optional(Schema.NullOr(Schema.String)),
  strategy: Schema.optional(Schema.NullOr(WorkflowBoardPlanningStrategy)),
  strategyRationale: Schema.optional(Schema.NullOr(Schema.String)),
  strategyMetadata: Schema.optional(Schema.NullOr(WorkflowBoardStrategyMetadata)),
  granularity: Schema.optional(Schema.NullOr(WorkflowBoardTaskGranularity)),
  tasks: Schema.optional(Schema.Array(WorkflowBoardTaskContract)),
  checks: Schema.optional(Schema.Array(WorkflowBoardPlannedCheck)),
  acMappings: Schema.optional(Schema.Array(WorkflowBoardAcEvidenceMapping)),
  history: Schema.optional(Schema.Array(WorkflowBoardRevisionEntry)),
  taskExecutions: Schema.optional(Schema.Array(WorkflowBoardTaskExecutionState)),
  fileContractReports: Schema.optional(Schema.Array(WorkflowBoardTaskFileContractReport)),
  checkReceipts: Schema.optional(Schema.Array(WorkflowBoardPlannedCheckReceipt)),
  reviewReport: Schema.optional(Schema.NullOr(WorkflowBoardReviewReport)),
  acObservations: Schema.optional(Schema.Array(WorkflowBoardAcObservationRecord)),
  shipReadiness: Schema.optional(Schema.NullOr(WorkflowBoardShipReadiness)),
  deliveryReceipt: Schema.optional(Schema.NullOr(WorkflowBoardDeliveryReceipt)),
  recoveryState: Schema.optional(Schema.NullOr(WorkflowBoardRecoveryState)),
  amendments: Schema.optional(Schema.Array(WorkflowBoardPlanAmendmentRecord)),
  cliImport: Schema.optional(WorkflowBoardCliImportProvenance),
});
export type WorkflowCardDetail = typeof WorkflowCardDetail.Type;

export const WorkflowBoardArtifact = Schema.Struct({
  cardId: WorkflowCardId,
  key: WorkflowBoardArtifactKey,
  kind: WorkflowBoardArtifactKind,
  relativePath: WorkflowBoardArtifactKey,
  digest: TrimmedNonEmptyString,
  content: Schema.String,
  revision: PositiveInt,
  updatedAt: IsoDateTime,
});
export type WorkflowBoardArtifact = typeof WorkflowBoardArtifact.Type;

export const WorkflowBoardSubscribeInput = Schema.Struct({
  projectId: ProjectId,
});
export type WorkflowBoardSubscribeInput = typeof WorkflowBoardSubscribeInput.Type;

export const WorkflowBoardGetCardInput = Schema.Struct({
  cardId: WorkflowCardId,
  projectId: Schema.optional(ProjectId),
});
export type WorkflowBoardGetCardInput = typeof WorkflowBoardGetCardInput.Type;

export const WorkflowBoardGetArtifactInput = Schema.Struct({
  cardId: WorkflowCardId,
  key: Schema.optional(WorkflowBoardArtifactKey),
  kind: Schema.optional(WorkflowBoardArtifactKind),
}).check(
  Schema.makeFilter((input) => input.key !== undefined || input.kind !== undefined, {
    expected: "at least one of `key` or `kind`",
  }),
);
export type WorkflowBoardGetArtifactInput = typeof WorkflowBoardGetArtifactInput.Type;

export const WorkflowBoardCreateCardInput = Schema.Struct({
  projectId: ProjectId,
  title: TrimmedNonEmptyString,
  brief: Schema.optional(TrimmedNonEmptyString),
  briefPath: Schema.optional(WorkflowBoardSafeInputPath),
  inputPaths: Schema.optional(Schema.Array(WorkflowBoardSafeInputPath)),
  intakeGoal: Schema.optional(WorkflowBoardIntakeGoal),
  wireframesEnabled: Schema.optional(Schema.Boolean),
  wireframesIndex: Schema.optional(Schema.NullOr(WorkflowBoardNonGitArtifactKey)),
  sourceType: Schema.optional(WorkflowBoardSpecSourceType),
  assistanceMode: Schema.optional(WorkflowBoardAssistanceMode),
  workspaceStrategy: Schema.optional(OrchestrationV2ThreadLaunchWorkspaceStrategy),
  modelSelection: Schema.optional(ModelSelection),
  runtimeMode: Schema.optional(RuntimeMode),
  interactionMode: Schema.optional(ProviderInteractionMode),
});
export type WorkflowBoardCreateCardInput = typeof WorkflowBoardCreateCardInput.Type;

export const WorkflowBoardUpdateSpecInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  title: Schema.optional(TrimmedNonEmptyString),
  brief: Schema.optional(Schema.String),
  intakeGoal: Schema.optional(WorkflowBoardIntakeGoal),
  inputPaths: Schema.optional(Schema.Array(WorkflowBoardSafeInputPath)),
  acceptanceCriteria: Schema.optional(Schema.Array(WorkflowBoardAcceptanceCriterion)),
  constraints: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  sourceRefs: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  wireframesIndex: Schema.optional(Schema.NullOr(WorkflowBoardNonGitArtifactKey)),
});
export type WorkflowBoardUpdateSpecInput = typeof WorkflowBoardUpdateSpecInput.Type;

export const WorkflowBoardUpdateSpecResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  specDigest: Schema.NullOr(TrimmedNonEmptyString),
  specApprovalStatus: Schema.NullOr(WorkflowBoardApprovalStatus),
  card: WorkflowCardDetail,
  diff: Schema.optional(Schema.NullOr(WorkflowBoardSpecDiff)),
  wireframesIndex: Schema.optional(Schema.NullOr(WorkflowBoardArtifactKey)),
  specApproval: Schema.optional(Schema.NullOr(WorkflowBoardApprovalReceipt)),
  planApproval: Schema.optional(Schema.NullOr(WorkflowBoardApprovalReceipt)),
});
export type WorkflowBoardUpdateSpecResult = typeof WorkflowBoardUpdateSpecResult.Type;

export const WorkflowBoardApproveSpecInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  expectedDigest: Schema.optional(TrimmedNonEmptyString),
});
export type WorkflowBoardApproveSpecInput = typeof WorkflowBoardApproveSpecInput.Type;

export const WorkflowBoardApproveSpecResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  specApproval: WorkflowBoardApprovalReceipt,
  card: WorkflowCardDetail,
});
export type WorkflowBoardApproveSpecResult = typeof WorkflowBoardApproveSpecResult.Type;

export const WorkflowBoardStartStageInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  stage: WorkflowBoardStageRunStage,
  taskId: Schema.optional(TrimmedNonEmptyString),
  stableLaunchKey: Schema.optional(TrimmedNonEmptyString),
  modelSelection: Schema.optional(ModelSelection),
  runtimeMode: Schema.optional(RuntimeMode),
  interactionMode: Schema.optional(ProviderInteractionMode),
  workspaceStrategy: Schema.optional(OrchestrationV2ThreadLaunchWorkspaceStrategy),
  acknowledgeUnsupportedPolicies: Schema.optional(Schema.Boolean),
  acknowledgedPolicies: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
});
export type WorkflowBoardStartStageInput = typeof WorkflowBoardStartStageInput.Type;

export const WorkflowBoardStartStageResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  stage: WorkflowBoardStageRunStage,
  taskId: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  stageRunId: WorkflowBoardStageRunId,
  runId: Schema.NullOr(RunId),
  threadId: ThreadId,
  resumed: Schema.Boolean,
});
export type WorkflowBoardStartStageResult = typeof WorkflowBoardStartStageResult.Type;

export const WorkflowBoardExploreStepInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  action: Schema.optional(WorkflowBoardExploreStepAction),
  continueRound: Schema.optional(Schema.Boolean),
  answers: Schema.optional(WorkflowBoardExploreAnswersArray),
});
export type WorkflowBoardExploreStepInput = typeof WorkflowBoardExploreStepInput.Type;

export const WorkflowBoardExploreStepResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  roundIndex: NonNegativeInt,
  pendingRoundPrompt: Schema.Boolean,
  paused: Schema.Boolean,
  exploration: WorkflowBoardExplorationState,
  decisions: Schema.Array(WorkflowBoardExplorationDecision),
  card: WorkflowCardDetail,
});
export type WorkflowBoardExploreStepResult = typeof WorkflowBoardExploreStepResult.Type;

export const WorkflowBoardChallengeStepInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  action: WorkflowBoardChallengeStepAction,
  concerns: Schema.optional(WorkflowBoardChallengeConcernsArray),
  acknowledgementToken: Schema.optional(Schema.String),
  unresolvedConcernIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  answers: Schema.optional(
    Schema.Union([
      Schema.Array(TrimmedNonEmptyString),
      Schema.Array(WorkflowBoardChallengeConcernResponse),
    ]),
  ),
  approved: Schema.optional(Schema.Boolean),
}).check(
  Schema.makeFilter(
    (input) => {
      if (input.approved === true) {
        return false;
      }
      if (input.action === "manualFallback") {
        if (input.acknowledgementToken !== "I acknowledge") {
          return false;
        }
        if ((input.unresolvedConcernIds?.length ?? 0) > 0) {
          return false;
        }
        if (
          !Array.isArray(input.answers) ||
          input.answers.length !== 3 ||
          !input.answers.every((entry) => typeof entry === "string" && entry.trim().length > 0)
        ) {
          return false;
        }
      }
      return true;
    },
    {
      expected:
        "a valid challenge step (approved cannot be true; manualFallback requires exact 'I acknowledge', empty unresolvedConcernIds, and 3 non-empty answers)",
    },
  ),
);
export type WorkflowBoardChallengeStepInput = typeof WorkflowBoardChallengeStepInput.Type;

export const WorkflowBoardChallengeStepResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  challengeReceipt: WorkflowBoardChallengeReceipt,
  concerns: Schema.Array(WorkflowBoardChallengeConcern),
  card: WorkflowCardDetail,
});
export type WorkflowBoardChallengeStepResult = typeof WorkflowBoardChallengeStepResult.Type;

export const WorkflowBoardUpdatePlanInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  lenses: Schema.optional(WorkflowBoardPlanLensesArray),
  alternatives: Schema.optional(WorkflowBoardPlanAlternativesPair),
  selectedAlternativeId: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  selectionRationale: Schema.optional(Schema.String),
  reviseAlternativesFeedback: Schema.optional(TrimmedNonEmptyString),
  strategy: Schema.optional(WorkflowBoardPlanningStrategy),
  strategyRationale: Schema.optional(Schema.String),
  strategyMetadata: Schema.optional(WorkflowBoardStrategyMetadata),
  granularity: Schema.optional(WorkflowBoardTaskGranularity),
  tasks: Schema.optional(WorkflowBoardTaskContractsArray),
  checks: Schema.optional(Schema.Array(WorkflowBoardPlannedCheck)),
  acMappings: Schema.optional(WorkflowBoardAcEvidenceMappingsArray),
}).check(
  Schema.makeFilter(
    (input) => {
      if (typeof input.selectedAlternativeId === "string") {
        if (
          input.selectionRationale === undefined ||
          input.selectionRationale.trim().length === 0
        ) {
          return false;
        }
        if (
          input.alternatives !== undefined &&
          !input.alternatives.some((alt) => alt.id.trim() === input.selectedAlternativeId?.trim())
        ) {
          return false;
        }
      }
      if (input.strategy === "custom") {
        if (input.strategyRationale === undefined || input.strategyRationale.trim().length === 0) {
          return false;
        }
      }
      if (input.strategy === "risk-first") {
        if (
          input.strategyMetadata?.uncertainty === undefined ||
          input.strategyMetadata?.probeTaskId === undefined
        ) {
          return false;
        }
      }
      if (input.strategy === "expand-contract") {
        if (
          input.strategyMetadata?.compatibilityWindow === undefined ||
          input.strategyMetadata?.transitionStep === undefined ||
          input.strategyMetadata?.retirementPrereq === undefined
        ) {
          return false;
        }
      }
      return true;
    },
    {
      expected:
        "a valid plan update (selectedAlternativeId requires non-empty selectionRationale and matching alternative; custom/risk-first/expand-contract require strategy fields)",
    },
  ),
);
export type WorkflowBoardUpdatePlanInput = typeof WorkflowBoardUpdatePlanInput.Type;

export const WorkflowBoardUpdatePlanResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  planDigest: Schema.NullOr(TrimmedNonEmptyString),
  specApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  planApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  lenses: Schema.Array(WorkflowBoardPlanLensEntry),
  alternatives: Schema.Array(WorkflowBoardPlanAlternative),
  selectedAlternativeId: Schema.NullOr(TrimmedNonEmptyString),
  selectionRationale: Schema.NullOr(Schema.String),
  strategy: Schema.NullOr(WorkflowBoardPlanningStrategy),
  strategyMetadata: Schema.NullOr(WorkflowBoardStrategyMetadata),
  granularity: Schema.NullOr(WorkflowBoardTaskGranularity),
  tasks: Schema.Array(WorkflowBoardTaskContract),
  acMappings: Schema.Array(WorkflowBoardAcEvidenceMapping),
  card: WorkflowCardDetail,
});
export type WorkflowBoardUpdatePlanResult = typeof WorkflowBoardUpdatePlanResult.Type;

export const WorkflowBoardApprovePlanInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  expectedSpecDigest: Schema.optional(TrimmedNonEmptyString),
  expectedPlanDigest: Schema.optional(TrimmedNonEmptyString),
});
export type WorkflowBoardApprovePlanInput = typeof WorkflowBoardApprovePlanInput.Type;

export const WorkflowBoardApprovePlanResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  planApproval: WorkflowBoardApprovalReceipt,
  card: WorkflowCardDetail,
});
export type WorkflowBoardApprovePlanResult = typeof WorkflowBoardApprovePlanResult.Type;

export const WorkflowBoardReturnForRevisionInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  targetStage: WorkflowBoardReturnTargetStage,
  reason: TrimmedNonEmptyString,
});
export type WorkflowBoardReturnForRevisionInput = typeof WorkflowBoardReturnForRevisionInput.Type;

export const WorkflowBoardReturnForRevisionResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  stage: WorkflowBoardStage,
  specApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  planApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  history: Schema.Array(WorkflowBoardRevisionEntry),
  card: WorkflowCardDetail,
});
export type WorkflowBoardReturnForRevisionResult = typeof WorkflowBoardReturnForRevisionResult.Type;

export const WorkflowBoardGetCardDiffInput = Schema.Struct({
  cardId: WorkflowCardId,
  taskId: Schema.optional(TrimmedNonEmptyString),
  includePatches: Schema.optional(Schema.Boolean),
});
export type WorkflowBoardGetCardDiffInput = typeof WorkflowBoardGetCardDiffInput.Type;

export const WorkflowBoardGetCardDiffResult = Schema.Struct({
  cardId: WorkflowCardId,
  worktreePath: Schema.NullOr(TrimmedNonEmptyString),
  baseRef: Schema.NullOr(TrimmedNonEmptyString),
  productSnapshotDigest: Schema.NullOr(TrimmedNonEmptyString),
  diffDigest: Schema.NullOr(TrimmedNonEmptyString),
  files: Schema.Array(WorkflowBoardDiffFileEntry),
  fileContractReport: Schema.optional(Schema.NullOr(WorkflowBoardTaskFileContractReport)),
  truncated: Schema.Boolean,
  generatedAt: IsoDateTime,
});
export type WorkflowBoardGetCardDiffResult = typeof WorkflowBoardGetCardDiffResult.Type;

export const WorkflowBoardPauseCardInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  reason: Schema.optional(TrimmedNonEmptyString),
});
export type WorkflowBoardPauseCardInput = typeof WorkflowBoardPauseCardInput.Type;

export const WorkflowBoardPauseCardResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  status: WorkflowBoardCardStatus,
  card: WorkflowCardDetail,
});
export type WorkflowBoardPauseCardResult = typeof WorkflowBoardPauseCardResult.Type;

export const WorkflowBoardResumeCardInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  reconcileInterrupted: Schema.optional(Schema.Boolean),
  note: Schema.optional(TrimmedNonEmptyString),
});
export type WorkflowBoardResumeCardInput = typeof WorkflowBoardResumeCardInput.Type;

export const WorkflowBoardResumeCardResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  status: WorkflowBoardCardStatus,
  reconciledCount: NonNegativeInt,
  card: WorkflowCardDetail,
});
export type WorkflowBoardResumeCardResult = typeof WorkflowBoardResumeCardResult.Type;

export const WorkflowBoardRetryStepInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  stepKind: WorkflowBoardRetryStepKind,
  targetId: Schema.optional(TrimmedNonEmptyString),
  reason: Schema.optional(TrimmedNonEmptyString),
}).check(
  Schema.makeFilter(
    (input) => {
      if (input.stepKind === "task" || input.stepKind === "check") {
        return input.targetId !== undefined && input.targetId.trim().length > 0;
      }
      return true;
    },
    {
      expected: "a retry step with non-empty `targetId` when `stepKind` is `'task'` or `'check'`",
    },
  ),
);
export type WorkflowBoardRetryStepInput = typeof WorkflowBoardRetryStepInput.Type;

export const WorkflowBoardRetryStepResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  stepKind: WorkflowBoardRetryStepKind,
  targetId: Schema.NullOr(TrimmedNonEmptyString),
  card: WorkflowCardDetail,
});
export type WorkflowBoardRetryStepResult = typeof WorkflowBoardRetryStepResult.Type;

export const WorkflowBoardProposeAmendmentInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  targetStage: Schema.optional(WorkflowBoardAmendmentTargetStage),
  reason: TrimmedNonEmptyString,
  summary: Schema.optional(TrimmedNonEmptyString),
  sourceTaskId: Schema.optional(TrimmedNonEmptyString),
  unexpectedFiles: Schema.optional(Schema.Array(WorkflowBoardArtifactKey)),
  tasks: Schema.optional(WorkflowBoardTaskContractsArray),
  plannedChecks: Schema.optional(Schema.Array(WorkflowBoardPlannedCheck)),
  acMappings: Schema.optional(WorkflowBoardAcEvidenceMappingsArray),
  deliverables: Schema.optional(Schema.Array(WorkflowBoardArtifactKey)),
});
export type WorkflowBoardProposeAmendmentInput = typeof WorkflowBoardProposeAmendmentInput.Type;

export const WorkflowBoardProposeAmendmentResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  stage: WorkflowBoardStage,
  amendment: WorkflowBoardPlanAmendmentRecord,
  specApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  planApproval: Schema.NullOr(WorkflowBoardApprovalReceipt),
  card: WorkflowCardDetail,
});
export type WorkflowBoardProposeAmendmentResult = typeof WorkflowBoardProposeAmendmentResult.Type;

export const WorkflowBoardResolveFindingInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  reviewId: Schema.optional(TrimmedNonEmptyString),
  findingId: TrimmedNonEmptyString,
  disposition: WorkflowBoardFindingResolveDisposition,
  reason: Schema.optional(TrimmedNonEmptyString),
}).check(
  Schema.makeFilter(
    (input) => {
      if (input.disposition === "dismiss" || input.disposition === "accept_risk") {
        return input.reason !== undefined && input.reason.trim().length > 0;
      }
      return true;
    },
    {
      expected:
        "a finding resolution with non-empty `reason` when `disposition` is `'dismiss'` or `'accept_risk'`",
    },
  ),
);
export type WorkflowBoardResolveFindingInput = typeof WorkflowBoardResolveFindingInput.Type;

export const WorkflowBoardResolveFindingResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  finding: WorkflowBoardReviewFinding,
  reviewReport: WorkflowBoardReviewReport,
  card: WorkflowCardDetail,
});
export type WorkflowBoardResolveFindingResult = typeof WorkflowBoardResolveFindingResult.Type;

export const WorkflowBoardRecordManualAcceptanceInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  criterionId: TrimmedNonEmptyString,
  verdict: WorkflowBoardManualAcceptanceVerdict,
  note: Schema.optional(TrimmedNonEmptyString),
  evidenceRef: Schema.optional(TrimmedNonEmptyString),
  evidenceDigest: Schema.optional(TrimmedNonEmptyString),
  expectedProductSnapshotDigest: Schema.optional(TrimmedNonEmptyString),
}).check(
  Schema.makeFilter(
    (input) => {
      if (input.verdict === "fail" || input.verdict === "not-applicable") {
        return input.note !== undefined && input.note.trim().length > 0;
      }
      return true;
    },
    {
      expected:
        "a manual acceptance verdict with non-empty `note` when `verdict` is `'fail'` or `'not-applicable'`",
    },
  ),
);
export type WorkflowBoardRecordManualAcceptanceInput =
  typeof WorkflowBoardRecordManualAcceptanceInput.Type;

export const WorkflowBoardRecordManualAcceptanceResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  observation: WorkflowBoardAcObservationRecord,
  shipReadiness: WorkflowBoardShipReadiness,
  card: WorkflowCardDetail,
});
export type WorkflowBoardRecordManualAcceptanceResult =
  typeof WorkflowBoardRecordManualAcceptanceResult.Type;

export const WorkflowBoardApproveDeliveryInput = Schema.Struct({
  cardId: WorkflowCardId,
  expectedRevision: PositiveInt,
  expectedProductSnapshotDigest: Schema.optional(TrimmedNonEmptyString),
  note: Schema.optional(TrimmedNonEmptyString),
});
export type WorkflowBoardApproveDeliveryInput = typeof WorkflowBoardApproveDeliveryInput.Type;

export const WorkflowBoardApproveDeliveryResult = Schema.Struct({
  cardId: WorkflowCardId,
  revision: PositiveInt,
  status: WorkflowBoardCardStatus,
  deliveryStatus: WorkflowBoardDeliveryStatus,
  deliveryReceipt: WorkflowBoardDeliveryReceipt,
  deliveryApproval: WorkflowBoardApprovalReceipt,
  card: WorkflowCardDetail,
});
export type WorkflowBoardApproveDeliveryResult = typeof WorkflowBoardApproveDeliveryResult.Type;

export { WorkflowBoardPreviewCliImportInput, WorkflowBoardApplyCliImportInput };

export const WorkflowBoardPreviewCliImportResult = Schema.Struct({
  projectId: ProjectId,
  sourcePath: WorkflowBoardArtifactKey,
  specId: TrimmedNonEmptyString,
  title: TrimmedNonEmptyString,
  stage: WorkflowBoardStage,
  status: WorkflowBoardCardStatus,
  existingCardId: Schema.NullOr(WorkflowCardId),
  alreadyImported: Schema.Boolean,
  activeLocks: Schema.Array(WorkflowBoardCliImportLockEntry),
  staleLocks: Schema.Array(WorkflowBoardCliImportLockEntry),
  canApply: Schema.Boolean,
  refusalReason: Schema.NullOr(Schema.String),
  tasks: Schema.Array(WorkflowBoardCliImportTaskPreview),
  decisionIds: Schema.Array(TrimmedNonEmptyString),
  assumptionIds: Schema.Array(TrimmedNonEmptyString),
  acceptanceCriterionIds: Schema.Array(TrimmedNonEmptyString),
  historicalReceipts: Schema.Array(WorkflowBoardCliImportHistoricalReceipt),
  unsupportedPolicies: Schema.Array(WorkflowBoardCliImportUnsupportedPolicy),
  missingLogNotes: Schema.Array(WorkflowBoardCliImportMissingLogNote),
  warnings: Schema.Array(Schema.String),
});
export type WorkflowBoardPreviewCliImportResult = typeof WorkflowBoardPreviewCliImportResult.Type;

export const WorkflowBoardApplyCliImportResult = Schema.Struct({
  cardId: WorkflowCardId,
  projectId: ProjectId,
  revision: PositiveInt,
  created: Schema.Boolean,
  sourceSpecId: TrimmedNonEmptyString,
  tasks: Schema.Array(WorkflowBoardTaskContract),
  historicalReceipts: Schema.Array(WorkflowBoardCliImportHistoricalReceipt),
  unsupportedPolicies: Schema.Array(WorkflowBoardCliImportUnsupportedPolicy),
  missingLogNotes: Schema.Array(WorkflowBoardCliImportMissingLogNote),
  card: WorkflowCardDetail,
});
export type WorkflowBoardApplyCliImportResult = typeof WorkflowBoardApplyCliImportResult.Type;

export const WorkflowBoardRequestErrorCode = Schema.Literals([
  "card_not_found",
  "project_not_found",
  "revision_conflict",
  "invalid_stage_transition",
  "spec_not_approved",
  "plan_not_approved",
  "exploration_unresolved",
  "challenge_unresolved",
  "external_edit_detected",
  "contract_conflict",
  "digest_mismatch",
  "invalid_artifact_key",
  "artifact_not_found",
  "invalid_input",
  "launch_failed",
  "internal_error",
]);
export type WorkflowBoardRequestErrorCode = typeof WorkflowBoardRequestErrorCode.Type;

export class WorkflowBoardRequestError extends Schema.TaggedError<WorkflowBoardRequestError>()(
  "WorkflowBoardRequestError",
  {
    code: WorkflowBoardRequestErrorCode,
    message: Schema.String,
    currentRevision: Schema.optional(PositiveInt),
  },
) {}
