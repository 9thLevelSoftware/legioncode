#![forbid(unsafe_code)]
//! Human-only Ship & Review handlers (`resolveFinding`, `recordManualAcceptance`, `approveDelivery`).

use std::sync::Arc;

use legion_contracts::workflow_board::{
    WorkflowBoardApproveDeliveryInput, WorkflowBoardApproveDeliveryResult, WorkflowBoardCardStatus,
    WorkflowBoardDeliveryStatus, WorkflowBoardRecordManualAcceptanceInput,
    WorkflowBoardRecordManualAcceptanceResult, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardResolveFindingInput,
    WorkflowBoardResolveFindingResult,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m3::{
    ApproveDeliveryInput, RecordManualAcceptanceInput, ResolveFindingInput,
};
use legion_persistence::workflow_board_repo::{
    approve_delivery, get_card_execution_detail, record_manual_acceptance, resolve_finding,
};
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_checks::sync_card_json_file;
use crate::workflow_board_diff::get_card_diff_impl;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, resolve_card_workspace_root, validate_safe_card_id,
};
use crate::workflow_board_hydration::to_approval_receipt;
use crate::workflow_board_hydration_m3::load_hydrated_card_detail;
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_snapshot::{reconcile_card_and_snapshot, resolve_head_commit_sha};
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, internal, map_repo_err, parse_expected_rev,
};

fn ensure_human_client_caller(caller_kind: &str) -> Result<(), WorkflowBoardRequestError> {
    let trimmed = caller_kind.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("thread:")
        || trimmed.starts_with("mcp")
        || trimmed.starts_with("peer-link:")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!("Human client caller required; `{trimmed}` is not permitted"),
        ));
    }
    Ok(())
}

pub(crate) async fn resolve_finding_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardResolveFindingInput,
    resolved_by: &str,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardResolveFindingResult, WorkflowBoardRequestError> {
    ensure_human_client_caller(resolved_by)?;
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;
    let disp_str = enum_as_str(&input.disposition).ok_or_else(|| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "Invalid finding disposition",
        )
    })?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let current = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    if invalidated {
        notify_project(&current.planning.card_detail.card.project_id);
    }
    check_revision(
        card_id,
        current.planning.card_detail.card.revision,
        input.expected_revision,
    )?;

    let ws_root = resolve_card_workspace_root(&conn, &current.planning.card_detail.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let updated = resolve_finding(
        &conn,
        &ResolveFindingInput {
            card_id,
            revision_id: &rev_id,
            expected_revision: expected_rev_i64,
            review_id: input.review_id.as_deref(),
            finding_id: input.finding_id.trim(),
            disposition: &disp_str,
            reason: input.reason.as_deref(),
            resolved_by,
        },
        &now_iso,
    )
    .map_err(map_repo_err)?;

    sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
    notify_project(&updated.planning.card_detail.card.project_id);
    let detail = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    let review_report = detail
        .review_report
        .clone()
        .ok_or_else(|| internal("Review report missing after resolveFinding"))?;
    let finding = review_report
        .findings
        .iter()
        .find(|f| f.id == input.finding_id.trim())
        .cloned()
        .ok_or_else(|| internal("Resolved finding missing from review report"))?;

    Ok(WorkflowBoardResolveFindingResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        finding,
        review_report,
        card: detail,
    })
}

