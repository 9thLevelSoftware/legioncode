import * as Schema from "effect/Schema";

import {
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  RunId,
  ThreadId,
  TrimmedNonEmptyString,
} from "./baseSchemas.ts";
import { ReviewDiffChangeType } from "./review.ts";
import {
  WorkflowBoardArtifactKey,
  WorkflowBoardEvidenceMethod,
  WorkflowBoardTaskStatus,
} from "./workflowBoardPlanning.ts";

export const WorkflowBoardStageRunStage = Schema.Literals([
  "spec",
  "plan",
  "execute",
  "review",
  "ship",
]);
export type WorkflowBoardStageRunStage = typeof WorkflowBoardStageRunStage.Type;

export const WorkflowBoardDeliveryStatus = Schema.Literals([
  "pending",
  "blocked",
  "ready",
  "shipped",
]);
export type WorkflowBoardDeliveryStatus = typeof WorkflowBoardDeliveryStatus.Type;

export const WorkflowBoardFileContractStatus = Schema.Literals([
  "clean",
  "violation_pending",
  "resolved",
  "amended",
  "stale",
]);
export type WorkflowBoardFileContractStatus = typeof WorkflowBoardFileContractStatus.Type;

export const WorkflowBoardFileContractResolution = Schema.Literals([
  "amend_plan",
  "accept_scope",
  "manual_reverted",
  "dismiss",
]);
export type WorkflowBoardFileContractResolution = typeof WorkflowBoardFileContractResolution.Type;

export const WorkflowBoardWriteJournalOperation = Schema.Literals(["write", "delete"]);
export type WorkflowBoardWriteJournalOperation = typeof WorkflowBoardWriteJournalOperation.Type;

export const WorkflowBoardWriteJournalStatus = Schema.Literals([
  "pending",
  "staged",
  "file_written",
  "committed",
  "rolled_back",
  "failed",
]);
export type WorkflowBoardWriteJournalStatus = typeof WorkflowBoardWriteJournalStatus.Type;

export const WorkflowBoardExternalEditStatus = Schema.Literals([
  "detected",
  "reconciled",
  "dismissed",
]);
export type WorkflowBoardExternalEditStatus = typeof WorkflowBoardExternalEditStatus.Type;

export const WorkflowBoardCheckStatus = Schema.Literals([
  "pending",
  "running",
  "passed",
  "failed",
  "stale",
  "historical",
]);
export type WorkflowBoardCheckStatus = typeof WorkflowBoardCheckStatus.Type;

export const WorkflowBoardReviewSeverity = Schema.Literals(["critical", "major", "minor", "info"]);
export type WorkflowBoardReviewSeverity = typeof WorkflowBoardReviewSeverity.Type;

export const WorkflowBoardReviewStatus = Schema.Literals([
  "pending",
  "running",
  "completed",
  "failed",
  "stale",
  "historical",
]);
export type WorkflowBoardReviewStatus = typeof WorkflowBoardReviewStatus.Type;

export const WorkflowBoardReviewVerdict = Schema.Literals([
  "pending",
  "pass",
  "findings",
  "failed",
  "stale",
]);
export type WorkflowBoardReviewVerdict = typeof WorkflowBoardReviewVerdict.Type;

export const WorkflowBoardReviewFindingStatus = Schema.Literals([
  "open",
  "resolved",
  "stale",
  "historical",
]);
export type WorkflowBoardReviewFindingStatus = typeof WorkflowBoardReviewFindingStatus.Type;

export const WorkflowBoardFindingDisposition = Schema.Literals([
  "unresolved",
  "fix",
  "accept_risk",
  "dismiss",
  "defer",
]);
export type WorkflowBoardFindingDisposition = typeof WorkflowBoardFindingDisposition.Type;

export const WorkflowBoardFindingResolveDisposition = Schema.Literals([
  "fix",
  "accept_risk",
  "dismiss",
  "defer",
]);
export type WorkflowBoardFindingResolveDisposition =
  typeof WorkflowBoardFindingResolveDisposition.Type;

