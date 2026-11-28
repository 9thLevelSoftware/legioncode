#![forbid(unsafe_code)]
//! Spec approval (`approveSpec`) and stage return (`returnForRevision`) mutations.

use std::sync::Arc;

use legion_contracts::workflow_board::{
    WorkflowBoardApproveSpecInput, WorkflowBoardApproveSpecResult, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardReturnForRevisionInput,
    WorkflowBoardReturnForRevisionResult,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types::{ApproveSpecInput, CardDetail};
use legion_persistence::workflow_board_repo::types_m2::ReturnForRevisionInput;
use legion_persistence::workflow_board_repo::{
    approve_spec, get_card_planning_detail, return_for_revision,
    validate_spec_approval_m2_readiness,
};
use legion_workflows::sha256_prefixed;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, resolve_card_workspace_root,
    validate_safe_card_id,
};
use crate::workflow_board_hydration::to_approval_receipt;
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_journal::{JournaledWriteSpec, execute_journaled_writes};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::{ensure_disk_spec_matches, reconcile_card_external_edits};
use crate::workflow_board_render::{parse_stored_spec_fields, render_card_json};
use crate::workflow_board_snapshot::clear_deliverable_stale_if_clean_and_reload;
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, map_repo_err, parse_expected_rev,
};

pub(crate) async fn approve_spec_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardApproveSpecInput,
    approved_by: &str,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardApproveSpecResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _card_guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    let current = &planning.card_detail;
    if invalidated {
        notify_project(&current.card.project_id);
    }
    check_revision(card_id, current.card.revision, input.expected_revision)?;

    let spec_digest = require_card_spec_digest(current)?;
    if let Some(ref exp) = input.expected_digest
        && exp.trim() != spec_digest
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::DigestMismatch,
            format!(
                "spec digest mismatch: expected `{}`, actual `{spec_digest}`",
                exp.trim()
            ),
        ));
    }
    ensure_disk_spec_matches(&conn, card_id, &spec_digest, &now_iso)?;

    if planning.decisions.iter().any(|d| d.status == "unresolved") {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ExplorationUnresolved,
            "Spec approval blocked while exploration decisions remain unresolved",
        ));
    }
    if let Some(ch) = planning.challenges.first()
        && matches!(ch.status.as_str(), "pending" | "in_progress" | "stale")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ChallengeUnresolved,
            format!(
                "Spec approval blocked while challenge status is `{}`",
                ch.status
            ),
        ));
    }
    validate_spec_approval_m2_readiness(&conn, card_id, &current.card.spec_json)
        .map_err(map_repo_err)?;

    let ws_root = resolve_card_workspace_root(&conn, &current.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let writes = prepare_approve_card_json(current, &now_iso)?;

    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let app_id = format!("apr-{}", Uuid::new_v4().simple());
    let updated = execute_journaled_writes(&conn, card_id, &card_dir, &writes, &now_iso, |tx| {
        approve_spec(
            tx,
            &ApproveSpecInput {
                card_id,
                revision_id: &rev_id,
                approval_id: &app_id,
                expected_revision: expected_rev_i64,
                expected_spec_digest: Some(&spec_digest),
                wireframes_digest: current.card.wireframes_digest.as_deref(),
                approved_by,
            },
            &now_iso,
        )
        .map_err(map_repo_err)?;
        get_card_planning_detail(tx, card_id).map_err(map_repo_err)
    })?;
    let updated = clear_deliverable_stale_if_clean_and_reload(&conn, card_id, updated, &now_iso)?;

    let receipt = updated
        .card_detail
        .approvals
        .iter()
        .find(|a| a.id == app_id)
        .map(to_approval_receipt)
        .transpose()?
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                "Newly recorded spec approval receipt missing",
            )
        })?;

    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    Ok(WorkflowBoardApproveSpecResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        spec_approval: receipt,
        card: detail,
    })
}

pub(crate) async fn return_for_revision_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardReturnForRevisionInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardReturnForRevisionResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _card_guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;
    let target_str = enum_as_str(&input.target_stage).unwrap_or_else(|| "spec".to_owned());

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    let current = &planning.card_detail;
    if invalidated {
        notify_project(&current.card.project_id);
    }
    check_revision(card_id, current.card.revision, input.expected_revision)?;

    let ws_root = resolve_card_workspace_root(&conn, &current.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let fields = parse_stored_spec_fields(&current.card.spec_json);
    let mut next_card = current.card.clone();
    next_card.revision += 1;
    next_card.stage.clone_from(&target_str);
    next_card.next_action = Some(format!("revise_{target_str}"));
    now_iso.clone_into(&mut next_card.updated_at);
    let card_json = render_card_json(&next_card, &fields)?;
    let writes = vec![JournaledWriteSpec {
        key: "card.json".to_owned(),
        bytes: card_json.as_bytes().to_vec(),
        expected_digest: sha256_prefixed(card_json.as_bytes()),
        revision: Some(next_card.revision),
    }];

    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let updated = execute_journaled_writes(&conn, card_id, &card_dir, &writes, &now_iso, |tx| {
        return_for_revision(
            tx,
            &ReturnForRevisionInput {
                card_id,
                revision_id: &rev_id,
                expected_revision: expected_rev_i64,
                target_stage: &target_str,
                reason: input.reason.trim(),
            },
            &now_iso,
        )
        .map_err(map_repo_err)
    })?;

    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    Ok(WorkflowBoardReturnForRevisionResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        stage: detail.summary.stage,
        spec_approval: detail.spec_approval.clone(),
        plan_approval: detail.plan_approval.clone(),
        history: detail.history.clone(),
        card: detail,
    })
}

fn require_card_spec_digest(current: &CardDetail) -> Result<String, WorkflowBoardRequestError> {
    current
        .card
        .spec_digest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::SpecNotApproved,
                "Cannot approve spec without a non-empty spec digest",
            )
        })
}

fn prepare_approve_card_json(
    current: &CardDetail,
    now_iso: &str,
) -> Result<Vec<JournaledWriteSpec>, WorkflowBoardRequestError> {
    let fields = parse_stored_spec_fields(&current.card.spec_json);
    let mut next_card = current.card.clone();
    next_card.revision += 1;
    if next_card.stage == "spec" {
        "plan".clone_into(&mut next_card.stage);
    }
    next_card.next_action = Some("start_plan".to_owned());
    now_iso.clone_into(&mut next_card.updated_at);
    let card_json = render_card_json(&next_card, &fields)?;
    Ok(vec![JournaledWriteSpec {
        key: "card.json".to_owned(),
        bytes: card_json.as_bytes().to_vec(),
        expected_digest: sha256_prefixed(card_json.as_bytes()),
        revision: Some(next_card.revision),
    }])
}
