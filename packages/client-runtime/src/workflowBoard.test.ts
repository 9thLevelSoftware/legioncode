import {
  EnvironmentAuthorizationError,
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProjectId,
  ProviderInstanceId,
  RunId,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WorkflowBoardNonGitArtifactKey,
  WorkflowBoardRequestError,
  WorkflowBoardStageRunId,
  WorkflowCardId,
  type WorkflowBoardCardSummary,
  type WorkflowBoardTaskContract,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import {
  WORKFLOW_BOARD_PLAN_LENSES,
  WORKFLOW_BOARD_STAGES,
  canApproveWorkflowBoardDelivery,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  collectDownstreamInvalidationStates,
  collectPlanApprovalBlockers,
  groupWorkflowBoardCards,
  isAcObservationStale,
  isCheckReceiptStale,
  isProjectWorkflowBoardEnabled,
  isProjectWorkflowBoardSupported,
  isReviewReportStale,
  isWorkflowBoardCardShipped,
  projectWorkflowBoardAvailability,
  resolveFixPointerNavigation,
  validReturnTargetStages,
  workflowBoardAcEvidenceStatusLabel,
  workflowBoardActiveTask,
  workflowBoardAvailabilityNotice,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardCheckStatusLabel,
  workflowBoardCliImportHistoricalReceiptKindLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardHasHistoricalEvidence,
  workflowBoardIsTaskDagReady,
  workflowBoardLatestStageRun,
  workflowBoardNeedsRecovery,
  workflowBoardNextReadyTask,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardProjectNotFoundNotice,
  workflowBoardReadPermissionNotice,
  workflowBoardRecoverySummary,
  workflowBoardReviewVerdictLabel,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
  workflowBoardStaleEvidenceSummary,
  workflowBoardTaskStatusMap,
  workflowBoardUnacknowledgedBlockingPolicies,
  workflowBoardUnresolvedFindings,
  isValidCliImportDirInput,
} from "./workflowBoard.ts";

const PROJECT_ID = ProjectId.make("project-1");

const decodeWorkflowBoardErrorUnion = Schema.decodeUnknownExit(
  Schema.Union([WorkflowBoardRequestError, EnvironmentAuthorizationError]),
);
const decodeUnrelatedStruct = Schema.decodeUnknownExit(Schema.Struct({ id: Schema.String }));

function makeSummary(
  overrides: Partial<WorkflowBoardCardSummary> & Pick<WorkflowBoardCardSummary, "cardId">,
): WorkflowBoardCardSummary {
  return {
    projectId: PROJECT_ID,
    title: "Card title",
    stage: "spec",
    status: "active",
    revision: PositiveInt.make(1),
    currentTask: null,
    blockerSummary: null,
    nextAction: null,
    specApprovedRevision: null,
    specApprovalStatus: null,
    activeThreadId: null,
    createdAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
    updatedAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
    ...overrides,
  };
}

describe("workflowBoard helpers", () => {
  it("gates board availability by connection, capability, and Labs flag", () => {
    expect(
      isProjectWorkflowBoardEnabled({
        settings: { workflowsEnabled: false },
        capabilities: { projectWorkflowBoard: true },
      }),
    ).toBe(false);
    expect(
      isProjectWorkflowBoardEnabled({
        settings: { workflowsEnabled: true },
        capabilities: { projectWorkflowBoard: undefined },
      }),
    ).toBe(false);
    expect(
      isProjectWorkflowBoardEnabled({
        settings: { workflowsEnabled: true },
        capabilities: { projectWorkflowBoard: true },
      }),
    ).toBe(true);

    expect(
      projectWorkflowBoardAvailability({
        connected: false,
        settings: { workflowsEnabled: true },
        capabilities: { projectWorkflowBoard: true },
      }),
    ).toBe("disconnected");
    expect(
      projectWorkflowBoardAvailability({
        connected: true,
        settings: { workflowsEnabled: true },
        capabilities: { projectWorkflowBoard: false },
      }),
    ).toBe("unsupported");
    expect(
      projectWorkflowBoardAvailability({
        connected: true,
        settings: { workflowsEnabled: false },
        capabilities: { projectWorkflowBoard: true },
      }),
    ).toBe("labs-disabled");
    expect(isProjectWorkflowBoardSupported(undefined)).toBe(false);
    expect(isProjectWorkflowBoardSupported({ projectWorkflowBoard: false })).toBe(false);
    expect(isProjectWorkflowBoardSupported({ projectWorkflowBoard: true })).toBe(true);

    expect(
      projectWorkflowBoardAvailability({
        connected: true,
        configLoaded: false,
        settings: undefined,
        capabilities: undefined,
      }),
    ).toBe("loading");
    expect(
      projectWorkflowBoardAvailability({
        connected: true,
        configLoaded: true,
        settings: { workflowsEnabled: true },
        capabilities: {},
      }),
    ).toBe("unsupported");
    expect(
      projectWorkflowBoardAvailability({
        connected: true,
        settings: { workflowsEnabled: true },
        capabilities: { projectWorkflowBoard: true },
      }),
    ).toBe("enabled");

    expect(workflowBoardAvailabilityNotice("enabled")).toBeNull();
    expect(workflowBoardAvailabilityNotice("loading")?.title).toBe("Loading workflow board…");
    expect(workflowBoardAvailabilityNotice("disconnected")?.title).toBe(
      "Environment is not connected",
    );
    expect(workflowBoardAvailabilityNotice("unsupported")?.title).toBe(
      "Workflow board is not supported",
    );
    expect(workflowBoardAvailabilityNotice("labs-disabled")).toMatchObject({
      title: "Workflows are off",
      action: "open-labs-settings",
    });
    expect(workflowBoardReadPermissionNotice().title).toBe("Read permission required");
    expect(workflowBoardProjectNotFoundNotice().title).toBe("Project not found");

    expect(
      workflowBoardErrorMessage({
        _tag: "UnsupportedRpcMethod",
        message: "Unsupported RPC method `workflowBoard.getCard`",
      }),
    ).toBe("This server does not support this workflow board action.");
    expect(
      workflowBoardErrorMessage(new Error("Workflow board is handled by the Rust server.")),
    ).toBe("This server does not support this workflow board action.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "internal_error",
          message: "Workflow board is handled by the Rust server.",
        }),
      ),
    ).toBe("This server does not support this workflow board action.");
    expect(workflowBoardErrorMessage("Unknown request tag: workflowBoard.getCard")).toBe(
      "This server does not support this workflow board action.",
    );
    expect(workflowBoardErrorMessage("unknown request tag: workflowBoard.createCard")).toBe(
      "This server does not support this workflow board action.",
    );
    expect(workflowBoardErrorMessage("Unsupported RPC method `workflowBoard.getCard`")).toBe(
      "This server does not support this workflow board action.",
    );

    const unsupportedRpcSchemaExit = decodeWorkflowBoardErrorUnion({
      _tag: "UnsupportedRpcMethod",
      message: "Unsupported RPC method `workflowBoard.getCard`",
    });
    expect(unsupportedRpcSchemaExit._tag).toBe("Failure");
    if (unsupportedRpcSchemaExit._tag === "Failure") {
      expect(workflowBoardErrorMessage(Cause.squash(unsupportedRpcSchemaExit.cause))).toBe(
        "This server does not support this workflow board action.",
      );
    }

    expect(
      workflowBoardErrorMessage({
        _tag: "SchemaError",
        name: "SchemaError",
        message:
          'Expected { readonly "_tag": "WorkflowBoardRequestError" } | { readonly "_tag": "EnvironmentAuthorizationError" }',
      }),
    ).toBe("This server does not support this workflow board action.");

    // Negative cases: normal internal_error, unrelated SchemaError, and unrelated bare string
    // must NOT be misclassified as unsupported workflow board RPC errors.
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "internal_error",
          message: "disk full",
        }),
      ),
    ).toBe("disk full");

    const unrelatedSchemaExit = decodeUnrelatedStruct({ id: 123 });
    expect(unrelatedSchemaExit._tag).toBe("Failure");
    if (unrelatedSchemaExit._tag === "Failure") {
      const rendered = workflowBoardErrorMessage(Cause.squash(unrelatedSchemaExit.cause));
      expect(rendered).not.toBe("This server does not support this workflow board action.");
      expect(rendered).toContain("Expected string");
    }
    expect(
      workflowBoardErrorMessage({
        _tag: "SchemaError",
        name: "SchemaError",
        message: 'Expected string at ["title"]',
      }),
    ).toBe('Expected string at ["title"]');
    expect(workflowBoardErrorMessage("unrelated bare defect")).toBeNull();
  });

  it("groups cards into the four stage columns and moves completed cards to completed", () => {
    expect(WORKFLOW_BOARD_STAGES).toEqual(["spec", "plan", "execute", "ship"]);
    const columns = groupWorkflowBoardCards([
      makeSummary({
        cardId: WorkflowCardId.make("c-spec-old"),
        stage: "spec",
        updatedAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-spec-new"),
        stage: "spec",
        status: "blocked",
        blockerSummary: "Needs decision",
        updatedAt: IsoDateTime.make("2026-11-24T02:00:00.000Z"),
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-plan"),
        stage: "plan",
        status: "paused",
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-exec"),
        stage: "execute",
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-ship-active"),
        stage: "ship",
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-shipped"),
        stage: "ship",
        status: "completed",
      }),
    ]);

    expect(columns.spec.map((c) => c.cardId)).toEqual(["c-spec-new", "c-spec-old"]);
    expect(columns.plan.map((c) => c.cardId)).toEqual(["c-plan"]);
    expect(columns.execute.map((c) => c.cardId)).toEqual(["c-exec"]);
    expect(columns.ship.map((c) => c.cardId)).toEqual(["c-ship-active"]);
    expect(columns.completed.map((c) => c.cardId)).toEqual(["c-shipped"]);

    const mixedPrecision = groupWorkflowBoardCards([
      makeSummary({
        cardId: WorkflowCardId.make("c-b"),
        stage: "spec",
        updatedAt: IsoDateTime.make("2026-11-24T01:00:00Z"),
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-a"),
        stage: "spec",
        updatedAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
      }),
      makeSummary({
        cardId: WorkflowCardId.make("c-half"),
        stage: "spec",
        updatedAt: IsoDateTime.make("2026-11-24T01:00:00.500Z"),
      }),
    ]);
    expect(mixedPrecision.spec.map((c) => c.cardId)).toEqual(["c-half", "c-a", "c-b"]);
  });

  it("derives labels, spec approval states, blockers, and next actions", () => {
    expect(workflowBoardStageLabel("spec")).toBe("Spec");
    expect(workflowBoardStageLabel("plan")).toBe("Plan");
    expect(workflowBoardStageLabel("execute")).toBe("Execute");
    expect(workflowBoardStageLabel("ship")).toBe("Ship");

    expect(workflowBoardCardStatusLabel("active")).toBe("Active");
    expect(workflowBoardCardStatusLabel("blocked")).toBe("Blocked");
    expect(workflowBoardCardStatusLabel("paused")).toBe("Paused");
    expect(workflowBoardCardStatusLabel("completed")).toBe("Completed");

    expect(workflowBoardSpecApprovalState({ specApprovalStatus: "valid" })).toBe("approved");
    expect(workflowBoardSpecApprovalState({ specApprovalStatus: "stale" })).toBe("stale");
    expect(workflowBoardSpecApprovalState({ specApprovalStatus: null })).toBe("unapproved");

    expect(workflowBoardSpecApprovalLabel("valid")).toBe("Spec approved");
    expect(workflowBoardSpecApprovalLabel("stale")).toBe("Approval stale");
    expect(workflowBoardSpecApprovalLabel("superseded")).toBe("Superseded");
    expect(workflowBoardSpecApprovalLabel("historical")).toBe("Historical");
    expect(workflowBoardSpecApprovalLabel(null)).toBe("Spec draft");

    expect(workflowBoardPlanApprovalState({ planApprovalStatus: "valid" })).toBe("approved");
    expect(workflowBoardPlanApprovalState({ planApprovalStatus: "stale" })).toBe("stale");
    expect(workflowBoardPlanApprovalState({ planApprovalStatus: null })).toBe("unapproved");
    expect(workflowBoardPlanApprovalState({})).toBe("unapproved");

    expect(workflowBoardPlanApprovalLabel("valid")).toBe("Plan approved");
    expect(workflowBoardPlanApprovalLabel("stale")).toBe("Plan approval stale");
    expect(workflowBoardPlanApprovalLabel("superseded")).toBe("Superseded");
    expect(workflowBoardPlanApprovalLabel("historical")).toBe("Historical");
    expect(workflowBoardPlanApprovalLabel(null)).toBe("Plan draft");
    expect(workflowBoardPlanApprovalLabel(undefined)).toBe("Plan draft");

    expect(
      workflowBoardCardBlocker(
        makeSummary({
          cardId: WorkflowCardId.make("b1"),
          status: "blocked",
          blockerSummary: "  Check failed  ",
        }),
      ),
    ).toBe("Check failed");
    expect(
      workflowBoardCardBlocker(
        makeSummary({
          cardId: WorkflowCardId.make("b2"),
          status: "blocked",
          blockerSummary: null,
        }),
      ),
    ).toBe("Blocked");
    expect(workflowBoardCardBlocker(makeSummary({ cardId: WorkflowCardId.make("b3") }))).toBeNull();

    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("n1"),
          stage: "plan",
          nextAction: "start_plan",
          specApprovalStatus: "valid",
        }),
      ),
    ).toBe("Start Plan");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("n2"),
          stage: "plan",
          activeThreadId: ThreadId.make("t-1"),
          specApprovalStatus: "valid",
        }),
      ),
    ).toBe("Open Plan thread");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("n3"),
          stage: "plan",
          status: "blocked",
          blockerSummary: "Missing approval",
        }),
      ),
    ).toBe("Missing approval");

    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-1"), nextAction: "explore_spec" }),
      ),
    ).toBe("Continue guided exploration");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-2"), nextAction: "resume_exploration" }),
      ),
    ).toBe("Resume exploration");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-3"), nextAction: "author_spec" }),
      ),
    ).toBe("Open Spec thread");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-4"), nextAction: "author_plan" }),
      ),
    ).toBe("Open Plan thread");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-5"), nextAction: "revise_spec" }),
      ),
    ).toBe("Revise spec");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-6"), nextAction: "revise_plan" }),
      ),
    ).toBe("Revise plan");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-7"), nextAction: "revise_execute" }),
      ),
    ).toBe("Revise execution");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m2-8"), nextAction: "start_execute" }),
      ),
    ).toBe("Ready for execution");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-1"), nextAction: "complete_task" }),
      ),
    ).toBe("Complete active task");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("m3-2"),
          nextAction: "run_checks_or_next_task",
        }),
      ),
    ).toBe("Run planned checks or next task");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-3"), nextAction: "continue_execute" }),
      ),
    ).toBe("Continue execution");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-4"), nextAction: "continue_stage" }),
      ),
    ).toBe("Continue execution");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-5"), nextAction: "run_review" }),
      ),
    ).toBe("Start independent review");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-6"), nextAction: "await_review" }),
      ),
    ).toBe("Open Review thread");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-7"), nextAction: "ready_for_ship" }),
      ),
    ).toBe("Review for delivery");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("m3-8"),
          nextAction: "resolve_contract_violation",
        }),
      ),
    ).toBe("Resolve file-contract violation");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("m3-9"),
          nextAction: "reconcile_external_action",
        }),
      ),
    ).toBe("Reconcile external changes");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-10"), nextAction: "retry_check" }),
      ),
    ).toBe("Retry failed check");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-11"), nextAction: "retry_review" }),
      ),
    ).toBe("Retry independent review");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-12"), nextAction: "retry_task" }),
      ),
    ).toBe("Retry failed task");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-13"), nextAction: "retry_stage" }),
      ),
    ).toBe("Retry failed step");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-14"), nextAction: "retry_step" }),
      ),
    ).toBe("Retry failed step");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-15"), nextAction: "resolve_finding" }),
      ),
    ).toBe("Resolve review findings");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-16"), nextAction: "fix_finding" }),
      ),
    ).toBe("Fix review finding");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-17"), nextAction: "resume_card" }),
      ),
    ).toBe("Resume card");
    expect(
      workflowBoardCardNextAction(
        makeSummary({ cardId: WorkflowCardId.make("m3-18"), nextAction: "git_actions" }),
      ),
    ).toBe("Select Git delivery actions");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("m4-1"),
          nextAction: "acknowledge_policies",
        }),
      ),
    ).toBe("Acknowledge imported policies");
    expect(
      workflowBoardCardNextAction(
        makeSummary({
          cardId: WorkflowCardId.make("m4-2"),
          nextAction: "resolve_blocked_task",
        }),
      ),
    ).toBe("Resolve blocked task");
  });

  it("picks the latest stage run and formats WorkflowBoardRequestError messages", () => {
    const detail: Pick<WorkflowCardDetail, "stageRuns"> = {
      stageRuns: [
        {
          id: WorkflowBoardStageRunId.make("sr-1"),
          stage: "plan",
          stableLaunchKey: "k-1",
          status: "failed",
          cardRevision: PositiveInt.make(3),
          runId: RunId.make("run-1"),
          threadId: ThreadId.make("thread-1"),
          projectId: PROJECT_ID,
          workspaceStrategy: { type: "root" },
          worktreePath: null,
          branch: null,
          modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
          runtimeMode: "full-access",
          interactionMode: "default",
          lastError: null,
          createdAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T01:01:00.000Z"),
          endedAt: null,
        },
        {
          id: WorkflowBoardStageRunId.make("sr-2"),
          stage: "plan",
          stableLaunchKey: "k-2",
          status: "running",
          cardRevision: PositiveInt.make(4),
          runId: RunId.make("run-2"),
          threadId: ThreadId.make("thread-2"),
          projectId: PROJECT_ID,
          workspaceStrategy: { type: "root" },
          worktreePath: null,
          branch: null,
          modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
          runtimeMode: "full-access",
          interactionMode: "default",
          lastError: null,
          createdAt: IsoDateTime.make("2026-11-24T02:00:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T02:01:00.000Z"),
          endedAt: null,
        },
      ],
    };

    expect(workflowBoardLatestStageRun(detail, "plan")?.id).toBe("sr-2");
    expect(workflowBoardLatestStageRun(detail, "execute")).toBeNull();

    const mixedEpochDetail: Pick<WorkflowCardDetail, "stageRuns"> = {
      stageRuns: [
        {
          ...detail.stageRuns[0]!,
          id: WorkflowBoardStageRunId.make("sr-tz"),
          cardRevision: PositiveInt.make(9),
          updatedAt: IsoDateTime.make("2026-11-24T02:00:00.000+02:00"),
        },
        {
          ...detail.stageRuns[0]!,
          id: WorkflowBoardStageRunId.make("sr-sec"),
          cardRevision: PositiveInt.make(2),
          updatedAt: IsoDateTime.make("2026-11-24T01:00:00Z"),
        },
        {
          ...detail.stageRuns[0]!,
          id: WorkflowBoardStageRunId.make("sr-half"),
          cardRevision: PositiveInt.make(3),
          updatedAt: IsoDateTime.make("2026-11-24T01:00:00.500Z"),
        },
        {
          ...detail.stageRuns[0]!,
          id: WorkflowBoardStageRunId.make("sr-half-higher-rev"),
          cardRevision: PositiveInt.make(4),
          updatedAt: IsoDateTime.make("2026-11-24T01:00:00.500+00:00"),
        },
      ],
    };
    expect(workflowBoardLatestStageRun(mixedEpochDetail, "plan")?.id).toBe("sr-half-higher-rev");

    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "revision_conflict",
          message: "stale",
          currentRevision: PositiveInt.make(4),
        }),
      ),
    ).toBe("This card changed on the server (revision 4). Refresh and try again.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "spec_not_approved",
          message: "nope",
        }),
      ),
    ).toBe("Approve the spec before starting Plan.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "plan_not_approved",
          message: "Missing AC-1 evidence mapping",
        }),
      ),
    ).toBe("Missing AC-1 evidence mapping");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "plan_not_approved",
          message: "   ",
        }),
      ),
    ).toBe("Complete and approve the plan before proceeding.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "exploration_unresolved",
          message: "",
        }),
      ),
    ).toBe("Resolve all exploration decisions before approving the spec.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "challenge_unresolved",
          message: "",
        }),
      ),
    ).toBe("Complete or resolve challenge concerns before approving the spec.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "external_edit_detected",
          message: "",
        }),
      ),
    ).toBe("External file edits were detected. Review and refresh before continuing.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "contract_conflict",
          message: "",
        }),
      ),
    ).toBe("The task file contracts or dependencies have a conflict.");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "digest_mismatch",
          message: "plan.md digest changed",
        }),
      ),
    ).toBe("plan.md digest changed");
    expect(
      workflowBoardErrorMessage(
        new WorkflowBoardRequestError({
          code: "digest_mismatch",
          message: "",
        }),
      ),
    ).toBe("Artifacts on disk do not match the expected revision digest.");
    expect(workflowBoardErrorMessage(new Error("Boom"))).toBe("Boom");
  });

  it("supports review stage runs and M3 delivery, recovery, DAG, staleness, and Ship gate selectors", () => {
    const task1: WorkflowBoardTaskContract = {
      taskId: "TSK-01",
      title: "First task",
      filesAllowed: [WorkflowBoardArtifactKey.make("src/a.ts")],
      blocks: ["TSK-02"],
      testingMethod: "test-first",
      status: "completed",
    };
    const task2: WorkflowBoardTaskContract = {
      taskId: "TSK-02",
      title: "Second task",
      dependsOn: ["TSK-01"],
      filesAllowed: [WorkflowBoardArtifactKey.make("src/b.ts")],
      testingMethod: "test-first",
      status: "todo",
    };
    const task3: WorkflowBoardTaskContract = {
      taskId: "TSK-03",
      title: "Third task",
      blockedBy: ["TSK-02"],
      filesAllowed: [WorkflowBoardArtifactKey.make("src/c.ts")],
      testingMethod: "test-first",
      status: "todo",
    };

    const detail: WorkflowCardDetail = {
      ...makeSummary({
        cardId: WorkflowCardId.make("c-m3"),
        stage: "execute",
        status: "active",
        deliveryStatus: "blocked",
      }),
      workspaceStrategy: { type: "root" },
      modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
      runtimeMode: "full-access",
      interactionMode: "default",
      spec: {
        title: "M3 card",
        brief: "Ship Execute",
        sourceType: "idea",
        assistanceMode: "balanced",
        acceptanceCriteria: [],
        constraints: [],
        sourceRefs: [],
        wireframesIndex: null,
        specDigest: "sha256:spec",
        updatedAt: IsoDateTime.make("2026-11-24T01:00:00.000Z"),
      },
      specApproval: null,
      stageRuns: [
        {
          id: WorkflowBoardStageRunId.make("sr-rev"),
          stage: "review",
          stableLaunchKey: "rev-1",
          status: "completed",
          cardRevision: PositiveInt.make(4),
          runId: RunId.make("run-rev"),
          threadId: ThreadId.make("thread-rev"),
          projectId: PROJECT_ID,
          workspaceStrategy: { type: "root" },
          worktreePath: null,
          branch: null,
          modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
          runtimeMode: "full-access",
          interactionMode: "default",
          lastError: null,
          createdAt: IsoDateTime.make("2026-11-24T03:00:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T03:05:00.000Z"),
          endedAt: IsoDateTime.make("2026-11-24T03:05:00.000Z"),
        },
      ],
      revisions: [],
      artifactKeys: [],
      tasks: [task1, task2, task3],
      taskExecutions: [
        {
          taskId: "TSK-01",
          status: "completed",
          threadId: ThreadId.make("thread-1"),
          runId: RunId.make("run-1"),
          stageRunId: "sr-1",
          worktreePath: "/tmp/wt",
          branch: "feat/m3",
          attempt: PositiveInt.make(1),
          startedAt: IsoDateTime.make("2026-11-24T02:00:00.000Z"),
          endedAt: IsoDateTime.make("2026-11-24T02:05:00.000Z"),
          lastError: null,
        },
      ],
      checkReceipts: [
        {
          checkId: "CHK-01",
          argv: ["node", "-e", "process.exit(0)"],
          status: "passed",
          exitCode: 0,
          logRef: null,
          productSnapshotDigest: "sha256:snap-old",
          executedAt: IsoDateTime.make("2026-11-24T02:10:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T02:10:00.000Z"),
        },
      ],
      reviewReport: {
        reviewId: "rev-1",
        stageRunId: "sr-rev",
        threadId: ThreadId.make("thread-rev"),
        status: "completed",
        verdict: "findings",
        summary: "Two items",
        productSnapshotDigest: "sha256:snap-new",
        findings: [
          {
            id: "fnd-open",
            severity: "major",
            status: "open",
            criterionId: "AC-01",
            file: WorkflowBoardArtifactKey.make("src/a.ts"),
            line: PositiveInt.make(12),
            message: "Fix edge case",
            disposition: "unresolved",
            resolutionNote: null,
            resolvedBy: null,
            resolvedAt: null,
          },
          {
            id: "fnd-fix",
            severity: "minor",
            status: "resolved",
            criterionId: null,
            file: null,
            line: null,
            message: "Needs code follow-up",
            disposition: "fix",
            resolutionNote: "Will fix",
            resolvedBy: "client",
            resolvedAt: IsoDateTime.make("2026-11-24T03:06:00.000Z"),
          },
          {
            id: "fnd-accepted",
            severity: "info",
            status: "resolved",
            criterionId: null,
            file: null,
            line: null,
            message: "Acceptable",
            disposition: "accept_risk",
            resolutionNote: "Verified",
            resolvedBy: "client",
            resolvedAt: IsoDateTime.make("2026-11-24T03:07:00.000Z"),
          },
        ],
        createdAt: IsoDateTime.make("2026-11-24T03:00:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-24T03:05:00.000Z"),
        completedAt: IsoDateTime.make("2026-11-24T03:05:00.000Z"),
      },
      acObservations: [
        {
          criterionId: "AC-01",
          method: "manual",
          status: "pass",
          checkIds: [],
          procedure: "Check UI",
          observationNote: "Looks good",
          evidenceRef: null,
          evidenceDigest: null,
          productSnapshotDigest: "sha256:snap-old",
          observedBy: "client",
          observedAt: IsoDateTime.make("2026-11-24T02:20:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T02:20:00.000Z"),
        },
      ],
      shipReadiness: {
        status: "blocked",
        ready: false,
        productSnapshotDigest: "sha256:snap-new",
        blockers: [
          {
            id: "b-1",
            kind: "check_stale",
            message: "CHK-01 stale",
            pointer: { stage: "execute", tab: "checks", targetId: "CHK-01" },
            blocking: true,
          },
        ],
        deliveryReceipt: null,
      },
    };

    expect(workflowBoardLatestStageRun(detail, "review")?.id).toBe("sr-rev");
    expect(isWorkflowBoardCardShipped(detail)).toBe(false);
    expect(
      isWorkflowBoardCardShipped(
        makeSummary({
          cardId: WorkflowCardId.make("c-shipped"),
          status: "completed",
          deliveryStatus: "shipped",
        }),
      ),
    ).toBe(true);

    expect(workflowBoardDeliveryStatusLabel("pending")).toBe("Delivery pending");
    expect(workflowBoardDeliveryStatusLabel("blocked")).toBe("Ship blocked");
    expect(workflowBoardDeliveryStatusLabel("ready")).toBe("Ready to ship");
    expect(workflowBoardDeliveryStatusLabel("shipped")).toBe("Shipped");
    expect(workflowBoardDeliveryStatusLabel("valid")).toBe("Shipped");
    expect(workflowBoardDeliveryStatusLabel("stale")).toBe("Stale");
    expect(workflowBoardDeliveryStatusLabel("superseded")).toBe("Superseded");
    expect(workflowBoardDeliveryStatusLabel("historical")).toBe("Historical");
    expect(workflowBoardCheckStatusLabel("passed")).toBe("Passed");
    expect(workflowBoardCheckStatusLabel("stale")).toBe("Stale");
    expect(workflowBoardReviewVerdictLabel("findings")).toBe("Findings");
    expect(workflowBoardAcEvidenceStatusLabel("not_applicable")).toBe("Not applicable");

    expect(workflowBoardNeedsRecovery(detail)).toBe(false);
    expect(
      workflowBoardNeedsRecovery({
        incompleteJournalCount: 1,
        hasUnreconciledEdits: false,
      }),
    ).toBe(true);
    expect(
      workflowBoardRecoverySummary({
        incompleteJournalCount: 0,
        hasUnreconciledEdits: true,
        recoveryState: {
          incompleteJournalCount: 0,
          pendingJournalEntries: [],
          externalEdits: [
            {
              id: "ext-1",
              relativePath: WorkflowBoardArtifactKey.make("spec.md"),
              expectedDigest: "a",
              actualDigest: "b",
              status: "detected",
              affectedGates: ["spec"],
              detectedAt: IsoDateTime.make("2026-11-24T03:00:00.000Z"),
              reconciledAt: null,
              summary: "Edited externally",
            },
          ],
          requiresReconciliation: true,
        },
      }),
    ).toMatchObject({
      requiresReconciliation: true,
      detectedExternalEdits: [{ id: "ext-1" }],
    });

    const statuses = workflowBoardTaskStatusMap(detail);
    expect(statuses.get("TSK-01")).toBe("completed");
    expect(statuses.get("TSK-02")).toBe("todo");
    expect(workflowBoardIsTaskDagReady(detail, "TSK-02")).toBe(true);
    expect(workflowBoardIsTaskDagReady(detail, "TSK-03")).toBe(false);
    expect(workflowBoardNextReadyTask(detail)?.taskId).toBe("TSK-02");
    expect(workflowBoardActiveTask(detail)).toBeNull();

    const withInProgress: WorkflowCardDetail = {
      ...detail,
      taskExecutions: [
        ...detail.taskExecutions!,
        {
          taskId: "TSK-02",
          status: "in_progress",
          threadId: ThreadId.make("thread-2"),
          runId: RunId.make("run-2"),
          stageRunId: "sr-2",
          worktreePath: "/tmp/wt",
          branch: "feat/m3",
          attempt: PositiveInt.make(1),
          startedAt: IsoDateTime.make("2026-11-24T03:10:00.000Z"),
          endedAt: null,
          lastError: null,
        },
      ],
    };
    expect(workflowBoardActiveTask(withInProgress)?.taskId).toBe("TSK-02");
    expect(workflowBoardNextReadyTask(withInProgress)).toBeNull();

    expect(workflowBoardUnresolvedFindings(detail).map((f) => f.id)).toEqual([
      "fnd-open",
      "fnd-fix",
    ]);
    expect(workflowBoardStaleEvidenceSummary(detail)).toEqual({
      staleChecks: 1,
      staleReview: false,
      staleAcObservations: 1,
      openViolations: 0,
      totalStale: 2,
    });

    expect(canApproveWorkflowBoardDelivery({ detail, canOperate: true })).toBe(false);
    const readyDetail: WorkflowCardDetail = {
      ...detail,
      shipReadiness: {
        status: "ready",
        ready: true,
        productSnapshotDigest: "sha256:snap-new",
        blockers: [],
        deliveryReceipt: null,
      },
    };
    expect(canApproveWorkflowBoardDelivery({ detail: readyDetail, canOperate: false })).toBe(false);
    expect(canApproveWorkflowBoardDelivery({ detail: readyDetail, canOperate: true })).toBe(true);
    expect(
      canApproveWorkflowBoardDelivery({
        detail: { ...readyDetail, status: "paused" },
        canOperate: true,
      }),
    ).toBe(false);
    expect(
      canApproveWorkflowBoardDelivery({
        detail: {
          ...readyDetail,
          status: "completed",
          deliveryStatus: "shipped",
          deliveryReceipt: {
            id: "del-1",
            approvalId: WorkflowBoardApprovalId.make("appr-del"),
            cardRevision: PositiveInt.make(8),
            productSnapshotDigest: "sha256:snap-new",
            commitSha: null,
            specDigest: "sha256:spec",
            planDigest: "sha256:plan",
            tasksDigest: "sha256:tasks",
            checksDigest: "sha256:checks",
            reviewId: "rev-1",
            acVerdictsSummary: "1/1 pass",
            approvedBy: "client",
            approvedAt: IsoDateTime.make("2026-11-24T04:00:00.000Z"),
            status: "shipped",
          },
        },
        canOperate: true,
      }),
    ).toBe(false);
  });

  it("supports M4 CLI import policy/historical selectors, labels, and path validation", () => {
    expect(workflowBoardCliImportPolicyStatusLabel("unsupported")).toBe("Requires acknowledgement");
    expect(workflowBoardCliImportPolicyStatusLabel("acknowledged")).toBe("Acknowledged");
    expect(workflowBoardCliImportHistoricalReceiptKindLabel("spec_approval")).toBe("Spec approval");
    expect(workflowBoardCliImportHistoricalReceiptKindLabel("plan_approval")).toBe("Plan approval");
    expect(workflowBoardCliImportHistoricalReceiptKindLabel("execution")).toBe("Execution");
    expect(workflowBoardCliImportHistoricalReceiptKindLabel("review")).toBe("Review");
    expect(workflowBoardCliImportHistoricalReceiptKindLabel("check")).toBe("Check");

    expect(isValidCliImportDirInput("")).toBe(true);
    expect(isValidCliImportDirInput("  ")).toBe(true);
    expect(isValidCliImportDirInput(".legion-cli")).toBe(true);
    expect(isValidCliImportDirInput("../escape")).toBe(false);
    expect(isValidCliImportDirInput(".git/hooks")).toBe(false);

    const cliCard: Pick<
      WorkflowCardDetail,
      | "specApprovalStatus"
      | "planApprovalStatus"
      | "checkReceipts"
      | "reviewReport"
      | "acObservations"
      | "fileContractReports"
      | "shipReadiness"
      | "cliImport"
    > = {
      specApprovalStatus: "historical",
      planApprovalStatus: "historical",
      checkReceipts: [
        {
          checkId: "chk-cli-exec-1",
          argv: ["node", "--test"],
          status: "historical",
          exitCode: 1,
          logRef: null,
          productSnapshotDigest: null,
          executedAt: null,
          updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        },
      ],
      reviewReport: {
        reviewId: "rev-hist",
        stageRunId: "sr-hist",
        threadId: null,
        status: "historical",
        verdict: "stale",
        summary: "Imported historical review",
        productSnapshotDigest: null,
        findings: [],
        createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        completedAt: null,
      },
      acObservations: [
        {
          criterionId: "AC-P0-01",
          method: "automated",
          status: "historical",
          checkIds: ["chk-cli-exec-1"],
          procedure: null,
          observationNote: null,
          evidenceRef: null,
          evidenceDigest: null,
          productSnapshotDigest: null,
          observedBy: null,
          observedAt: null,
          updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        },
      ],
      cliImport: {
        sourceSystem: "legion-cli",
        sourceCliDir: WorkflowBoardArtifactKey.make(".legion-cli"),
        sourceSpecId: "spec-t3-code",
        owner: "legioncode",
        importedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        lastSyncedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        sourceDigest: "sha256:cli",
        decisionIds: ["D-001"],
        assumptionIds: ["ASM-0001"],
        acceptanceCriterionIds: ["AC-P0-01"],
        historicalReceipts: [
          {
            id: "hist-1",
            kind: "spec_approval",
            status: "historical",
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/workflow/spec-approval.yaml"),
            recordedAt: null,
            recordedBy: "user",
            summary: "Approved in CLI",
            requiresNativeRevalidation: true,
          },
        ],
        unsupportedPolicies: [
          {
            key: "sandbox.requireHardened",
            category: "sandbox",
            summary: "Hardened sandbox",
            configuredValue: "true",
            status: "unsupported",
            blocksExecution: true,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
          {
            key: "dashboard",
            category: "ui",
            summary: "CLI dashboard config",
            configuredValue: null,
            status: "unsupported",
            blocksExecution: false,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
          {
            key: "qa.passScore",
            category: "qa",
            summary: "QA pass score",
            configuredValue: "85",
            status: "acknowledged",
            blocksExecution: true,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
        ],
        acknowledgedPolicies: ["qa.passScore"],
        missingLogNotes: [],
        preservedArtifactPaths: [WorkflowBoardArtifactKey.make("spec.md")],
        assurancePolicySummary: null,
      },
    };

    expect(workflowBoardUnacknowledgedBlockingPolicies(cliCard).map((p) => p.key)).toEqual([
      "sandbox.requireHardened",
    ]);
    expect(workflowBoardHasHistoricalEvidence(cliCard)).toBe(true);
    expect(
      workflowBoardHasHistoricalEvidence({
        specApprovalStatus: "valid",
        planApprovalStatus: "valid",
        checkReceipts: [],
        reviewReport: null,
        acObservations: [],
      }),
    ).toBe(false);
    expect(workflowBoardStaleEvidenceSummary(cliCard)).toEqual({
      staleChecks: 1,
      staleReview: true,
      staleAcObservations: 1,
      openViolations: 0,
      totalStale: 3,
    });
  });

  it("evaluates promoted Spec/Plan approval gates, staleness, return targets, and fix pointers", () => {
    const specDetail: Pick<
      WorkflowCardDetail,
      "spec" | "specApproval" | "exploration" | "challengeReceipt"
    > = {
      spec: {
        title: "Card",
        brief: "Brief",
        sourceType: "idea",
        assistanceMode: "balanced",
        acceptanceCriteria: [{ id: "AC-1", text: "Works" }],
        constraints: [],
        sourceRefs: [],
        wireframesIndex: null,
        specDigest: "sha256:spec",
        updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
      },
      specApproval: null,
      exploration: {
        roundIndex: NonNegativeInt.make(1),
        pendingRoundPrompt: false,
        paused: false,
        unresolvedIds: [],
        decisions: [],
      },
      challengeReceipt: {
        status: "complete",
        concerns: [],
        synthesisCount: NonNegativeInt.make(0),
        specDigest: "sha256:spec",
        interviewDigest: null,
        manualFallback: null,
        updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
      },
    };
    expect(canApproveWorkflowBoardSpec({ detail: specDetail, canOperate: true })).toBe(true);
    expect(canApproveWorkflowBoardSpec({ detail: specDetail, canOperate: false })).toBe(false);
    expect(
      canApproveWorkflowBoardSpec({
        detail: {
          ...specDetail,
          spec: { ...specDetail.spec, assistanceMode: "direct", acceptanceCriteria: [] },
        },
        canOperate: true,
      }),
    ).toBe(false);
    expect(
      canApproveWorkflowBoardSpec({
        detail: {
          ...specDetail,
          exploration: {
            ...specDetail.exploration!,
            unresolvedIds: ["dec-1"],
          },
        },
        canOperate: true,
      }),
    ).toBe(false);
    expect(
      canApproveWorkflowBoardSpec({
        detail: {
          ...specDetail,
          challengeReceipt: { ...specDetail.challengeReceipt!, status: "stale" },
        },
        canOperate: true,
      }),
    ).toBe(false);

    const planDetail = {
      cardId: WorkflowCardId.make("c-plan-gate"),
      projectId: PROJECT_ID,
      title: "Plan card",
      stage: "plan" as const,
      status: "active" as const,
      revision: PositiveInt.make(2),
      currentTask: null,
      blockerSummary: null,
      nextAction: null,
      specApprovedRevision: PositiveInt.make(2),
      specApprovalStatus: "valid" as const,
      activeThreadId: null,
      createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
      updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
      workspaceStrategy: { type: "root" as const },
      modelSelection: null,
      runtimeMode: null,
      interactionMode: null,
      spec: specDetail.spec,
      specApproval: {
        id: WorkflowBoardApprovalId.make("a-1"),
        gate: "spec" as const,
        status: "valid" as const,
        revision: PositiveInt.make(2),
        artifactDigest: "sha256:spec",
        specDigest: "sha256:spec",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        approvedBy: "client",
      },
      stageRuns: [],
      revisions: [],
      artifactKeys: [],
      plan: {
        lenses: WORKFLOW_BOARD_PLAN_LENSES.map((lens) => ({
          lens,
          applicable: false,
          summary: null,
          reason: "Not needed",
        })),
        alternatives: [
          { id: "alt-a", title: "A", summary: "Summary A", tradeoffs: "Tradeoffs A" },
          { id: "alt-b", title: "B", summary: "Summary B", tradeoffs: "Tradeoffs B" },
        ],
        alternativesRevisionCount: NonNegativeInt.make(1),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Simpler",
        strategy: "outcomes" as const,
        strategyRationale: null,
        strategyMetadata: null,
        granularity: "balanced" as const,
        tasks: [
          {
            taskId: "TSK-1",
            title: "Task 1",
            filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/a.ts")],
            testingMethod: "test-first" as const,
          },
        ],
        checks: [{ id: "chk-1", argv: ["vp", "test", "run"] as const }],
        acMappings: [{ criterionId: "AC-1", method: "automated" as const, checkIds: ["chk-1"] }],
        planDigest: "sha256:plan",
        tasksDigest: "sha256:tasks",
        updatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
      },
    } satisfies WorkflowCardDetail;

    expect(collectPlanApprovalBlockers(planDetail)).toEqual([]);
    expect(canApproveWorkflowBoardPlan({ detail: planDetail, canOperate: true })).toBe(true);
    expect(canApproveWorkflowBoardPlan({ detail: planDetail, canOperate: false })).toBe(false);
    expect(
      collectPlanApprovalBlockers({
        ...planDetail,
        plan: { ...planDetail.plan!, acMappings: [] },
      }),
    ).toEqual(["ac_mappings_incomplete"]);

    expect(
      collectDownstreamInvalidationStates({
        specApproval: null,
        specApprovalStatus: "stale",
        planApproval: null,
        planApprovalStatus: "stale",
        challengeReceipt: { ...specDetail.challengeReceipt!, status: "stale" },
        blockerSummary: "External file edited on disk",
      }),
    ).toEqual({
      specStale: true,
      planStale: true,
      challengeStale: true,
      externalEditNotice: true,
    });

    expect(
      isCheckReceiptStale({ status: "passed", productSnapshotDigest: "snap-1" }, "snap-2"),
    ).toBe(true);
    expect(
      isReviewReportStale(
        { status: "completed", verdict: "pass", productSnapshotDigest: "snap-1" },
        "snap-2",
      ),
    ).toBe(true);
    expect(
      isAcObservationStale({ status: "pass", productSnapshotDigest: "snap-1" }, "snap-1"),
    ).toBe(false);

    expect(validReturnTargetStages("spec")).toEqual([]);
    expect(validReturnTargetStages("plan")).toEqual(["spec"]);
    expect(validReturnTargetStages("execute")).toEqual(["spec", "plan"]);
    expect(validReturnTargetStages("ship")).toEqual(["spec", "plan", "execute"]);

    expect(
      resolveFixPointerNavigation({ stage: "execute", tab: "checks", targetId: "chk-1" }),
    ).toEqual({ subTab: "execute", executeSection: "checks", targetId: "chk-1" });
    expect(
      resolveFixPointerNavigation({ stage: "ship", tab: "acceptance", targetId: "AC-1" }),
    ).toEqual({ subTab: "ship", shipSection: "acceptance", targetId: "AC-1" });
  });
});