export const WorkflowBoardAcEvidenceStatus = Schema.Literals([
  "pending",
  "pass",
  "fail",
  "not_applicable",
  "stale",
  "historical",
]);
export type WorkflowBoardAcEvidenceStatus = typeof WorkflowBoardAcEvidenceStatus.Type;

export const WorkflowBoardManualAcceptanceVerdict = Schema.Literals([
  "pass",
  "fail",
  "not-applicable",
]);
export type WorkflowBoardManualAcceptanceVerdict = typeof WorkflowBoardManualAcceptanceVerdict.Type;

export const WorkflowBoardShipBlockerKind = Schema.Literals([
  "task_incomplete",
  "contract_violation",
  "check_failed",
  "check_stale",
  "check_missing",
  "review_unresolved",
  "review_stale",
  "ac_unverified",
  "external_edit",
  "journal_incomplete",
  "plan_stale",
  "spec_stale",
  "policy_unacknowledged",
]);
export type WorkflowBoardShipBlockerKind = typeof WorkflowBoardShipBlockerKind.Type;

export const WorkflowBoardRetryStepKind = Schema.Literals(["task", "check", "review", "stage"]);
export type WorkflowBoardRetryStepKind = typeof WorkflowBoardRetryStepKind.Type;

export const WorkflowBoardAmendmentTargetStage = Schema.Literals(["spec", "plan"]);
export type WorkflowBoardAmendmentTargetStage = typeof WorkflowBoardAmendmentTargetStage.Type;

export const WorkflowBoardAmendmentStatus = Schema.Literals([
  "proposed",
  "applied",
  "superseded",
  "rejected",
]);
export type WorkflowBoardAmendmentStatus = typeof WorkflowBoardAmendmentStatus.Type;

export const WorkflowBoardDiffChangeType = ReviewDiffChangeType;
export type WorkflowBoardDiffChangeType = typeof WorkflowBoardDiffChangeType.Type;

export const WorkflowBoardDiffFileContractStatus = Schema.Literals([
  "allowed",
  "unexpected",
  "forbidden",
  "unassigned",
]);
export type WorkflowBoardDiffFileContractStatus = typeof WorkflowBoardDiffFileContractStatus.Type;

export const WorkflowBoardTaskExecutionState = Schema.Struct({
  taskId: TrimmedNonEmptyString,
  status: WorkflowBoardTaskStatus,
  threadId: Schema.NullOr(ThreadId),
  runId: Schema.NullOr(RunId),
  stageRunId: Schema.NullOr(TrimmedNonEmptyString),
  worktreePath: Schema.NullOr(TrimmedNonEmptyString),
  branch: Schema.NullOr(TrimmedNonEmptyString),
  attempt: PositiveInt,
  startedAt: Schema.NullOr(IsoDateTime),
  endedAt: Schema.NullOr(IsoDateTime),
  lastError: Schema.NullOr(Schema.String),
});
export type WorkflowBoardTaskExecutionState = typeof WorkflowBoardTaskExecutionState.Type;

export const WorkflowBoardTaskFileContractReport = Schema.Struct({
  id: Schema.optional(TrimmedNonEmptyString),
  taskId: TrimmedNonEmptyString,
  threadId: Schema.NullOr(ThreadId),
  runId: Schema.NullOr(RunId),
  status: WorkflowBoardFileContractStatus,
  changedFiles: Schema.Array(WorkflowBoardArtifactKey),
  allowedFiles: Schema.Array(WorkflowBoardArtifactKey),
  unexpectedFiles: Schema.Array(WorkflowBoardArtifactKey),
  forbiddenFiles: Schema.Array(WorkflowBoardArtifactKey),
  preservedInWorktree: Schema.Boolean,
  blockingReason: Schema.NullOr(Schema.String),
  resolutionDisposition: Schema.optional(Schema.NullOr(WorkflowBoardFileContractResolution)),
  resolutionNote: Schema.optional(Schema.NullOr(Schema.String)),
  resolvedBy: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  resolvedAt: Schema.optional(Schema.NullOr(IsoDateTime)),
  updatedAt: IsoDateTime,
});
export type WorkflowBoardTaskFileContractReport = typeof WorkflowBoardTaskFileContractReport.Type;

