import * as Schema from "effect/Schema";

import { IsoDateTime, PositiveInt, ProjectId, TrimmedNonEmptyString } from "./baseSchemas.ts";
import { ModelSelection } from "./modelSelection.ts";
import { OrchestrationV2ThreadLaunchWorkspaceStrategy } from "./orchestrationV2.ts";
import { ProviderInteractionMode, RuntimeMode } from "./providerPolicy.ts";
import {
  WorkflowBoardArgvCommand,
  WorkflowBoardArtifactKey,
  WorkflowBoardSafeInputPath,
  WorkflowBoardTaskStatus,
} from "./workflowBoardPlanning.ts";

export const WorkflowBoardCliImportLockKind = Schema.Literals([
  "engine_lock",
  "live_spawn",
  "resume_json",
]);
export type WorkflowBoardCliImportLockKind = typeof WorkflowBoardCliImportLockKind.Type;

export const WorkflowBoardCliImportLockStatus = Schema.Literals(["active", "stale"]);
export type WorkflowBoardCliImportLockStatus = typeof WorkflowBoardCliImportLockStatus.Type;

export const WorkflowBoardCliImportLockEntry = Schema.Struct({
  kind: WorkflowBoardCliImportLockKind,
  relativePath: WorkflowBoardArtifactKey,
  status: WorkflowBoardCliImportLockStatus,
  pid: Schema.NullOr(PositiveInt),
  runId: Schema.NullOr(TrimmedNonEmptyString),
  startedAt: Schema.NullOr(TrimmedNonEmptyString),
  message: Schema.String,
});
export type WorkflowBoardCliImportLockEntry = typeof WorkflowBoardCliImportLockEntry.Type;

export const WorkflowBoardCliImportPolicyStatus = Schema.Literals(["unsupported", "acknowledged"]);
export type WorkflowBoardCliImportPolicyStatus = typeof WorkflowBoardCliImportPolicyStatus.Type;

export const WorkflowBoardCliImportUnsupportedPolicy = Schema.Struct({
  key: TrimmedNonEmptyString,
  category: TrimmedNonEmptyString,
  summary: TrimmedNonEmptyString,
  configuredValue: Schema.NullOr(Schema.String),
  status: WorkflowBoardCliImportPolicyStatus,
  blocksExecution: Schema.Boolean,
  sourcePath: WorkflowBoardArtifactKey,
});
export type WorkflowBoardCliImportUnsupportedPolicy =
  typeof WorkflowBoardCliImportUnsupportedPolicy.Type;

export const WorkflowBoardCliImportHistoricalReceiptKind = Schema.Literals([
  "spec_approval",
  "plan_approval",
  "execution",
  "review",
  "check",
]);
export type WorkflowBoardCliImportHistoricalReceiptKind =
  typeof WorkflowBoardCliImportHistoricalReceiptKind.Type;

export const WorkflowBoardCliImportHistoricalStatus = Schema.Literals(["historical"]);
export type WorkflowBoardCliImportHistoricalStatus =
  typeof WorkflowBoardCliImportHistoricalStatus.Type;

export const WorkflowBoardCliImportHistoricalReceipt = Schema.Struct({
  id: TrimmedNonEmptyString,
  kind: WorkflowBoardCliImportHistoricalReceiptKind,
  status: WorkflowBoardCliImportHistoricalStatus,
  sourcePath: WorkflowBoardArtifactKey,
  recordedAt: Schema.NullOr(TrimmedNonEmptyString),
  recordedBy: Schema.NullOr(TrimmedNonEmptyString),
  summary: Schema.String,
  requiresNativeRevalidation: Schema.Boolean,
});
export type WorkflowBoardCliImportHistoricalReceipt =
  typeof WorkflowBoardCliImportHistoricalReceipt.Type;

export const WorkflowBoardCliImportMissingLogNote = Schema.Struct({
  taskId: Schema.NullOr(TrimmedNonEmptyString),
  checkId: Schema.NullOr(TrimmedNonEmptyString),
  relativePath: WorkflowBoardArtifactKey,
  command: Schema.NullOr(TrimmedNonEmptyString),
  note: Schema.String,
});
export type WorkflowBoardCliImportMissingLogNote = typeof WorkflowBoardCliImportMissingLogNote.Type;

export const WorkflowBoardCliImportTaskPreview = Schema.Struct({
  id: TrimmedNonEmptyString,
  taskId: TrimmedNonEmptyString,
  title: TrimmedNonEmptyString,
  status: WorkflowBoardTaskStatus,
  blockerSummary: Schema.NullOr(Schema.String),
  verificationCommands: Schema.Array(WorkflowBoardArgvCommand),
  missingLogNotes: Schema.Array(WorkflowBoardCliImportMissingLogNote),
});
export type WorkflowBoardCliImportTaskPreview = typeof WorkflowBoardCliImportTaskPreview.Type;

export const WorkflowBoardCliImportSourceSystem = Schema.Literals(["legion-cli"]);
export type WorkflowBoardCliImportSourceSystem = typeof WorkflowBoardCliImportSourceSystem.Type;

export const WorkflowBoardCliImportOwner = Schema.Literals(["legioncode"]);
export type WorkflowBoardCliImportOwner = typeof WorkflowBoardCliImportOwner.Type;

export const WorkflowBoardCliImportProvenance = Schema.Struct({
  sourceSystem: WorkflowBoardCliImportSourceSystem,
  sourceCliDir: WorkflowBoardArtifactKey,
  sourceSpecId: TrimmedNonEmptyString,
  owner: WorkflowBoardCliImportOwner,
  importedAt: IsoDateTime,
  lastSyncedAt: IsoDateTime,
  sourceDigest: Schema.NullOr(TrimmedNonEmptyString),
  decisionIds: Schema.Array(TrimmedNonEmptyString),
  assumptionIds: Schema.Array(TrimmedNonEmptyString),
  acceptanceCriterionIds: Schema.Array(TrimmedNonEmptyString),
  historicalReceipts: Schema.Array(WorkflowBoardCliImportHistoricalReceipt),
  unsupportedPolicies: Schema.Array(WorkflowBoardCliImportUnsupportedPolicy),
  acknowledgedPolicies: Schema.Array(TrimmedNonEmptyString),
  missingLogNotes: Schema.Array(WorkflowBoardCliImportMissingLogNote),
  preservedArtifactPaths: Schema.Array(WorkflowBoardArtifactKey),
  assurancePolicySummary: Schema.NullOr(Schema.String),
});
export type WorkflowBoardCliImportProvenance = typeof WorkflowBoardCliImportProvenance.Type;

export const WorkflowBoardPreviewCliImportInput = Schema.Struct({
  projectId: ProjectId,
  cliDir: Schema.optional(WorkflowBoardSafeInputPath),
});
export type WorkflowBoardPreviewCliImportInput = typeof WorkflowBoardPreviewCliImportInput.Type;

export const WorkflowBoardApplyCliImportInput = Schema.Struct({
  projectId: ProjectId,
  cliDir: Schema.optional(WorkflowBoardSafeInputPath),
  acknowledgeUnsupportedPolicies: Schema.optional(Schema.Boolean),
  acknowledgedPolicies: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  modelSelection: Schema.optional(ModelSelection),
  runtimeMode: Schema.optional(RuntimeMode),
  interactionMode: Schema.optional(ProviderInteractionMode),
  workspaceStrategy: Schema.optional(OrchestrationV2ThreadLaunchWorkspaceStrategy),
});
export type WorkflowBoardApplyCliImportInput = typeof WorkflowBoardApplyCliImportInput.Type;