pub(crate) async fn record_manual_acceptance_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardRecordManualAcceptanceInput,
    observed_by: &str,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardRecordManualAcceptanceResult, WorkflowBoardRequestError> {
    ensure_human_client_caller(observed_by)?;
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;
    let verdict_str = enum_as_str(&input.verdict).ok_or_else(|| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "Invalid manual acceptance verdict",
        )
    })?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let current = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    check_revision(
        card_id,
        current.planning.card_detail.card.revision,
        input.expected_revision,
    )?;
    let ws_root = resolve_card_workspace_root(&conn, &current.planning.card_detail.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let (invalidated, _) = reconcile_card_and_snapshot(&conn, card_id, &ws_root, &now_iso)?;
    if invalidated {
        notify_project(&current.planning.card_detail.card.project_id);
    }

    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let updated = record_manual_acceptance(
        &conn,
        &RecordManualAcceptanceInput {
            card_id,
            revision_id: &rev_id,
            expected_revision: expected_rev_i64,
            criterion_id: input.criterion_id.trim(),
            verdict: &verdict_str,
            note: input.note.as_deref(),
            evidence_ref: input.evidence_ref.as_deref(),
            evidence_digest: input.evidence_digest.as_deref(),
            expected_product_snapshot_digest: input.expected_product_snapshot_digest.as_deref(),
            observed_by,
        },
        &now_iso,
    )
    .map_err(map_repo_err)?;

    sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
    notify_project(&updated.planning.card_detail.card.project_id);
    let detail = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    let observation = detail
        .ac_observations
        .iter()
        .find(|o| o.criterion_id == input.criterion_id.trim())
        .cloned()
        .ok_or_else(|| internal("Recorded AC observation missing"))?;
    let ship_readiness = detail
        .ship_readiness
        .clone()
        .ok_or_else(|| internal("Ship readiness missing"))?;

    Ok(WorkflowBoardRecordManualAcceptanceResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        observation,
        ship_readiness,
        card: detail,
    })
}

pub(crate) async fn approve_delivery_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardApproveDeliveryInput,
    approved_by: &str,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardApproveDeliveryResult, WorkflowBoardRequestError> {
    ensure_human_client_caller(approved_by)?;
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let (ws_root, card_dir, commit_sha, diff_digest) = {
        let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
        let current = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
        check_revision(
            card_id,
            current.planning.card_detail.card.revision,
            input.expected_revision,
        )?;
        let ws_root = resolve_card_workspace_root(&conn, &current.planning.card_detail.card)?;
        let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
        let (invalidated, _snap) = reconcile_card_and_snapshot(&conn, card_id, &ws_root, &now_iso)?;
        if invalidated {
            notify_project(&current.planning.card_detail.card.project_id);
        }
        let commit_sha = resolve_head_commit_sha(&ws_root);
        drop(conn);
        let diff_digest = get_card_diff_impl(
            orchestrator,
            &legion_contracts::workflow_board::WorkflowBoardGetCardDiffInput {
                card_id: input.card_id.clone(),
                task_id: None,
                include_patches: Some(false),
            },
        )
        .ok()
        .and_then(|d| d.diff_digest);
        (ws_root, card_dir, commit_sha, diff_digest)
    };

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let app_id = format!("apr-{}", Uuid::new_v4().simple());
    let del_id = format!("del-{}", Uuid::new_v4().simple());
    let updated = approve_delivery(
        &conn,
        &ApproveDeliveryInput {
            card_id,
            revision_id: &rev_id,
            approval_id: &app_id,
            delivery_receipt_id: &del_id,
            expected_revision: expected_rev_i64,
            expected_product_snapshot_digest: input.expected_product_snapshot_digest.as_deref(),
            commit_sha: commit_sha.as_deref(),
            diff_digest: diff_digest.as_deref(),
            note: input.note.as_deref(),
            approved_by,
        },
        &now_iso,
    )
    .map_err(map_repo_err)?;
    sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;

    let delivery_approval = updated
        .planning
        .card_detail
        .approvals
        .iter()
        .find(|a| a.id == app_id)
        .map(to_approval_receipt)
        .transpose()?
        .ok_or_else(|| internal("Newly recorded delivery approval missing"))?;

    notify_project(&updated.planning.card_detail.card.project_id);
    let detail = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    let delivery_receipt = detail
        .delivery_receipt
        .clone()
        .ok_or_else(|| internal("Newly recorded delivery receipt missing"))?;

    Ok(WorkflowBoardApproveDeliveryResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        status: WorkflowBoardCardStatus::Completed,
        delivery_status: WorkflowBoardDeliveryStatus::Shipped,
        delivery_receipt,
        delivery_approval,
        card: detail,
    })
}
