import * as Exit from "effect/Exit";
import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import { AuthOrchestrationOperateScope, AuthOrchestrationReadScope } from "./auth.ts";
import { CLIENT_GUARDED_RPC_SCOPES, clientRpcRequiredScopes } from "./clientRpcPermissions.ts";
import { WS_METHODS, WsRpcGroup } from "./rpc.ts";
import {
  WorkflowBoardApprovalReceipt,
  WorkflowBoardApproveDeliveryInput,
  WorkflowBoardApproveDeliveryResult,
  WorkflowBoardApproveSpecInput,
  WorkflowBoardCardSummary,
  WorkflowBoardCreateCardInput,
  WorkflowBoardGetArtifactInput,
  WorkflowBoardGetCardDiffInput,
  WorkflowBoardGetCardDiffResult,
  WorkflowBoardGetCardInput,
  WorkflowBoardPauseCardInput,
  WorkflowBoardProposeAmendmentInput,
  WorkflowBoardRecordManualAcceptanceInput,
  WorkflowBoardResolveFindingInput,
  WorkflowBoardResumeCardInput,
  WorkflowBoardRetryStepInput,
  WorkflowBoardStartStageInput,
  WorkflowBoardStartStageResult,
  WorkflowBoardUpdateSpecInput,
  WorkflowCardDetail,
} from "./workflowBoard.ts";
import {
  WorkflowBoardAcEvidenceStatus,
  WorkflowBoardCheckStatus,
  WorkflowBoardDeliveryStatus,
  WorkflowBoardFileContractResolution,
  WorkflowBoardFileContractStatus,
  WorkflowBoardFindingDisposition,
  WorkflowBoardManualAcceptanceVerdict,
  WorkflowBoardPlanAmendmentRecord,
  WorkflowBoardPlannedCheckReceipt,
  WorkflowBoardRecoveryEntry,
  WorkflowBoardReviewFinding,
  WorkflowBoardReviewReport,
  WorkflowBoardReviewSeverity,
  WorkflowBoardReviewStatus,
  WorkflowBoardReviewVerdict,
  WorkflowBoardShipBlockerKind,
  WorkflowBoardStageRunStage,
  WorkflowBoardTaskFileContractReport,
} from "./workflowBoardM3.ts";

const decodeStageRunStage = Schema.decodeUnknownExit(WorkflowBoardStageRunStage);
const decodeDeliveryStatus = Schema.decodeUnknownExit(WorkflowBoardDeliveryStatus);
const decodeFileContractStatus = Schema.decodeUnknownExit(WorkflowBoardFileContractStatus);
const decodeFileContractResolution = Schema.decodeUnknownExit(WorkflowBoardFileContractResolution);
const decodeCheckStatus = Schema.decodeUnknownExit(WorkflowBoardCheckStatus);
const decodeReviewSeverity = Schema.decodeUnknownExit(WorkflowBoardReviewSeverity);
const decodeReviewStatus = Schema.decodeUnknownExit(WorkflowBoardReviewStatus);
const decodeReviewVerdict = Schema.decodeUnknownExit(WorkflowBoardReviewVerdict);
const decodeFindingDisposition = Schema.decodeUnknownExit(WorkflowBoardFindingDisposition);
const decodeAcStatus = Schema.decodeUnknownExit(WorkflowBoardAcEvidenceStatus);
const decodeManualVerdict = Schema.decodeUnknownExit(WorkflowBoardManualAcceptanceVerdict);
const decodeBlockerKind = Schema.decodeUnknownExit(WorkflowBoardShipBlockerKind);

const decodeStartStage = Schema.decodeUnknownExit(WorkflowBoardStartStageInput);
const decodePauseCard = Schema.decodeUnknownExit(WorkflowBoardPauseCardInput);
const decodeResumeCard = Schema.decodeUnknownExit(WorkflowBoardResumeCardInput);
const decodeRetryStep = Schema.decodeUnknownExit(WorkflowBoardRetryStepInput);
const decodeProposeAmendment = Schema.decodeUnknownExit(WorkflowBoardProposeAmendmentInput);
const decodeResolveFinding = Schema.decodeUnknownExit(WorkflowBoardResolveFindingInput);
const decodeManualAcceptance = Schema.decodeUnknownExit(WorkflowBoardRecordManualAcceptanceInput);
const decodeApproveDelivery = Schema.decodeUnknownExit(WorkflowBoardApproveDeliveryInput);
const decodeGetCardDiff = Schema.decodeUnknownExit(WorkflowBoardGetCardDiffInput);

