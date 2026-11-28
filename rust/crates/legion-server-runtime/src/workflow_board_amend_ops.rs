#![forbid(unsafe_code)]
//! `workflowBoard.proposeAmendment` service logic and bounded plan-item update helpers.

use std::path::Path;
use std::sync::Arc;

use legion_contracts::ids::WorkflowCardId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardAmendmentTargetStage, WorkflowBoardProposeAmendmentInput,
    WorkflowBoardProposeAmendmentResult, WorkflowBoardRequestError, WorkflowBoardUpdatePlanInput,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m3::ProposeAmendmentInput;
use legion_persistence::workflow_board_repo::{
    get_card_execution_detail, get_card_planning_detail, propose_amendment,
};
use serde_json::json;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_checks::sync_card_json_file;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, resolve_card_workspace_root,
    validate_safe_card_id,
};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_hydration_m3::load_hydrated_card_detail;
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_plan::{build_merged_plan_doc, build_plan_writes};
use crate::workflow_board_plan_inputs::{cleanup_obsolete_task_files, persist_plan_with_journal};
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_snapshot::clear_deliverable_stale_if_clean_and_reload;
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, internal, map_repo_err, parse_expected_rev,
};

pub(crate) async fn propose_amendment_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardProposeAmendmentInput,
    proposed_by: &str,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardProposeAmendmentResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;
    let target_stage = input
        .target_stage
        .unwrap_or(WorkflowBoardAmendmentTargetStage::Plan);
    let target_str = enum_as_str(&target_stage).unwrap_or_else(|| "plan".to_owned());

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

    let proposed_changes_json = serde_json::to_string(&json!({
        "tasks": input.tasks,
        "plannedChecks": input.planned_checks,
        "acMappings": input.ac_mappings,
        "deliverables": input.deliverables,
    }))
    .map_err(|e| internal(e.to_string()))?;
    let unexp_refs: Vec<&str> = input
        .unexpected_files
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let amend_id = format!("amd-{}", Uuid::new_v4().simple());

    let mut updated = propose_amendment(
        &conn,
        &ProposeAmendmentInput {
            card_id,
            revision_id: &rev_id,
            amendment_id: &amend_id,
            expected_revision: expected_rev_i64,
            target_stage: &target_str,
            reason: input.reason.trim(),
            summary: input.summary.as_deref(),
            source_task_id: input.source_task_id.as_deref(),
            unexpected_files: &unexp_refs,
            proposed_changes_json: &proposed_changes_json,
            proposed_by,
        },
        &now_iso,
    )
    .map_err(map_repo_err)?;

    if target_stage == WorkflowBoardAmendmentTargetStage::Plan
        && (input.tasks.is_some() || input.planned_checks.is_some() || input.ac_mappings.is_some())
    {
        apply_amended_plan_items(
            &conn,
            &ws_root,
            &card_dir,
            card_id,
            updated.planning.card_detail.card.revision,
            input,
            &now_iso,
        )?;
        updated = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    }

    sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
    notify_project(&updated.planning.card_detail.card.project_id);
    let detail = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    let amendment = detail
        .amendments
        .iter()
        .find(|a| a.id == amend_id)
        .cloned()
        .ok_or_else(|| internal("Newly recorded amendment missing"))?;
    Ok(WorkflowBoardProposeAmendmentResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        stage: detail.summary.stage,
        amendment,
        spec_approval: detail.spec_approval.clone(),
        plan_approval: detail.plan_approval.clone(),
        card: detail,
    })
}

fn apply_amended_plan_items(
    conn: &rusqlite::Connection,
    ws_root: &Path,
    card_dir: &Path,
    card_id: &str,
    expected_revision: i64,
    input: &WorkflowBoardProposeAmendmentInput,
    now_iso: &str,
) -> Result<(), WorkflowBoardRequestError> {
    let expected_rev_u64 = u64::try_from(expected_revision.max(1)).unwrap_or(1);
    let plan_input = WorkflowBoardUpdatePlanInput {
        card_id: WorkflowCardId::new(card_id).map_err(|e| internal(e.to_string()))?,
        expected_revision: expected_rev_u64,
        lenses: None,
        alternatives: None,
        selected_alternative_id: PatchValue::Absent,
        selection_rationale: None,
        revise_alternatives_feedback: None,
        strategy: None,
        strategy_rationale: None,
        strategy_metadata: None,
        granularity: None,
        tasks: input.tasks.clone(),
        checks: input.planned_checks.clone(),
        ac_mappings: input.ac_mappings.clone(),
    };
    let planning = get_card_planning_detail(conn, card_id).map_err(map_repo_err)?;
    let existing_keys = enumerate_artifact_keys(ws_root, card_id);
    let existing_detail = to_card_detail_from_planning(&planning, existing_keys)?;
    let merged_doc = build_merged_plan_doc(existing_detail.plan.as_ref(), &plan_input, now_iso);
    let (plan_json, plan_digest, design_digest, writes) =
        build_plan_writes(card_id, &planning.card_detail.card, &merged_doc, now_iso)?;
    let updated = persist_plan_with_journal(
        conn,
        card_id,
        card_dir,
        &writes,
        expected_revision,
        &plan_input,
        &plan_json,
        &plan_digest,
        &design_digest,
        now_iso,
    )?;
    let updated = clear_deliverable_stale_if_clean_and_reload(conn, card_id, updated, now_iso)?;
    cleanup_obsolete_task_files(card_dir, &updated.tasks);
    Ok(())
}
