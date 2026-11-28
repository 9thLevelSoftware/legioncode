import * as Schema from "effect/Schema";

import { IsoDateTime, NonNegativeInt, TrimmedNonEmptyString } from "./baseSchemas.ts";
import {
  areDisjointTaskFiles,
  areDistinctPlanAlternatives,
  areUniqueAcCriterionIds,
  areUniqueLenses,
  isAcyclicTaskDag,
  isNotCredentialFilePath,
  isNotGitMetadataPath,
  isSafeArgvCommandFilter,
  isSafeRelativeArtifactKey,
  isWithinArtifactKeyMaxScalars,
  maxLengthFilter,
} from "./workflowBoardValidators.ts";

export const WorkflowBoardArtifactKey = TrimmedNonEmptyString.check(
  isWithinArtifactKeyMaxScalars,
  isSafeRelativeArtifactKey,
);
export type WorkflowBoardArtifactKey = typeof WorkflowBoardArtifactKey.Type;

export const WorkflowBoardNonGitArtifactKey = WorkflowBoardArtifactKey.check(isNotGitMetadataPath);
export type WorkflowBoardNonGitArtifactKey = typeof WorkflowBoardNonGitArtifactKey.Type;

export const WorkflowBoardSafeInputPath = WorkflowBoardArtifactKey.check(
  isNotCredentialFilePath,
  isNotGitMetadataPath,
);
export type WorkflowBoardSafeInputPath = typeof WorkflowBoardSafeInputPath.Type;

export const WorkflowBoardIntakeGoal = Schema.Literals(["greenfield", "change", "audit"]);
export type WorkflowBoardIntakeGoal = typeof WorkflowBoardIntakeGoal.Type;

export const WorkflowBoardSourceInputDigest = Schema.Struct({
  path: WorkflowBoardArtifactKey,
  digest: TrimmedNonEmptyString,
});
export type WorkflowBoardSourceInputDigest = typeof WorkflowBoardSourceInputDigest.Type;

export const WorkflowBoardSpecIntakeAnalysis = Schema.Struct({
  suppliedDecisions: Schema.Array(TrimmedNonEmptyString),
  inferredDecisions: Schema.Array(TrimmedNonEmptyString),
  missingDecisions: Schema.Array(TrimmedNonEmptyString),
  sourceDigest: Schema.NullOr(TrimmedNonEmptyString),
});
export type WorkflowBoardSpecIntakeAnalysis = typeof WorkflowBoardSpecIntakeAnalysis.Type;

export const WorkflowBoardSpecDiff = Schema.Struct({
  previousDigest: Schema.NullOr(TrimmedNonEmptyString),
  nextDigest: TrimmedNonEmptyString,
  changedFields: Schema.Array(TrimmedNonEmptyString),
  unifiedDiff: Schema.String,
});
export type WorkflowBoardSpecDiff = typeof WorkflowBoardSpecDiff.Type;

export const WorkflowBoardExploreAction = Schema.Literals([
  "explain",
  "recommend",
  "edit",
  "unsure",
  "pause",
  "resume",
]);
export type WorkflowBoardExploreAction = typeof WorkflowBoardExploreAction.Type;

export const WorkflowBoardExploreStepAction = Schema.Literals(["step", "pause", "resume"]);
export type WorkflowBoardExploreStepAction = typeof WorkflowBoardExploreStepAction.Type;

export const WorkflowBoardExplorationDecisionStatus = Schema.Literals(["unresolved", "resolved"]);
export type WorkflowBoardExplorationDecisionStatus =
  typeof WorkflowBoardExplorationDecisionStatus.Type;

export const WorkflowBoardExplorationDecision = Schema.Struct({
  id: TrimmedNonEmptyString,
  prompt: Schema.String,
  status: WorkflowBoardExplorationDecisionStatus,
  lastAction: Schema.NullOr(WorkflowBoardExploreAction),
  answer: Schema.NullOr(TrimmedNonEmptyString),
  explanation: Schema.optional(Schema.NullOr(Schema.String)),
  recommendation: Schema.optional(Schema.NullOr(Schema.String)),
  updatedAt: IsoDateTime,
});
export type WorkflowBoardExplorationDecision = typeof WorkflowBoardExplorationDecision.Type;