const cardDetailCodec = Schema.toCodecJson(WorkflowCardDetail);
const startStageResultCodec = Schema.toCodecJson(WorkflowBoardStartStageResult);
const getCardDiffResultCodec = Schema.toCodecJson(WorkflowBoardGetCardDiffResult);
const approveDeliveryResultCodec = Schema.toCodecJson(WorkflowBoardApproveDeliveryResult);

const m3DetailWire = {
  cardId: "card-m3",
  projectId: "proj-1",
  title: "M3 Execute and Ship",
  stage: "ship",
  status: "completed",
  revision: 9,
  currentTask: null,
  blockerSummary: null,
  nextAction: "git_actions",
  specApprovedRevision: 3,
  specApprovalStatus: "valid",
  activeThreadId: "thr-rev-1",
  createdAt: "2026-11-24T00:00:00Z",
  updatedAt: "2026-11-24T03:00:00Z",
  planApprovedRevision: 5,
  planApprovalStatus: "valid",
  incompleteJournalCount: 0,
  hasUnreconciledEdits: false,
  deliveryStatus: "shipped",
  workspaceStrategy: { type: "root" },
  modelSelection: { instanceId: "devReplay", model: "dev-replay" },
  runtimeMode: "auto-accept-edits",
  interactionMode: "default",
  spec: {
    title: "M3 Execute and Ship",
    brief: "Deliver execution and ship gates",
    sourceType: "idea",
    assistanceMode: "balanced",
    acceptanceCriteria: [{ id: "AC-01", text: "Ship requires zero blockers" }],
    constraints: ["No auto-commits"],
    sourceRefs: ["README.md"],
    wireframesIndex: null,
    specDigest: "sha256:spec-m3",
    updatedAt: "2026-11-24T00:30:00Z",
  },
  specApproval: null,
  stageRuns: [
    {
      id: "sr-rev-1",
      stage: "review",
      taskId: null,
      attempt: 1,
      stableLaunchKey: "board:card-m3:review:1",
      status: "completed",
      cardRevision: 8,
      runId: "run-rev-1",
      threadId: "thr-rev-1",
      projectId: "proj-1",
      workspaceStrategy: { type: "root" },
      worktreePath: "/repo/.worktrees/card-m3",
      branch: "feat/card-m3",
      modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      runtimeMode: "auto-accept-edits",
      interactionMode: "default",
      lastError: null,
      createdAt: "2026-11-24T02:00:00Z",
      updatedAt: "2026-11-24T02:30:00Z",
      endedAt: "2026-11-24T02:30:00Z",
    },
  ],
  revisions: [],
  artifactKeys: ["card.json", "spec.md", "plan.md"],
  taskExecutions: [
    {
      taskId: "TSK-0001",
      status: "completed",
      threadId: "thr-tsk-1",
      runId: "run-tsk-1",
      stageRunId: "sr-tsk-1",
      worktreePath: "/repo/.worktrees/card-m3",
      branch: "feat/card-m3",
      attempt: 1,
      startedAt: "2026-11-24T01:00:00Z",
      endedAt: "2026-11-24T01:20:00Z",
      lastError: null,
    },
  ],
  fileContractReports: [
    {
      taskId: "TSK-0001",
      threadId: "thr-tsk-1",
      runId: "run-tsk-1",
      status: "clean",
      changedFiles: ["src/a.ts"],
      allowedFiles: ["src/a.ts"],
      unexpectedFiles: [],
      forbiddenFiles: [],
      preservedInWorktree: true,
      blockingReason: null,
      updatedAt: "2026-11-24T01:20:00Z",
    },
  ],
  checkReceipts: [
    {
      checkId: "chk-1",
      criterionId: "AC-01",
      ownerTaskId: "TSK-0001",
      argv: ["cargo", "test"],
      description: "Run persistence tests",
      status: "passed",
      exitCode: 0,
      logRef: "logs/chk-1.log",
      productSnapshotDigest: "sha256:snap-1",
      executedAt: "2026-11-24T01:30:00Z",
      updatedAt: "2026-11-24T01:30:00Z",
    },
  ],
  reviewReport: {
    reviewId: "rev-run-1",
    stageRunId: "sr-rev-1",
    threadId: "thr-rev-1",
    status: "completed",
    verdict: "pass",
    summary: "Clean review",
    productSnapshotDigest: "sha256:snap-1",
    findings: [],
    createdAt: "2026-11-24T02:00:00Z",
    updatedAt: "2026-11-24T02:30:00Z",
    completedAt: "2026-11-24T02:30:00Z",
  },
  acObservations: [
    {
      criterionId: "AC-01",
      method: "manual",
      status: "not_applicable",
      checkIds: [],
      procedure: "Inspect delivery gate",
      observationNote: "Backend-only change",
      evidenceRef: null,
      evidenceDigest: null,
      productSnapshotDigest: "sha256:snap-1",
      observedBy: "client",
      observedAt: "2026-11-24T02:45:00Z",
      updatedAt: "2026-11-24T02:45:00Z",
    },
  ],
  deliveryReceipt: {
    id: "del-1",
    approvalId: "apr-del-1",
    cardRevision: 9,
    productSnapshotDigest: "sha256:snap-1",
    commitSha: null,
    specDigest: "sha256:spec-m3",
    planDigest: "sha256:plan-m3",
    tasksDigest: "sha256:tasks-m3",
    checksDigest: "sha256:checks-m3",
    reviewId: "rev-run-1",
    acVerdictsSummary: '{"not_applicable":1}',
    approvedBy: "client",
    approvedAt: "2026-11-24T03:00:00Z",
    status: "shipped",
  },
  shipReadiness: {
    status: "shipped",
    ready: true,
    productSnapshotDigest: "sha256:snap-1",
    blockers: [],
    deliveryReceipt: null,
  },
  recoveryState: {
    incompleteJournalCount: 0,
    pendingJournalEntries: [],
    externalEdits: [],
    requiresReconciliation: false,
  },
  amendments: [],
} as const;