export const WorkflowBoardRecoveryEntry = Schema.Struct({
  id: TrimmedNonEmptyString,
  targetPath: WorkflowBoardArtifactKey,
  operation: WorkflowBoardWriteJournalOperation,
  status: WorkflowBoardWriteJournalStatus,
  expectedDigest: Schema.NullOr(TrimmedNonEmptyString),
  previousDigest: Schema.NullOr(TrimmedNonEmptyString),
  revision: Schema.NullOr(PositiveInt),
  lastError: Schema.NullOr(Schema.String),
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  completedAt: Schema.NullOr(IsoDateTime),
});
export type WorkflowBoardRecoveryEntry = typeof WorkflowBoardRecoveryEntry.Type;

const WorkflowBoardM3ApprovalGate = Schema.Literals(["spec", "plan", "delivery"]);
const WorkflowBoardM3ArtifactKind = Schema.Literals([
  "card",
  "spec",
  "plan",
  "design",
  "wireframes",
]);
const WorkflowBoardM3Stage = Schema.Literals(["spec", "plan", "execute", "ship"]);

export const WorkflowBoardExternalEditRecord = Schema.Struct({
  id: TrimmedNonEmptyString,
  relativePath: WorkflowBoardArtifactKey,
  artifactKind: Schema.optional(Schema.NullOr(WorkflowBoardM3ArtifactKind)),
  expectedDigest: Schema.NullOr(TrimmedNonEmptyString),
  actualDigest: Schema.NullOr(TrimmedNonEmptyString),
  status: WorkflowBoardExternalEditStatus,
  affectedGates: Schema.Array(WorkflowBoardM3ApprovalGate),
  detectedAt: IsoDateTime,
  reconciledAt: Schema.NullOr(IsoDateTime),
  summary: Schema.String,
});
export type WorkflowBoardExternalEditRecord = typeof WorkflowBoardExternalEditRecord.Type;

export const WorkflowBoardRecoveryState = Schema.Struct({
  incompleteJournalCount: NonNegativeInt,
  pendingJournalEntries: Schema.Array(WorkflowBoardRecoveryEntry),
  externalEdits: Schema.Array(WorkflowBoardExternalEditRecord),
  requiresReconciliation: Schema.Boolean,
  lastRecoveredAt: Schema.optional(Schema.NullOr(IsoDateTime)),
});
export type WorkflowBoardRecoveryState = typeof WorkflowBoardRecoveryState.Type;

export const WorkflowBoardPlannedCheckReceipt = Schema.Struct({
  id: Schema.optional(TrimmedNonEmptyString),
  checkId: TrimmedNonEmptyString,
  criterionId: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  ownerTaskId: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  argv: Schema.NonEmptyArray(TrimmedNonEmptyString),
  description: Schema.optional(Schema.NullOr(Schema.String)),
  status: WorkflowBoardCheckStatus,
  exitCode: Schema.NullOr(Schema.Int),
  stdoutExcerpt: Schema.optional(Schema.NullOr(Schema.String)),
  stderrExcerpt: Schema.optional(Schema.NullOr(Schema.String)),
  logRef: Schema.NullOr(TrimmedNonEmptyString),
  productSnapshotDigest: Schema.NullOr(TrimmedNonEmptyString),
  commitSha: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  specDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  planDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  staleReason: Schema.optional(Schema.NullOr(Schema.String)),
  executedAt: Schema.NullOr(IsoDateTime),
  updatedAt: IsoDateTime,
});
export type WorkflowBoardPlannedCheckReceipt = typeof WorkflowBoardPlannedCheckReceipt.Type;

export const WorkflowBoardReviewFinding = Schema.Struct({
  id: TrimmedNonEmptyString,
  severity: WorkflowBoardReviewSeverity,
  status: Schema.optional(WorkflowBoardReviewFindingStatus),
  criterionId: Schema.NullOr(TrimmedNonEmptyString),
  file: Schema.NullOr(WorkflowBoardArtifactKey),
  line: Schema.NullOr(PositiveInt),
  message: TrimmedNonEmptyString,
  disposition: WorkflowBoardFindingDisposition,
  resolutionNote: Schema.NullOr(Schema.String),
  resolvedBy: Schema.NullOr(TrimmedNonEmptyString),
  resolvedAt: Schema.NullOr(IsoDateTime),
});
export type WorkflowBoardReviewFinding = typeof WorkflowBoardReviewFinding.Type;