export const WorkflowBoardExplorationState = Schema.Struct({
  roundIndex: NonNegativeInt,
  pendingRoundPrompt: Schema.Boolean,
  paused: Schema.Boolean,
  unresolvedIds: Schema.Array(TrimmedNonEmptyString),
  decisions: Schema.Array(WorkflowBoardExplorationDecision),
  interviewDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
});
export type WorkflowBoardExplorationState = typeof WorkflowBoardExplorationState.Type;

export const WorkflowBoardExploreAnswerInput = Schema.Struct({
  decisionId: TrimmedNonEmptyString,
  action: Schema.optional(WorkflowBoardExploreAction),
  choice: Schema.optional(WorkflowBoardExploreAction),
  answer: Schema.optional(TrimmedNonEmptyString),
  note: Schema.optional(Schema.String),
}).check(
  Schema.makeFilter(
    (input) => {
      const effectiveAction = input.action ?? input.choice;
      if (effectiveAction === undefined) {
        return false;
      }
      if (effectiveAction === "edit") {
        const val = input.answer ?? input.note;
        return val !== undefined && val.trim().length > 0;
      }
      return true;
    },
    {
      expected:
        "an exploration answer with `action` (or `choice`), plus non-empty `answer`/`note` when `edit`",
    },
  ),
);
export type WorkflowBoardExploreAnswerInput = typeof WorkflowBoardExploreAnswerInput.Type;

export const WorkflowBoardChallengeStepAction = Schema.Literals([
  "start",
  "resolve",
  "synthesize",
  "complete",
  "manualFallback",
]);
export type WorkflowBoardChallengeStepAction = typeof WorkflowBoardChallengeStepAction.Type;

export const WorkflowBoardChallengeDisposition = Schema.Literals([
  "unresolved",
  "answer",
  "dismiss",
  "accept",
  "accept-risk",
  "defer",
]);
export type WorkflowBoardChallengeDisposition = typeof WorkflowBoardChallengeDisposition.Type;

export const WorkflowBoardChallengeStatus = Schema.Literals([
  "pending",
  "in_progress",
  "resolved",
  "synthesized",
  "manual_fallback",
  "complete",
  "stale",
]);
export type WorkflowBoardChallengeStatus = typeof WorkflowBoardChallengeStatus.Type;

export const WorkflowBoardChallengeConcernInput = Schema.Struct({
  id: TrimmedNonEmptyString,
  summary: TrimmedNonEmptyString,
  whyMatters: Schema.optional(Schema.NullOr(Schema.String)),
  evidence: Schema.optional(Schema.NullOr(Schema.String)),
  assumption: Schema.optional(Schema.NullOr(Schema.String)),
  disposition: WorkflowBoardChallengeDisposition,
  answer: Schema.optional(Schema.String),
  reason: Schema.optional(Schema.String),
  mutatedAnswer: Schema.optional(Schema.String),
}).check(
  Schema.makeFilter(
    (concern) => {
      if (concern.disposition === "dismiss") {
        if (concern.reason === undefined || concern.reason.trim().length === 0) {
          return false;
        }
      }
      if (concern.disposition === "answer") {
        if (concern.answer === undefined || concern.answer.trim().length === 0) {
          return false;
        }
      }
      if (
        concern.mutatedAnswer !== undefined &&
        concern.mutatedAnswer.trim() !== (concern.answer ?? "").trim()
      ) {
        return false;
      }
      return true;
    },
    {
      expected:
        "a valid challenge concern (dismiss requires reason, answer requires answer, synthesis cannot mutate answer)",
    },
  ),
);
export type WorkflowBoardChallengeConcernInput = typeof WorkflowBoardChallengeConcernInput.Type;

