#![forbid(unsafe_code)]
//! M3 execution lifecycle controls (`pauseCard`, `resumeCard`, `retryStep`).

use std::sync::Arc;

use legion_contracts::workflow_board::{
    WorkflowBoardPauseCardInput, WorkflowBoardPauseCardResult, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardResumeCardInput, WorkflowBoardResumeCardResult,
    WorkflowBoardRetryStepInput, WorkflowBoardRetryStepKind, WorkflowBoardRetryStepResult,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m3::{
    PauseCardInput, ReconcileInterruptedActionInput, ResumeCardInput, RetryStepInput,
};
use legion_persistence::workflow_board_repo::{
    get_card_detail, get_card_execution_detail, pause_card, reconcile_interrupted_action,
    resume_card, retry_step, update_card_external_edit_state,
};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::sha256_prefixed;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_checks::{run_planned_check_impl, sync_card_json_file};
use crate::workflow_board_fs::{
    ensure_confined_card_dir, resolve_card_workspace_root, validate_safe_card_id,
};
use crate::workflow_board_hydration_m3::load_hydrated_card_detail;
use crate::workflow_board_journal::{JournaledWriteSpec, execute_journaled_writes};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::reconcile_incomplete_write_journal;
use crate::workflow_board_render::{parse_stored_spec_fields, render_card_json};
use crate::workflow_board_snapshot::{
    clear_deliverable_stale_if_clean, compute_product_snapshot_digest, reconcile_card_and_snapshot,
};
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, map_repo_err, parse_expected_rev,
};

pub(crate) async fn pause_card_impl(
    orchestrator: &Arc<Orchestrator>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    input: &WorkflowBoardPauseCardInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardPauseCardResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let (detail, active_wf_runs, project_id) = {
        let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
        let current = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
        let cur_card = &current.planning.card_detail.card;
        check_revision(card_id, cur_card.revision, input.expected_revision)?;

        let ws_root = resolve_card_workspace_root(&conn, cur_card)?;
        let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
        let fields = parse_stored_spec_fields(&cur_card.spec_json);
        let mut next_card = cur_card.clone();
        next_card.revision += 1;
        "paused".clone_into(&mut next_card.status);
        next_card.next_action = Some("resume_card".to_owned());
        now_iso.clone_into(&mut next_card.updated_at);
        let card_json = render_card_json(&next_card, &fields)?;
        let writes = vec![JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(card_json.as_bytes()),
            revision: Some(next_card.revision),
        }];
        let rev_id = format!("rev-{}", Uuid::new_v4().simple());
        let updated =
            execute_journaled_writes(&conn, card_id, &card_dir, &writes, &now_iso, |tx| {
                pause_card(
                    tx,
                    &PauseCardInput {
                        card_id,
                        revision_id: &rev_id,
                        expected_revision: expected_rev_i64,
                        reason: input.reason.as_deref(),
                    },
                    &now_iso,
                )
                .map_err(map_repo_err)
            })?;
        let wf_runs: Vec<String> = updated
            .planning
            .card_detail
            .stage_runs
            .iter()
            .filter(|sr| sr.status == "paused")
            .filter_map(|sr| sr.workflow_run_id.clone())
            .collect();
        let hydrated = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
        let pid = updated.planning.card_detail.card.project_id;
        (hydrated, wf_runs, pid)
    };

    if let Some(mgr) = workflow_manager {
        let engine = mgr.engine();
        for rid in active_wf_runs {
            let _ = engine.cancel(&rid, input.reason.clone()).await;
        }
    }
    notify_project(&project_id);
    Ok(WorkflowBoardPauseCardResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        status: detail.summary.status,
        card: detail,
    })
}

pub(crate) async fn resume_card_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardResumeCardInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardResumeCardResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let current = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    let cur_card = &current.planning.card_detail.card;
    check_revision(card_id, cur_card.revision, input.expected_revision)?;
    if cur_card.status != "paused" {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!(
                "cannot resume card with status `{}` (must be `paused`)",
                cur_card.status
            ),
        ));
    }
    let ws_root = resolve_card_workspace_root(&conn, cur_card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;

    let mut reconciled_count = 0_u64;
    if input.reconcile_interrupted.unwrap_or(false) {
        let j_rep = reconcile_incomplete_write_journal(&conn, &now_iso)?;
        reconciled_count += u64::try_from(j_rep.committed + j_rep.rolled_back).unwrap_or(0);
        let snap = compute_product_snapshot_digest(&ws_root);
        for act in current
            .external_actions
            .iter()
            .filter(|a| a.status == "interrupted")
        {
            reconcile_interrupted_action(
                &conn,
                &ReconcileInterruptedActionInput {
                    card_id,
                    action_id: &act.id,
                    status: "reconciled",
                    observed_snapshot_digest: Some(&snap),
                    note: input.note.as_deref(),
                    reconciled_by: "client",
                },
                &now_iso,
            )
            .map_err(map_repo_err)?;
            reconciled_count += 1;
        }
        let _ = reconcile_card_and_snapshot(&conn, card_id, &ws_root, &now_iso)?;
        let after_reconcile = get_card_detail(&conn, card_id).map_err(map_repo_err)?;
        if after_reconcile.card.external_edit_state.as_deref() == Some("deliverable_stale") {
            let _ = clear_deliverable_stale_if_clean(&conn, card_id, &now_iso)?;
        } else if after_reconcile.card.external_edit_state.as_deref() != Some("clean") {
            update_card_external_edit_state(&conn, card_id, "clean", None, &now_iso)
                .map_err(map_repo_err)?;
        }
    }

    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let updated = resume_card(
        &conn,
        &ResumeCardInput {
            card_id,
            revision_id: &rev_id,
            expected_revision: expected_rev_i64,
            note: input.note.as_deref(),
        },
        &now_iso,
    )
    .map_err(map_repo_err)?;
    sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
    notify_project(&updated.planning.card_detail.card.project_id);
    let detail = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    Ok(WorkflowBoardResumeCardResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        status: detail.summary.status,
        reconciled_count,
        card: detail,
    })
}

pub(crate) async fn retry_step_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardRetryStepInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardRetryStepResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let step_str = enum_as_str(&input.step_kind).unwrap_or_else(|| "task".to_owned());
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let (ws_root, project_id) = {
        let _guard = acquire_card_lock(orchestrator, card_id).await;
        let now_iso = utc_now_iso();
        let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
        let current = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
        check_revision(
            card_id,
            current.planning.card_detail.card.revision,
            input.expected_revision,
        )?;
        let ws_root = resolve_card_workspace_root(&conn, &current.planning.card_detail.card)?;
        let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
        let rev_id = format!("rev-{}", Uuid::new_v4().simple());
        let updated = retry_step(
            &conn,
            &RetryStepInput {
                card_id,
                revision_id: &rev_id,
                expected_revision: expected_rev_i64,
                step_kind: &step_str,
                target_id: input.target_id.as_deref(),
                reason: input.reason.as_deref(),
            },
            &now_iso,
        )
        .map_err(map_repo_err)?;
        sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
        (ws_root, updated.planning.card_detail.card.project_id)
    };
    notify_project(&project_id);

    if input.step_kind == WorkflowBoardRetryStepKind::Check
        && let Some(ref check_id) = input.target_id
    {
        run_planned_check_impl(orchestrator, card_id, check_id, None, &notify_project).await?;
    }

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let detail = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    Ok(WorkflowBoardRetryStepResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        step_kind: input.step_kind,
        target_id: input.target_id.clone(),
        card: detail,
    })
}
