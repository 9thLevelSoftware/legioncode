//! M3 Project Workflow Board contracts, validators, and JSON wire parity tests.

use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceStatus, WorkflowBoardApproveDeliveryInput,
    WorkflowBoardApproveDeliveryResult, WorkflowBoardCheckStatus, WorkflowBoardDeliveryStatus,
    WorkflowBoardFileContractResolution, WorkflowBoardFileContractStatus,
    WorkflowBoardFindingDisposition, WorkflowBoardGetCardDiffInput, WorkflowBoardGetCardDiffResult,
    WorkflowBoardManualAcceptanceVerdict, WorkflowBoardPauseCardInput,
    WorkflowBoardProposeAmendmentInput, WorkflowBoardRecordManualAcceptanceInput,
    WorkflowBoardResolveFindingInput, WorkflowBoardResumeCardInput, WorkflowBoardRetryStepInput,
    WorkflowBoardReviewSeverity, WorkflowBoardReviewStatus, WorkflowBoardReviewVerdict,
    WorkflowBoardShipBlockerKind, WorkflowBoardStageRunStage, WorkflowBoardStartStageInput,
    WorkflowBoardStartStageResult, WorkflowCardDetail,
};
use serde_json::json;

use super::workflow_board_fixtures::sample_detail;

#[test]
fn m3_unified_enum_literals_match_ts_and_sqlite() {
    for stage in ["spec", "plan", "execute", "review", "ship"] {
        assert!(serde_json::from_value::<WorkflowBoardStageRunStage>(json!(stage)).is_ok());
    }
    for s in ["pending", "blocked", "ready", "shipped"] {
        assert!(serde_json::from_value::<WorkflowBoardDeliveryStatus>(json!(s)).is_ok());
    }
    for s in ["clean", "violation_pending", "resolved", "amended", "stale"] {
        assert!(serde_json::from_value::<WorkflowBoardFileContractStatus>(json!(s)).is_ok());
    }
    for s in ["amend_plan", "accept_scope", "manual_reverted", "dismiss"] {
        assert!(serde_json::from_value::<WorkflowBoardFileContractResolution>(json!(s)).is_ok());
    }
    for s in [
        "pending",
        "running",
        "passed",
        "failed",
        "stale",
        "historical",
    ] {
        assert!(serde_json::from_value::<WorkflowBoardCheckStatus>(json!(s)).is_ok());
    }
    for s in ["critical", "major", "minor", "info"] {
        assert!(serde_json::from_value::<WorkflowBoardReviewSeverity>(json!(s)).is_ok());
    }
    for s in [
        "pending",
        "running",
        "completed",
        "failed",
        "stale",
        "historical",
    ] {
        assert!(serde_json::from_value::<WorkflowBoardReviewStatus>(json!(s)).is_ok());
    }
    for s in ["pending", "pass", "findings", "failed", "stale"] {
        assert!(serde_json::from_value::<WorkflowBoardReviewVerdict>(json!(s)).is_ok());
    }
    for s in ["unresolved", "fix", "accept_risk", "dismiss", "defer"] {
        assert!(serde_json::from_value::<WorkflowBoardFindingDisposition>(json!(s)).is_ok());
    }
    for s in [
        "pending",
        "pass",
        "fail",
        "not_applicable",
        "stale",
        "historical",
    ] {
        assert!(serde_json::from_value::<WorkflowBoardAcEvidenceStatus>(json!(s)).is_ok());
    }
    for s in ["pass", "fail", "not-applicable"] {
        assert!(serde_json::from_value::<WorkflowBoardManualAcceptanceVerdict>(json!(s)).is_ok());
    }
    assert!(
        serde_json::from_value::<WorkflowBoardManualAcceptanceVerdict>(json!("not_applicable"))
            .is_err()
    );
    for b in [
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
    ] {
        assert!(serde_json::from_value::<WorkflowBoardShipBlockerKind>(json!(b)).is_ok());
    }
}

#[test]
fn m3_rpc_input_validators_match_ts() {
    assert!(
        serde_json::from_value::<WorkflowBoardStartStageInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 5,
            "stage": "review",
            "taskId": "TSK-0001",
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardPauseCardInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 5,
            "reason": "Hold",
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardResumeCardInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 6,
            "reconcileInterrupted": true,
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardGetCardDiffInput>(json!({
            "cardId": "card-m3",
            "taskId": "TSK-0001",
            "includePatches": true,
        }))
        .is_ok()
    );

    // retryStep: task/check require non-empty targetId
    assert!(
        serde_json::from_value::<WorkflowBoardRetryStepInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 6,
            "stepKind": "task",
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardRetryStepInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 6,
            "stepKind": "check",
            "targetId": "   ",
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardRetryStepInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 6,
            "stepKind": "task",
            "targetId": "TSK-0001",
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardRetryStepInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 6,
            "stepKind": "review",
        }))
        .is_ok()
    );
}

