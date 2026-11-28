//! M3 cross-language deserializer parity tests (`argv` non-empty, `>= 1` line/revision, and
//! rejecting explicit JSON `null` on non-nullable `Schema.optional(T)` fields).

use legion_contracts::workflow_board::{
    WorkflowBoardApproveDeliveryInput, WorkflowBoardCardSummary, WorkflowBoardGetCardDiffInput,
    WorkflowBoardPauseCardInput, WorkflowBoardPlanAmendmentRecord,
    WorkflowBoardPlannedCheckReceipt, WorkflowBoardProposeAmendmentInput,
    WorkflowBoardRecordManualAcceptanceInput, WorkflowBoardRecoveryEntry,
    WorkflowBoardResolveFindingInput, WorkflowBoardResumeCardInput, WorkflowBoardRetryStepInput,
    WorkflowBoardReviewFinding, WorkflowBoardReviewReport, WorkflowBoardStageRunLink,
    WorkflowBoardStartStageInput, WorkflowBoardTaskFileContractReport,
};
use serde_json::{Value, json};

use super::workflow_board_fixtures::sample_summary;

#[test]
fn m3_rejects_empty_argv_and_zero_line_or_revision() {
    assert!(
        serde_json::from_value::<WorkflowBoardPlannedCheckReceipt>(json!({
            "checkId": "chk-1",
            "argv": [],
            "status": "pending",
            "exitCode": null,
            "logRef": null,
            "productSnapshotDigest": null,
            "executedAt": null,
            "updatedAt": "2026-11-24T00:00:00Z",
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardRecoveryEntry>(json!({
            "id": "j-1",
            "targetPath": "spec.md",
            "operation": "write",
            "status": "pending",
            "expectedDigest": null,
            "previousDigest": null,
            "revision": 0,
            "lastError": null,
            "createdAt": "2026-11-24T00:00:00Z",
            "updatedAt": "2026-11-24T00:00:00Z",
            "completedAt": null,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardRecoveryEntry>(json!({
            "id": "j-1",
            "targetPath": "spec.md",
            "operation": "write",
            "status": "pending",
            "expectedDigest": null,
            "previousDigest": null,
            "revision": null,
            "lastError": null,
            "createdAt": "2026-11-24T00:00:00Z",
            "updatedAt": "2026-11-24T00:00:00Z",
            "completedAt": null,
        }))
        .is_ok()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardReviewFinding>(json!({
            "id": "f-1",
            "severity": "major",
            "criterionId": null,
            "file": null,
            "line": 0,
            "message": "Check line 0",
            "disposition": "unresolved",
            "resolutionNote": null,
            "resolvedBy": null,
            "resolvedAt": null,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardReviewFinding>(json!({
            "id": "f-1",
            "severity": "major",
            "criterionId": null,
            "file": null,
            "line": null,
            "message": "Null line allowed",
            "disposition": "unresolved",
            "resolutionNote": null,
            "resolvedBy": null,
            "resolvedAt": null,
        }))
        .is_ok()
    );
}

#[test]
fn m3_rpc_inputs_reject_explicit_null_for_non_nullable_optional_fields() {
    assert!(
        serde_json::from_value::<WorkflowBoardStartStageInput>(json!({
            "cardId": "card-m3", "expectedRevision": 1, "stage": "plan", "taskId": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardGetCardDiffInput>(json!({
            "cardId": "card-m3", "taskId": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardGetCardDiffInput>(json!({
            "cardId": "card-m3", "includePatches": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardPauseCardInput>(json!({
            "cardId": "card-m3", "expectedRevision": 1, "reason": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardResumeCardInput>(json!({
            "cardId": "card-m3", "expectedRevision": 1, "reconcileInterrupted": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardRetryStepInput>(json!({
            "cardId": "card-m3", "expectedRevision": 1, "stepKind": "review", "targetId": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardResolveFindingInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 1,
            "reviewId": null,
            "findingId": "f-1",
            "disposition": "fix",
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardRecordManualAcceptanceInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 1,
            "criterionId": "AC-01",
            "verdict": "pass",
            "note": null,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardApproveDeliveryInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 1,
            "expectedProductSnapshotDigest": null,
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WorkflowBoardProposeAmendmentInput>(json!({
            "cardId": "card-m3",
            "expectedRevision": 1,
            "targetStage": null,
            "reason": "Amend",
        }))
        .is_err()
    );
}

#[test]
fn m3_card_and_check_dtos_reject_explicit_null_for_non_nullable_optional_fields() {
    for field in ["incompleteJournalCount", "hasUnreconciledEdits"] {
        let mut summary = serde_json::to_value(sample_summary()).expect("summary");
        summary[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowBoardCardSummary>(summary).is_err(),
            "summary.{field} must reject null"
        );
    }

    assert!(
        serde_json::from_value::<WorkflowBoardTaskFileContractReport>(json!({
            "id": null,
            "taskId": "TSK-1",
            "threadId": null,
            "runId": null,
            "status": "clean",
            "changedFiles": [],
            "allowedFiles": [],
            "unexpectedFiles": [],
            "forbiddenFiles": [],
            "preservedInWorktree": true,
            "blockingReason": null,
            "updatedAt": "2026-11-24T00:00:00Z",
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardPlannedCheckReceipt>(json!({
            "id": null,
            "checkId": "chk-1",
            "argv": ["cargo", "test"],
            "status": "passed",
            "exitCode": 0,
            "logRef": null,
            "productSnapshotDigest": null,
            "executedAt": null,
            "updatedAt": "2026-11-24T00:00:00Z",
        }))
        .is_err()
    );
}

#[test]
fn m3_review_and_amendment_dtos_reject_explicit_null_for_non_nullable_optional_fields() {
    assert!(
        serde_json::from_value::<WorkflowBoardReviewFinding>(json!({
            "id": "f-1",
            "severity": "minor",
            "status": null,
            "criterionId": null,
            "file": null,
            "line": null,
            "message": "Check status",
            "disposition": "unresolved",
            "resolutionNote": null,
            "resolvedBy": null,
            "resolvedAt": null,
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<WorkflowBoardReviewReport>(json!({
            "reviewId": "rr-1",
            "stageRunId": null,
            "threadId": null,
            "status": "completed",
            "verdict": null,
            "summary": null,
            "productSnapshotDigest": null,
            "findings": [],
            "createdAt": "2026-11-24T00:00:00Z",
            "updatedAt": "2026-11-24T00:00:00Z",
            "completedAt": null,
        }))
        .is_err()
    );

    for field in ["targetStage", "status"] {
        let mut rec = json!({
            "id": "am-1",
            "cardRevision": 2,
            "reason": "Amend",
            "summary": null,
            "sourceTaskId": null,
            "unexpectedFiles": [],
            "createdAt": "2026-11-24T00:00:00Z",
        });
        rec[field] = Value::Null;
        assert!(
            serde_json::from_value::<WorkflowBoardPlanAmendmentRecord>(rec).is_err(),
            "amendment.{field} must reject null"
        );
    }

    assert!(
        serde_json::from_value::<WorkflowBoardStageRunLink>(json!({
            "id": "sr-1",
            "stage": "execute",
            "attempt": null,
            "stableLaunchKey": "k-1",
            "status": "running",
            "cardRevision": 2,
            "runId": null,
            "threadId": null,
            "projectId": "proj-1",
            "workspaceStrategy": { "type": "root" },
            "worktreePath": null,
            "branch": null,
            "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
            "runtimeMode": "auto-accept-edits",
            "interactionMode": "default",
            "lastError": null,
            "createdAt": "2026-11-24T00:00:00Z",
            "updatedAt": "2026-11-24T00:00:00Z",
            "endedAt": null,
        }))
        .is_err()
    );
}
