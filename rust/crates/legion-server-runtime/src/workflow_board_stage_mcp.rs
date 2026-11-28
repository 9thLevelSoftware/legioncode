#![forbid(unsafe_code)]
//! Scoped MCP stage committers for Execute tasks (`complete_task_from_stage_impl`)
//! and independent Review reports (`record_review_from_stage_impl`).

use std::sync::Arc;

use legion_contracts::workflow_board::{WorkflowBoardRequestError, WorkflowCardDetail};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m3::{
    CompleteTaskInput, RecordReviewRunInput, ReviewFindingInput,
};
use legion_persistence::workflow_board_repo::{
    complete_task, get_card_execution_detail, link_stage_run_thread, record_review_run,
};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::types::SignalAddress;
use serde_json::json;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_checks::sync_card_json_file;
use crate::workflow_board_diff::{
    classify_contract_files, compute_turn_changed_files, load_task_baseline,
};
use crate::workflow_board_fs::{
    ensure_confined_card_dir, resolve_card_workspace_root, validate_safe_card_id,
};
use crate::workflow_board_hydration_m3::load_hydrated_card_detail;
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_snapshot::compute_product_snapshot_digest;
use crate::workflow_board_types::{db_lock_err, internal, map_repo_err};

/// Tuple describing a raw review finding emitted from scoped MCP.
pub(crate) type RawReviewFindingTuple = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
);

/// Parameters for [`complete_task_from_stage_impl`].
pub(crate) struct CompleteTaskFromStageParams<'a> {
    pub card_id_raw: &'a str,
    pub stage_run_id: &'a str,
    pub task_id: &'a str,
    pub outcome_status: &'a str,
    pub error_message: Option<&'a str>,
}

/// Parameters for [`record_review_from_stage_impl`].
pub(crate) struct RecordReviewFromStageParams<'a> {
    pub card_id_raw: &'a str,
    pub stage_run_id: &'a str,
    pub verdict: &'a str,
    pub summary: Option<&'a str>,
    pub findings: &'a [RawReviewFindingTuple],
}

/// Completes a bound Execute stage task via scoped MCP (with post-turn contract diff validation).
pub(crate) async fn complete_task_from_stage_impl(
    orchestrator: &Arc<Orchestrator>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    params: CompleteTaskFromStageParams<'_>,
    notify_project: impl Fn(&str),
) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
    let CompleteTaskFromStageParams {
        card_id_raw,
        stage_run_id,
        ..
    } = params;
    let card_id = validate_safe_card_id(card_id_raw)?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let (detail, wf_run_id, project_id) =
        persist_stage_task_completion(orchestrator, card_id, &params, &now_iso)?;
    notify_project(&project_id);
    signal_stage_wait(
        workflow_manager,
        wf_run_id,
        "await_execute_completion",
        json!({ "status": params.outcome_status }),
        &format!("exec-{stage_run_id}"),
    )
    .await;
    Ok(detail)
}

async fn signal_stage_wait(
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    wf_run_id: Option<String>,
    wait_key: &str,
    payload: serde_json::Value,
    signal_key: &str,
) {
    if let Some(rid) = wf_run_id
        && let Some(mgr) = workflow_manager
    {
        let _ = mgr
            .engine()
            .signal(
                SignalAddress::Run {
                    run_id: rid,
                    wait: wait_key.to_owned(),
                },
                payload,
                signal_key,
                None,
            )
            .await;
    }
}

/// Records an independent review run via scoped MCP (strictly enforcing `disposition = "unresolved"`).
pub(crate) async fn record_review_from_stage_impl(
    orchestrator: &Arc<Orchestrator>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    params: RecordReviewFromStageParams<'_>,
    notify_project: impl Fn(&str),
) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
    let RecordReviewFromStageParams {
        card_id_raw,
        stage_run_id,
        verdict,
        summary,
        findings,
    } = params;
    let card_id = validate_safe_card_id(card_id_raw)?;
    let _guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let (detail, wf_run_id, project_id) = {
        let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
        let exec = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
        let card = &exec.planning.card_detail.card;
        let sr = exec
            .planning
            .card_detail
            .stage_runs
            .iter()
            .find(|r| r.id == stage_run_id)
            .cloned()
            .ok_or_else(|| internal("Stage run missing"))?;
        let ws_root = resolve_card_workspace_root(&conn, card)?;
        let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
        let snap = compute_product_snapshot_digest(&ws_root);
        let review_id = format!("rvw-{}", Uuid::new_v4().simple());
        let rev_id = format!("rev-{}", Uuid::new_v4().simple());
        let finding_refs: Vec<ReviewFindingInput<'_>> = findings
            .iter()
            .map(|(id, sev, cid, file, line, msg)| ReviewFindingInput {
                id: id.as_str(),
                severity: sev.as_str(),
                criterion_id: cid.as_deref(),
                file_path: file.as_deref(),
                line: *line,
                message: msg.as_str(),
            })
            .collect();
        let updated = record_review_run(
            &conn,
            &RecordReviewRunInput {
                card_id,
                revision_id: &rev_id,
                review_id: &review_id,
                expected_revision: card.revision,
                stage_run_id: Some(stage_run_id),
                thread_id: sr.thread_id.as_deref(),
                snapshot_digest: Some(&snap),
                verdict,
                summary,
                findings: &finding_refs,
            },
            &now_iso,
        )
        .map_err(map_repo_err)?;
        let sr_status = if verdict == "failed" {
            "failed"
        } else {
            "completed"
        };
        let _ = link_stage_run_thread(
            &conn,
            &legion_persistence::workflow_board_repo::types::LinkStageRunThreadInput {
                id: &sr.id,
                status: sr_status,
                thread_id: sr.thread_id.as_deref(),
                workflow_run_id: sr.workflow_run_id.as_deref(),
                worktree_path: sr.worktree_path.as_deref(),
                branch: sr.branch.as_deref(),
                last_error: None,
                ended_at: Some(&now_iso),
            },
            &now_iso,
        );
        sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
        let hydrated = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
        (
            hydrated,
            sr.workflow_run_id,
            updated.planning.card_detail.card.project_id,
        )
    };
    notify_project(&project_id);
    signal_stage_wait(
        workflow_manager,
        wf_run_id,
        "await_review_completion",
        json!({ "verdict": verdict }),
        &format!("rev-{stage_run_id}"),
    )
    .await;
    Ok(detail)
}