#[test]
fn m3_finding_acceptance_amendment_validators_match_ts() {
    for disp in ["dismiss", "accept_risk"] {
        assert!(
            serde_json::from_value::<WorkflowBoardResolveFindingInput>(json!({
                "cardId": "card-m3",
                "expectedRevision": 7,
                "findingId": "f-1",
                "disposition": disp,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkflowBoardResolveFindingInput>(json!({
                "cardId": "card-m3",
                "expectedRevision": 7,
                "findingId": "f-1",
                "disposition": disp,
                "reason": "Covered by integration suite",
            }))
            .is_ok()
        );
    }
    assert!(
        serde_json::from_value::<WorkflowBoardResolveFindingInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 7,
            "findingId": "f-1",
            "disposition": "fix",
        }))
        .is_ok()
    );

    // recordManualAcceptance: fail/not-applicable require note
    for verdict in ["fail", "not-applicable"] {
        assert!(
            serde_json::from_value::<WorkflowBoardRecordManualAcceptanceInput>(json!({
                "cardId": "card-m3",
                "expectedRevision": 8,
                "criterionId": "AC-01",
                "verdict": verdict,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkflowBoardRecordManualAcceptanceInput>(json!({
                "cardId": "card-m3",
                "expectedRevision": 8,
                "criterionId": "AC-01",
                "verdict": verdict,
                "note": "Verified manually",
            }))
            .is_ok()
        );
    }

    // proposeAmendment: rejects shell wrappers and invalid deliverables
    assert!(
        serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 7,
            "reason": "Add check",
            "plannedChecks": [{ "id": "chk-bad", "argv": ["bash", "-lc", "echo hi"] }],
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 7,
            "reason": "Bad deliverable",
            "deliverables": ["../../secret.txt"],
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 7,
            "targetStage": "plan",
            "reason": "Expand file contract",
            "tasks": [{
                "taskId": "TSK-0001",
                "title": "Update contracts",
                "filesAllowed": ["packages/contracts/src/workflowBoardM3.ts"],
                "testingMethod": "test-first",
            }],
            "plannedChecks": [{ "id": "chk-1", "argv": ["cargo", "test"] }],
            "deliverables": ["plan.md"],
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardApproveDeliveryInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 8,
            "expectedProductSnapshotDigest": "sha256:snap-1",
            "note": "Ready",
        }))
        .is_ok()
    );
}

#[test]
fn m3_detail_and_rpc_results_round_trip() {
    let detail_json = serde_json::to_value(sample_detail()).expect("serialize detail");
    let round_tripped: WorkflowCardDetail =
        serde_json::from_value(detail_json).expect("deserialize detail");
    assert_eq!(round_tripped.task_executions.len(), 0);

    let start_review: WorkflowBoardStartStageResult = serde_json::from_value(json!({
        "cardId": "card-m3",
        "revision": 8,
        "stage": "review",
        "taskId": null,
        "stageRunId": "sr-rev-1",
        "runId": "run-rev-1",
        "threadId": "thr-rev-1",
        "resumed": false,
    }))
    .expect("review start result");
    assert_eq!(start_review.stage, WorkflowBoardStageRunStage::Review);

    let diff_res: WorkflowBoardGetCardDiffResult = serde_json::from_value(json!({
        "cardId": "card-m3",
        "worktreePath": "/repo/.worktrees/card-m3",
        "baseRef": "main",
        "productSnapshotDigest": "sha256:snap-1",
        "diffDigest": "sha256:diff-1",
        "files": [{
            "path": "src/a.ts",
            "previousPath": null,
            "changeType": "change",
            "additions": 4,
            "deletions": 1,
            "patch": "@@ -1 +1 @@",
            "contractStatus": "allowed",
        }],
        "fileContractReport": null,
        "truncated": false,
        "generatedAt": "2026-11-24T02:00:00Z",
    }))
    .expect("diff result");
    assert_eq!(diff_res.files.len(), 1);

    let approve_res: WorkflowBoardApproveDeliveryResult = serde_json::from_value(json!({
        "cardId": "card-m3",
        "revision": 9,
        "status": "completed",
        "deliveryStatus": "shipped",
        "deliveryReceipt": {
            "id": "del-1",
            "approvalId": "apr-del-1",
            "cardRevision": 9,
            "productSnapshotDigest": "sha256:snap-1",
            "commitSha": null,
            "specDigest": "sha256:spec-1",
            "planDigest": "sha256:plan-1",
            "tasksDigest": "sha256:tasks-1",
            "checksDigest": "sha256:checks-1",
            "reviewId": "rev-1",
            "acVerdictsSummary": "{\"pass\":1}",
            "approvedBy": "client",
            "approvedAt": "2026-11-24T03:00:00Z",
            "status": "shipped",
        },
        "deliveryApproval": {
            "id": "apr-del-1",
            "gate": "delivery",
            "status": "valid",
            "revision": 9,
            "artifactDigest": "sha256:snap-1",
            "specDigest": "sha256:spec-1",
            "wireframesDigest": null,
            "approvedAt": "2026-11-24T03:00:00Z",
            "approvedBy": "client",
            "productSnapshotDigest": "sha256:snap-1",
        },
        "card": serde_json::to_value(sample_detail()).expect("detail"),
    }))
    .expect("approveDelivery result");
    assert_eq!(
        approve_res.delivery_status,
        WorkflowBoardDeliveryStatus::Shipped
    );
}