describe("workflowBoard M3 contracts and permissions", () => {
  it("validates unified M3 enum literals and StageRunStage extension", () => {
    for (const s of ["spec", "plan", "execute", "review", "ship"]) {
      expect(Exit.isSuccess(decodeStageRunStage(s))).toBe(true);
    }
    expect(Exit.isFailure(decodeStageRunStage("unknown"))).toBe(true);

    for (const s of ["pending", "blocked", "ready", "shipped"]) {
      expect(Exit.isSuccess(decodeDeliveryStatus(s))).toBe(true);
    }
    for (const s of ["clean", "violation_pending", "resolved", "amended", "stale"]) {
      expect(Exit.isSuccess(decodeFileContractStatus(s))).toBe(true);
    }
    for (const s of ["amend_plan", "accept_scope", "manual_reverted", "dismiss"]) {
      expect(Exit.isSuccess(decodeFileContractResolution(s))).toBe(true);
    }
    for (const s of ["pending", "running", "passed", "failed", "stale", "historical"]) {
      expect(Exit.isSuccess(decodeCheckStatus(s))).toBe(true);
    }
    for (const s of ["critical", "major", "minor", "info"]) {
      expect(Exit.isSuccess(decodeReviewSeverity(s))).toBe(true);
    }
    for (const s of ["pending", "running", "completed", "failed", "stale", "historical"]) {
      expect(Exit.isSuccess(decodeReviewStatus(s))).toBe(true);
    }
    for (const s of ["pending", "pass", "findings", "failed", "stale"]) {
      expect(Exit.isSuccess(decodeReviewVerdict(s))).toBe(true);
    }
    for (const s of ["unresolved", "fix", "accept_risk", "dismiss", "defer"]) {
      expect(Exit.isSuccess(decodeFindingDisposition(s))).toBe(true);
    }
    for (const s of ["pending", "pass", "fail", "not_applicable", "stale", "historical"]) {
      expect(Exit.isSuccess(decodeAcStatus(s))).toBe(true);
    }
    for (const s of ["pass", "fail", "not-applicable"]) {
      expect(Exit.isSuccess(decodeManualVerdict(s))).toBe(true);
    }
    expect(Exit.isFailure(decodeManualVerdict("not_applicable"))).toBe(true);

    for (const b of [
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
    ]) {
      expect(Exit.isSuccess(decodeBlockerKind(b))).toBe(true);
    }
  });

  it("enforces symmetric validators on retryStep, resolveFinding, recordManualAcceptance, and proposeAmendment", () => {
    expect(
      Exit.isSuccess(
        decodeStartStage({
          cardId: "card-m3",
          expectedRevision: 5,
          stage: "review",
          taskId: "TSK-0001",
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodePauseCard({ cardId: "card-m3", expectedRevision: 5, reason: "Operator break" }),
      ),
    ).toBe(true);
    expect(
      Exit.isSuccess(
        decodeResumeCard({
          cardId: "card-m3",
          expectedRevision: 6,
          reconcileInterrupted: true,
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isSuccess(
        decodeGetCardDiff({ cardId: "card-m3", taskId: "TSK-0001", includePatches: true }),
      ),
    ).toBe(true);

    // retryStep: task/check require non-empty targetId; review/stage do not
    expect(
      Exit.isFailure(decodeRetryStep({ cardId: "card-m3", expectedRevision: 6, stepKind: "task" })),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeRetryStep({
          cardId: "card-m3",
          expectedRevision: 6,
          stepKind: "check",
          targetId: "   ",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isSuccess(
        decodeRetryStep({
          cardId: "card-m3",
          expectedRevision: 6,
          stepKind: "task",
          targetId: "TSK-0001",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isSuccess(
        decodeRetryStep({ cardId: "card-m3", expectedRevision: 6, stepKind: "review" }),
      ),
    ).toBe(true);

    // resolveFinding: dismiss and accept_risk require non-empty reason
    for (const disp of ["dismiss", "accept_risk"] as const) {
      expect(
        Exit.isFailure(
          decodeResolveFinding({
            cardId: "card-m3",
            expectedRevision: 7,
            findingId: "f-1",
            disposition: disp,
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeResolveFinding({
            cardId: "card-m3",
            expectedRevision: 7,
            findingId: "f-1",
            disposition: disp,
            reason: "   ",
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isSuccess(
          decodeResolveFinding({
            cardId: "card-m3",
            expectedRevision: 7,
            findingId: "f-1",
            disposition: disp,
            reason: "Covered by integration suite",
          }),
        ),
      ).toBe(true);
    }
    expect(
      Exit.isSuccess(
        decodeResolveFinding({
          cardId: "card-m3",
          expectedRevision: 7,
          findingId: "f-1",
          disposition: "fix",
        }),
      ),
    ).toBe(true);

    // recordManualAcceptance: fail and not-applicable require non-empty note
    for (const verdict of ["fail", "not-applicable"] as const) {
      expect(
        Exit.isFailure(
          decodeManualAcceptance({
            cardId: "card-m3",
            expectedRevision: 8,
            criterionId: "AC-01",
            verdict,
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isSuccess(
          decodeManualAcceptance({
            cardId: "card-m3",
            expectedRevision: 8,
            criterionId: "AC-01",
            verdict,
            note: "Verified manually",
          }),
        ),
      ).toBe(true);
    }
    expect(
      Exit.isSuccess(
        decodeManualAcceptance({
          cardId: "card-m3",
          expectedRevision: 8,
          criterionId: "AC-01",
          verdict: "pass",
        }),
      ),
    ).toBe(true);

    // proposeAmendment: rejects shell wrappers in plannedChecks and overlapping filesAllowed
    expect(
      Exit.isFailure(
        decodeProposeAmendment({
          cardId: "card-m3",
          expectedRevision: 7,
          reason: "Add check",
          plannedChecks: [{ id: "chk-bad", argv: ["bash", "-lc", "echo hi"] }],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeProposeAmendment({
          cardId: "card-m3",
          expectedRevision: 7,
          reason: "Bad deliverables",
          deliverables: ["../../secret.txt"],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isSuccess(
        decodeProposeAmendment({
          cardId: "card-m3",
          expectedRevision: 7,
          targetStage: "plan",
          reason: "Expand file contract",
          tasks: [
            {
              taskId: "TSK-0001",
              title: "Update contracts",
              filesAllowed: ["packages/contracts/src/workflowBoardM3.ts"],
              testingMethod: "test-first",
            },
          ],
          plannedChecks: [{ id: "chk-1", argv: ["vp", "test", "run"] }],
          deliverables: ["plan.md"],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isSuccess(
        decodeApproveDelivery({
          cardId: "card-m3",
          expectedRevision: 8,
          expectedProductSnapshotDigest: "sha256:snap-1",
          note: "Ready to ship",
        }),
      ),
    ).toBe(true);
  });

  it("round-trips M3 card detail, startStage review result, getCardDiff result, and approveDelivery result", () => {
    const decodedDetail = Schema.decodeSync(cardDetailCodec)(m3DetailWire);
    expect(Schema.encodeSync(cardDetailCodec)(decodedDetail)).toEqual(m3DetailWire);

    const startReviewRes = {
      cardId: "card-m3",
      revision: 8,
      stage: "review",
      taskId: null,
      stageRunId: "sr-rev-1",
      runId: "run-rev-1",
      threadId: "thr-rev-1",
      resumed: false,
    } as const;
    expect(
      Schema.encodeSync(startStageResultCodec)(
        Schema.decodeSync(startStageResultCodec)(startReviewRes),
      ),
    ).toEqual(startReviewRes);

    const diffRes = {
      cardId: "card-m3",
      worktreePath: "/repo/.worktrees/card-m3",
      baseRef: "main",
      productSnapshotDigest: "sha256:snap-1",
      diffDigest: "sha256:diff-1",
      files: [
        {
          path: "src/a.ts",
          previousPath: null,
          changeType: "change",
          additions: 12,
          deletions: 3,
          patch: "@@ -1,3 +1,12 @@",
          contractStatus: "allowed",
        },
      ],
      fileContractReport: null,
      truncated: false,
      generatedAt: "2026-11-24T02:00:00Z",
    } as const;
    expect(
      Schema.encodeSync(getCardDiffResultCodec)(Schema.decodeSync(getCardDiffResultCodec)(diffRes)),
    ).toEqual(diffRes);

    const approveRes = {
      cardId: "card-m3",
      revision: 9,
      status: "completed",
      deliveryStatus: "shipped",
      deliveryReceipt: m3DetailWire.deliveryReceipt,
      deliveryApproval: {
        id: "apr-del-1",
        gate: "delivery",
        status: "valid",
        revision: 9,
        artifactDigest: "sha256:snap-1",
        specDigest: "sha256:spec-m3",
        wireframesDigest: null,
        approvedAt: "2026-11-24T03:00:00Z",
        approvedBy: "client",
        planDigest: "sha256:plan-m3",
        tasksDigest: "sha256:tasks-m3",
        checksDigest: "sha256:checks-m3",
        productSnapshotDigest: "sha256:snap-1",
      },
      card: m3DetailWire,
    } as const;
    expect(
      Schema.encodeSync(approveDeliveryResultCodec)(
        Schema.decodeSync(approveDeliveryResultCodec)(approveRes),
      ),
    ).toEqual(approveRes);
  });

  it("maps M3 RPC scopes via clientRpcRequiredScopes and wires all 8 M3 RPCs into WsRpcGroup/CLIENT_GUARDED_RPC_SCOPES", () => {
    expect(clientRpcRequiredScopes(WS_METHODS.workflowBoardGetCardDiff, {})).toEqual([
      AuthOrchestrationReadScope,
    ]);
    expect(Object.hasOwn(CLIENT_GUARDED_RPC_SCOPES, WS_METHODS.workflowBoardGetCardDiff)).toBe(
      true,
    );
    expect(WsRpcGroup.requests.has(WS_METHODS.workflowBoardGetCardDiff)).toBe(true);

    for (const method of [
      WS_METHODS.workflowBoardPauseCard,
      WS_METHODS.workflowBoardResumeCard,
      WS_METHODS.workflowBoardRetryStep,
      WS_METHODS.workflowBoardProposeAmendment,
      WS_METHODS.workflowBoardResolveFinding,
      WS_METHODS.workflowBoardRecordManualAcceptance,
      WS_METHODS.workflowBoardApproveDelivery,
    ]) {
      expect(clientRpcRequiredScopes(method, {})).toEqual([AuthOrchestrationOperateScope]);
      expect(Object.hasOwn(CLIENT_GUARDED_RPC_SCOPES, method)).toBe(true);
      expect(WsRpcGroup.requests.has(method)).toBe(true);
    }
  });

  it("rejects empty argv, zero revision/line, and explicit null on non-nullable optional fields", () => {
    const decodeReceipt = Schema.decodeUnknownExit(WorkflowBoardPlannedCheckReceipt);
    const decodeRecovery = Schema.decodeUnknownExit(WorkflowBoardRecoveryEntry);
    const decodeFinding = Schema.decodeUnknownExit(WorkflowBoardReviewFinding);
    const decodeReport = Schema.decodeUnknownExit(WorkflowBoardReviewReport);
    const decodeContractReport = Schema.decodeUnknownExit(WorkflowBoardTaskFileContractReport);
    const decodeAmendment = Schema.decodeUnknownExit(WorkflowBoardPlanAmendmentRecord);
    const decodeSummary = Schema.decodeUnknownExit(WorkflowBoardCardSummary);

    expect(
      Exit.isFailure(
        decodeReceipt({
          checkId: "chk-1",
          argv: [],
          status: "pending",
          exitCode: null,
          logRef: null,
          productSnapshotDigest: null,
          executedAt: null,
          updatedAt: "2026-11-24T00:00:00Z",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeRecovery({
          id: "j-1",
          targetPath: "spec.md",
          operation: "write",
          status: "pending",
          expectedDigest: null,
          previousDigest: null,
          revision: 0,
          lastError: null,
          createdAt: "2026-11-24T00:00:00Z",
          updatedAt: "2026-11-24T00:00:00Z",
          completedAt: null,
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeFinding({
          id: "f-1",
          severity: "major",
          criterionId: null,
          file: null,
          line: 0,
          message: "Check line 0",
          disposition: "unresolved",
          resolutionNote: null,
          resolvedBy: null,
          resolvedAt: null,
        }),
      ),
    ).toBe(true);

    // Explicit null on non-nullable optional fields is rejected across M3 RPC inputs and DTOs
    expect(
      Exit.isFailure(
        decodeStartStage({ cardId: "c", expectedRevision: 1, stage: "plan", taskId: null }),
      ),
    ).toBe(true);
    expect(Exit.isFailure(decodeGetCardDiff({ cardId: "c", taskId: null }))).toBe(true);
    expect(
      Exit.isFailure(decodePauseCard({ cardId: "c", expectedRevision: 1, reason: null })),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeResumeCard({ cardId: "c", expectedRevision: 1, reconcileInterrupted: null }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeRetryStep({ cardId: "c", expectedRevision: 1, stepKind: "review", targetId: null }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeResolveFinding({
          cardId: "c",
          expectedRevision: 1,
          reviewId: null,
          findingId: "f-1",
          disposition: "fix",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeManualAcceptance({
          cardId: "c",
          expectedRevision: 1,
          criterionId: "AC-01",
          verdict: "pass",
          note: null,
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeApproveDelivery({
          cardId: "c",
          expectedRevision: 1,
          expectedProductSnapshotDigest: null,
        }),
      ),
    ).toBe(true);
    expect(Exit.isFailure(decodeSummary({ ...m3DetailWire, incompleteJournalCount: null }))).toBe(
      true,
    );
    expect(Exit.isFailure(decodeSummary({ ...m3DetailWire, hasUnreconciledEdits: null }))).toBe(
      true,
    );
    expect(
      Exit.isFailure(
        decodeContractReport({
          id: null,
          taskId: "TSK-1",
          threadId: null,
          runId: null,
          status: "clean",
          changedFiles: [],
          allowedFiles: [],
          unexpectedFiles: [],
          forbiddenFiles: [],
          preservedInWorktree: true,
          blockingReason: null,
          updatedAt: "2026-11-24T00:00:00Z",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeFinding({
          id: "f-1",
          severity: "minor",
          status: null,
          criterionId: null,
          file: null,
          line: null,
          message: "Status null",
          disposition: "unresolved",
          resolutionNote: null,
          resolvedBy: null,
          resolvedAt: null,
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeReport({
          reviewId: "rr-1",
          stageRunId: null,
          threadId: null,
          status: "completed",
          verdict: null,
          summary: null,
          productSnapshotDigest: null,
          findings: [],
          createdAt: "2026-11-24T00:00:00Z",
          updatedAt: "2026-11-24T00:00:00Z",
          completedAt: null,
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeAmendment({
          id: "am-1",
          cardRevision: 2,
          targetStage: null,
          reason: "Amend",
          summary: null,
          sourceTaskId: null,
          unexpectedFiles: [],
          createdAt: "2026-11-24T00:00:00Z",
        }),
      ),
    ).toBe(true);
  });

  it("rejects explicit null on nested amendment M2 fields, approvalReceipt, cardDetail, and M1/M2 RPC optional fields", () => {
    const decodeGetCard = Schema.decodeUnknownExit(WorkflowBoardGetCardInput);
    const decodeGetArtifact = Schema.decodeUnknownExit(WorkflowBoardGetArtifactInput);
    const decodeCreateCard = Schema.decodeUnknownExit(WorkflowBoardCreateCardInput);
    const decodeUpdateSpec = Schema.decodeUnknownExit(WorkflowBoardUpdateSpecInput);
    const decodeApproveSpec = Schema.decodeUnknownExit(WorkflowBoardApproveSpecInput);
    const decodeReceipt = Schema.decodeUnknownExit(WorkflowBoardApprovalReceipt);
    const decodeDetail = Schema.decodeUnknownExit(WorkflowCardDetail);

    for (const field of [
      "id",
      "dependsOn",
      "blockedBy",
      "blocks",
      "filesForbidden",
      "expectedArtifacts",
      "verificationCommands",
      "status",
    ] as const) {
      expect(
        Exit.isFailure(
          decodeProposeAmendment({
            cardId: "card-m3",
            expectedRevision: 1,
            reason: "Amend",
            tasks: [
              {
                taskId: "TSK-1",
                title: "Task",
                filesAllowed: ["src/a.ts"],
                testingMethod: "test-first",
                [field]: null,
              },
            ],
          }),
        ),
      ).toBe(true);
    }

    expect(
      Exit.isFailure(
        decodeProposeAmendment({
          cardId: "card-m3",
          expectedRevision: 1,
          reason: "Amend",
          plannedChecks: [{ id: "chk-1", criterionId: null, argv: ["cargo", "test"] }],
        }),
      ),
    ).toBe(true);

    for (const [method, field] of [
      ["automated", "checkIds"],
      ["automated", "command"],
      ["manual", "procedure"],
      ["external", "externalRef"],
      ["external", "externalSource"],
    ] as const) {
      expect(
        Exit.isFailure(
          decodeProposeAmendment({
            cardId: "card-m3",
            expectedRevision: 1,
            reason: "Amend",
            acMappings: [
              {
                criterionId: "AC-01",
                method,
                checkIds: ["chk-1"],
                procedure: "Manual step",
                externalRef: "ext-1",
                externalSource: "ci",
                [field]: null,
              },
            ],
          }),
        ),
      ).toBe(true);
    }

    expect(Exit.isFailure(decodeGetCard({ cardId: "card-1", projectId: null }))).toBe(true);
    expect(Exit.isFailure(decodeGetArtifact({ cardId: "card-1", key: null, kind: "spec" }))).toBe(
      true,
    );
    expect(
      Exit.isFailure(decodeGetArtifact({ cardId: "card-1", key: "spec.md", kind: null })),
    ).toBe(true);

    for (const field of [
      "brief",
      "briefPath",
      "inputPaths",
      "intakeGoal",
      "wireframesEnabled",
      "sourceType",
      "assistanceMode",
      "workspaceStrategy",
      "modelSelection",
      "runtimeMode",
      "interactionMode",
    ] as const) {
      expect(
        Exit.isFailure(decodeCreateCard({ projectId: "proj-1", title: "Card", [field]: null })),
      ).toBe(true);
    }

    for (const field of [
      "title",
      "brief",
      "intakeGoal",
      "inputPaths",
      "acceptanceCriteria",
      "constraints",
      "sourceRefs",
    ] as const) {
      expect(
        Exit.isFailure(decodeUpdateSpec({ cardId: "card-1", expectedRevision: 1, [field]: null })),
      ).toBe(true);
    }

    expect(
      Exit.isFailure(
        decodeApproveSpec({ cardId: "card-1", expectedRevision: 1, expectedDigest: null }),
      ),
    ).toBe(true);

    expect(
      Exit.isFailure(
        decodeReceipt({
          id: "apr-1",
          gate: "plan",
          status: "valid",
          revision: 1,
          artifactDigest: "sha256:plan",
          specDigest: "sha256:spec",
          wireframesDigest: null,
          approvedAt: "2026-11-24T00:00:00Z",
          approvedBy: "client",
          acceptanceCriteriaIds: null,
        }),
      ),
    ).toBe(true);

    for (const field of ["maxChallengeConcerns", "alternativesRevisionCount"] as const) {
      expect(Exit.isFailure(decodeDetail({ ...m3DetailWire, [field]: null }))).toBe(true);
    }
    for (const nullableField of [
      "wireframesIndex",
      "strategy",
      "strategyMetadata",
      "granularity",
    ] as const) {
      expect(Exit.isSuccess(decodeDetail({ ...m3DetailWire, [nullableField]: null }))).toBe(true);
    }
  });
});