export const WorkflowBoardChallengeConcern = Schema.Struct({
  id: TrimmedNonEmptyString,
  summary: TrimmedNonEmptyString,
  whyMatters: Schema.optional(Schema.NullOr(Schema.String)),
  evidence: Schema.optional(Schema.NullOr(Schema.String)),
  assumption: Schema.optional(Schema.NullOr(Schema.String)),
  disposition: WorkflowBoardChallengeDisposition,
  answer: Schema.NullOr(Schema.String),
  reason: Schema.NullOr(Schema.String),
});
export type WorkflowBoardChallengeConcern = typeof WorkflowBoardChallengeConcern.Type;

export const WorkflowBoardChallengeConcernResponse = Schema.Struct({
  concernId: TrimmedNonEmptyString,
  disposition: WorkflowBoardChallengeDisposition,
  response: Schema.optional(Schema.String),
  reason: Schema.optional(Schema.String),
});
export type WorkflowBoardChallengeConcernResponse =
  typeof WorkflowBoardChallengeConcernResponse.Type;

export const WorkflowBoardChallengeManualFallback = Schema.Struct({
  acknowledgementToken: Schema.Literal("I acknowledge"),
  answers: Schema.Tuple([TrimmedNonEmptyString, TrimmedNonEmptyString, TrimmedNonEmptyString]),
  recordedAt: IsoDateTime,
});
export type WorkflowBoardChallengeManualFallback = typeof WorkflowBoardChallengeManualFallback.Type;

export const WorkflowBoardChallengeReceipt = Schema.Struct({
  status: WorkflowBoardChallengeStatus,
  concerns: Schema.Array(WorkflowBoardChallengeConcern),
  synthesisCount: NonNegativeInt,
  specDigest: Schema.NullOr(TrimmedNonEmptyString),
  interviewDigest: Schema.NullOr(TrimmedNonEmptyString),
  sourceRefsDigest: Schema.optional(Schema.NullOr(TrimmedNonEmptyString)),
  manualFallback: Schema.NullOr(WorkflowBoardChallengeManualFallback),
  diffSummary: Schema.optional(Schema.NullOr(Schema.String)),
  updatedAt: IsoDateTime,
});
export type WorkflowBoardChallengeReceipt = typeof WorkflowBoardChallengeReceipt.Type;

export const WorkflowBoardPlanLens = Schema.Literals([
  "ux",
  "functional",
  "architecture",
  "quality",
  "infrastructure",
  "delivery",
]);
export type WorkflowBoardPlanLens = typeof WorkflowBoardPlanLens.Type;

export const WorkflowBoardPlanLensEntry = Schema.Struct({
  lens: WorkflowBoardPlanLens,
  applicable: Schema.NullOr(Schema.Boolean),
  summary: Schema.optional(Schema.NullOr(Schema.String)),
  reason: Schema.optional(Schema.NullOr(Schema.String)),
}).check(
  Schema.makeFilter(
    (entry) => {
      if (entry.applicable === true) {
        return (
          entry.summary !== undefined && entry.summary !== null && entry.summary.trim().length > 0
        );
      }
      if (entry.applicable === false) {
        return (
          entry.reason !== undefined && entry.reason !== null && entry.reason.trim().length > 0
        );
      }
      return true;
    },
    {
      expected:
        "a design lens with non-empty `summary` when `applicable` is true, or non-empty `reason` when false",
    },
  ),
);
export type WorkflowBoardPlanLensEntry = typeof WorkflowBoardPlanLensEntry.Type;

export const WorkflowBoardPlanAlternative = Schema.Struct({
  id: TrimmedNonEmptyString,
  title: TrimmedNonEmptyString,
  summary: TrimmedNonEmptyString,
  tradeoffs: TrimmedNonEmptyString,
});
export type WorkflowBoardPlanAlternative = typeof WorkflowBoardPlanAlternative.Type;

export const WorkflowBoardPlanningStrategy = Schema.Literals([
  "outcomes",
  "risk-first",
  "expand-contract",
  "custom",
]);
export type WorkflowBoardPlanningStrategy = typeof WorkflowBoardPlanningStrategy.Type;