export const WorkflowBoardReviewReport = Schema.Struct({
  reviewId: TrimmedNonEmptyString,
  stageRunId: Schema.NullOr(TrimmedNonEmptyString),
  threadId: Schema.NullOr(ThreadId),
  status: WorkflowBoardReviewStatus,
  verdict: Schema.optional(WorkflowBoardReviewVerdict),
  summary: Schema.NullOr(Schema.String),
  productSnapshotDigest: Schema.NullOr(TrimmedNonEmptyString),
  staleReason: Schema.optional(Schema.NullOr(Schema.String)),
  findings: Schema.Array(WorkflowBoardReviewFinding),
  createdAt: IsoDateTime,
  updatedAt: IsoDateTime,
  completedAt: Schema.NullOr(IsoDateTime),
});
export type WorkflowBoardReviewReport = typeof WorkflowBoardReviewReport.Type;

export const WorkflowBoardAcObservationRecord = Schema.Struct({
  criterionId: TrimmedNonEmptyString,
  method: WorkflowBoardEvidenceMethod,
  status: WorkflowBoardAcEvidenceStatus,
  checkIds: Schema.Array(TrimmedNonEmptyString),
  procedure: Schema.NullOr(Schema.String),
  observationNote: Schema.NullOr(Schema.String),
  evidenceRef: Schema.NullOr(TrimmedNonEmptyString),
  evidenceDigest: Schema.NullOr(TrimmedNonEmptyString),
  productSnapshotDigest: Schema.NullOr(TrimmedNonEmptyString),
  externalSource: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  staleReason: Schema.optional(Schema.NullOr(Schema.String)),
  observedBy: Schema.NullOr(TrimmedNonEmptyString),
  observedAt: Schema.NullOr(IsoDateTime),
  updatedAt: IsoDateTime,
});
export type WorkflowBoardAcObservationRecord = typeof WorkflowBoardAcObservationRecord.Type;

export const WorkflowBoardFixPointer = Schema.Struct({
  stage: WorkflowBoardM3Stage,
  tab: TrimmedNonEmptyString,
  targetId: Schema.NullOr(TrimmedNonEmptyString),
});
export type WorkflowBoardFixPointer = typeof WorkflowBoardFixPointer.Type;

export const WorkflowBoardShipBlocker = Schema.Struct({
  id: TrimmedNonEmptyString,
  kind: WorkflowBoardShipBlockerKind,
  message: TrimmedNonEmptyString,
  pointer: WorkflowBoardFixPointer,
  blocking: Schema.Boolean,
});
export type WorkflowBoardShipBlocker = typeof WorkflowBoardShipBlocker.Type;

export const WorkflowBoardPlanAmendmentRecord = Schema.Struct({
  id: TrimmedNonEmptyString,
  cardRevision: PositiveInt,
  targetStage: Schema.optional(WorkflowBoardAmendmentTargetStage),
  status: Schema.optional(WorkflowBoardAmendmentStatus),
  reason: TrimmedNonEmptyString,
  summary: Schema.NullOr(Schema.String),
  sourceTaskId: Schema.NullOr(TrimmedNonEmptyString),
  unexpectedFiles: Schema.Array(WorkflowBoardArtifactKey),
  createdAt: IsoDateTime,
});
export type WorkflowBoardPlanAmendmentRecord = typeof WorkflowBoardPlanAmendmentRecord.Type;

export const WorkflowBoardDiffFileEntry = Schema.Struct({
  path: WorkflowBoardArtifactKey,
  previousPath: Schema.NullOr(WorkflowBoardArtifactKey),
  changeType: WorkflowBoardDiffChangeType,
  additions: NonNegativeInt,
  deletions: NonNegativeInt,
  patch: Schema.NullOr(Schema.String),
  contractStatus: Schema.optional(Schema.NullOr(WorkflowBoardDiffFileContractStatus)),
});
export type WorkflowBoardDiffFileEntry = typeof WorkflowBoardDiffFileEntry.Type;