fn persist_stage_task_completion(
    orchestrator: &Orchestrator,
    card_id: &str,
    params: &CompleteTaskFromStageParams<'_>,
    now_iso: &str,
) -> Result<(WorkflowCardDetail, Option<String>, String), WorkflowBoardRequestError> {
    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let exec = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    let card = &exec.planning.card_detail.card;
    let sr = exec
        .planning
        .card_detail
        .stage_runs
        .iter()
        .find(|r| r.id == params.stage_run_id)
        .cloned()
        .ok_or_else(|| internal("Stage run missing"))?;
    let ws_root = resolve_card_workspace_root(&conn, card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let baseline = load_task_baseline(&card_dir, params.stage_run_id);
    let changed = compute_turn_changed_files(&ws_root, baseline.as_ref());
    let task_row = exec
        .planning
        .tasks
        .iter()
        .find(|t| t.task_id == params.task_id);
    let allowed: Vec<String> = task_row
        .and_then(|t| serde_json::from_str(&t.files_allowed_json).ok())
        .unwrap_or_default();
    let forbidden: Vec<String> = task_row
        .and_then(|t| serde_json::from_str(&t.files_forbidden_json).ok())
        .unwrap_or_default();
    let classified = classify_contract_files(&ws_root, &changed, &allowed, &forbidden);
    let snap = compute_product_snapshot_digest(&ws_root);

    let ch_refs: Vec<&str> = classified
        .changed_files
        .iter()
        .map(String::as_str)
        .collect();
    let un_refs: Vec<&str> = classified
        .unexpected_files
        .iter()
        .map(String::as_str)
        .collect();
    let fb_refs: Vec<&str> = classified
        .forbidden_files
        .iter()
        .map(String::as_str)
        .collect();
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let viol_id = format!("viol-{}", Uuid::new_v4().simple());

    let updated = complete_task(
        &conn,
        &CompleteTaskInput {
            card_id,
            revision_id: &rev_id,
            expected_revision: card.revision,
            task_id: params.task_id,
            outcome_status: params.outcome_status,
            stage_run_id: Some(params.stage_run_id),
            thread_id: sr.thread_id.as_deref(),
            workflow_run_id: sr.workflow_run_id.as_deref(),
            diff_digest: Some(&classified.diff_digest),
            product_snapshot_digest: Some(&snap),
            error_message: params.error_message,
            violation_id: Some(&viol_id),
            changed_files: &ch_refs,
            unexpected_files: &un_refs,
            forbidden_files: &fb_refs,
        },
        now_iso,
    )
    .map_err(map_repo_err)?;
    let sr_status =
        if un_refs.is_empty() && fb_refs.is_empty() && params.outcome_status == "completed" {
            "completed"
        } else {
            "failed"
        };
    let _ = link_stage_run_thread(
        &conn,
        &legion_persistence::workflow_board_repo::types::LinkStageRunThreadInput {
            id: &sr.id,
            status: sr_status,
            thread_id: sr.thread_id.as_deref(),
            workflow_run_id: sr.workflow_run_id.as_deref(),
            worktree_path: sr.worktree_path.as_deref(),
            branch: sr.branch.as_deref(),
            last_error: params.error_message,
            ended_at: Some(now_iso),
        },
        now_iso,
    );
    sync_card_json_file(&card_dir, &updated.planning.card_detail.card)?;
    let hydrated = load_hydrated_card_detail(&conn, &ws_root, card_id)?;
    Ok((
        hydrated,
        sr.workflow_run_id,
        updated.planning.card_detail.card.project_id,
    ))
}