export const WorkflowBoardStrategyMetadata = Schema.Struct({
  uncertainty: Schema.optional(TrimmedNonEmptyString),
  probeTaskId: Schema.optional(TrimmedNonEmptyString),
  dependentTaskIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  compatibilityWindow: Schema.optional(TrimmedNonEmptyString),
  transitionStep: Schema.optional(TrimmedNonEmptyString),
  retirementPrereq: Schema.optional(TrimmedNonEmptyString),
  compatibilityTaskIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  transitionTaskIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  retirementTaskIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  prerequisiteEvidencePaths: Schema.optional(Schema.Array(WorkflowBoardArtifactKey)),
});
export type WorkflowBoardStrategyMetadata = typeof WorkflowBoardStrategyMetadata.Type;

export const WorkflowBoardTaskGranularity = Schema.Literals(["coarse", "balanced", "fine"]);
export type WorkflowBoardTaskGranularity = typeof WorkflowBoardTaskGranularity.Type;

export const WorkflowBoardTaskTestingMethod = Schema.Literals([
  "test-first",
  "regression-first",
  "existing-checks",
]);
export type WorkflowBoardTaskTestingMethod = typeof WorkflowBoardTaskTestingMethod.Type;

export const WorkflowBoardTaskStatus = Schema.Literals([
  "pending",
  "todo",
  "in_progress",
  "completed",
  "failed",
  "blocked",
  "skipped",
]);
export type WorkflowBoardTaskStatus = typeof WorkflowBoardTaskStatus.Type;

export const WorkflowBoardArgvCommand =
  Schema.NonEmptyArray(TrimmedNonEmptyString).check(isSafeArgvCommandFilter);
export type WorkflowBoardArgvCommand = typeof WorkflowBoardArgvCommand.Type;

export const WorkflowBoardTaskContract = Schema.Struct({
  id: Schema.optional(TrimmedNonEmptyString),
  taskId: TrimmedNonEmptyString,
  title: TrimmedNonEmptyString,
  dependsOn: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  blockedBy: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  blocks: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  filesAllowed: Schema.NonEmptyArray(WorkflowBoardNonGitArtifactKey),
  filesForbidden: Schema.optional(Schema.Array(WorkflowBoardNonGitArtifactKey)),
  expectedArtifacts: Schema.optional(Schema.Array(WorkflowBoardArtifactKey)),
  verificationCommands: Schema.optional(Schema.Array(WorkflowBoardArgvCommand)),
  testingMethod: WorkflowBoardTaskTestingMethod,
  testingLimitations: Schema.optional(Schema.NullOr(Schema.String)),
  status: Schema.optional(WorkflowBoardTaskStatus),
}).check(
  Schema.makeFilter(
    (task) => {
      const taskId = task.taskId.trim();
      const forbidden = new Set((task.filesForbidden ?? []).map((p) => p.trim()));
      if (!task.filesAllowed.every((p) => !forbidden.has(p.trim()))) {
        return false;
      }
      const selfEdges = [
        ...(task.dependsOn ?? []),
        ...(task.blockedBy ?? []),
        ...(task.blocks ?? []),
      ];
      return selfEdges.every((id) => id.trim() !== taskId);
    },
    {
      expected:
        "a task contract whose `filesAllowed` and `filesForbidden` do not intersect and that does not depend on or block itself",
    },
  ),
);
export type WorkflowBoardTaskContract = typeof WorkflowBoardTaskContract.Type;

export const WorkflowBoardPlannedCheck = Schema.Struct({
  id: TrimmedNonEmptyString,
  criterionId: Schema.optional(TrimmedNonEmptyString),
  argv: WorkflowBoardArgvCommand,
  description: Schema.optional(Schema.NullOr(Schema.String)),
});
export type WorkflowBoardPlannedCheck = typeof WorkflowBoardPlannedCheck.Type;

export const WorkflowBoardEvidenceMethod = Schema.Literals(["automated", "manual", "external"]);
export type WorkflowBoardEvidenceMethod = typeof WorkflowBoardEvidenceMethod.Type;

export const WorkflowBoardAcEvidenceMapping = Schema.Struct({
  criterionId: TrimmedNonEmptyString,
  method: WorkflowBoardEvidenceMethod,
  checkIds: Schema.optional(Schema.Array(TrimmedNonEmptyString)),
  command: Schema.optional(WorkflowBoardArgvCommand),
  procedure: Schema.optional(TrimmedNonEmptyString),
  externalRef: Schema.optional(TrimmedNonEmptyString),
  externalSource: Schema.optional(TrimmedNonEmptyString),
  limitations: Schema.optional(Schema.NullOr(Schema.String)),
}).check(
  Schema.makeFilter(
    (mapping) => {
      if (mapping.method === "automated") {
        return mapping.command !== undefined || (mapping.checkIds?.length ?? 0) > 0;
      }
      if (mapping.method === "manual") {
        return mapping.procedure !== undefined && mapping.procedure.trim().length > 0;
      }
      return (
        (mapping.externalRef !== undefined && mapping.externalRef.trim().length > 0) ||
        (mapping.externalSource !== undefined && mapping.externalSource.trim().length > 0) ||
        (mapping.procedure !== undefined && mapping.procedure.trim().length > 0)
      );
    },
    {
      expected:
        "an AC evidence mapping with command/checkIds for automated, procedure for manual, or externalRef/externalSource/procedure for external",
    },
  ),
);
export type WorkflowBoardAcEvidenceMapping = typeof WorkflowBoardAcEvidenceMapping.Type;

export const WorkflowBoardPlanDocument = Schema.Struct({
  lenses: Schema.Array(WorkflowBoardPlanLensEntry),
  alternatives: Schema.Array(WorkflowBoardPlanAlternative),
  alternativesRevisionCount: NonNegativeInt,
  selectedAlternativeId: Schema.NullOr(TrimmedNonEmptyString),
  selectionRationale: Schema.NullOr(Schema.String),
  strategy: Schema.NullOr(WorkflowBoardPlanningStrategy),
  strategyRationale: Schema.NullOr(Schema.String),
  strategyMetadata: Schema.NullOr(WorkflowBoardStrategyMetadata),
  granularity: Schema.NullOr(WorkflowBoardTaskGranularity),
  tasks: Schema.Array(WorkflowBoardTaskContract),
  checks: Schema.Array(WorkflowBoardPlannedCheck),
  acMappings: Schema.Array(WorkflowBoardAcEvidenceMapping),
  planDigest: Schema.NullOr(TrimmedNonEmptyString),
  tasksDigest: Schema.NullOr(TrimmedNonEmptyString),
  updatedAt: IsoDateTime,
});
export type WorkflowBoardPlanDocument = typeof WorkflowBoardPlanDocument.Type;

export const WorkflowBoardReturnTargetStage = Schema.Literals(["spec", "plan", "execute"]);
export type WorkflowBoardReturnTargetStage = typeof WorkflowBoardReturnTargetStage.Type;

export const WorkflowBoardExploreAnswersArray = Schema.Array(WorkflowBoardExploreAnswerInput).check(
  maxLengthFilter(3, "exploration answers per step"),
);

export const WorkflowBoardChallengeConcernsArray = Schema.Array(
  WorkflowBoardChallengeConcernInput,
).check(maxLengthFilter(3, "challenge concerns"));

export const WorkflowBoardPlanLensesArray = Schema.Array(WorkflowBoardPlanLensEntry).check(
  Schema.makeFilter(areUniqueLenses, { expected: "unique design lens entries" }),
);

export const WorkflowBoardPlanAlternativesPair = Schema.Array(WorkflowBoardPlanAlternative).check(
  Schema.makeFilter(areDistinctPlanAlternatives, {
    expected: "exactly two materially distinct design alternatives with unique IDs",
  }),
);

export const WorkflowBoardTaskContractsArray = Schema.Array(WorkflowBoardTaskContract).check(
  Schema.makeFilter((tasks) => areDisjointTaskFiles(tasks) && isAcyclicTaskDag(tasks), {
    expected:
      "an acyclic task DAG with unique task IDs, valid dependency references, and mutually exclusive filesAllowed",
  }),
);

export const WorkflowBoardAcEvidenceMappingsArray = Schema.Array(
  WorkflowBoardAcEvidenceMapping,
).check(
  Schema.makeFilter(areUniqueAcCriterionIds, {
    expected: "unique acceptance criterion IDs across AC evidence mappings",
  }),
);
