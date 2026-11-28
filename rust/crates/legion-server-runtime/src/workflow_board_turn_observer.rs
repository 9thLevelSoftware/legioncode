#![forbid(unsafe_code)]
//! Background & catch-up observer for settled `Execute` and `Review` agent turns.

use std::sync::Arc;

use legion_contracts::orchestration::DomainEventPayload;
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m3::{
    CompleteTaskInput, RecordReviewRunInput, ReviewFindingInput,
};
use legion_persistence::workflow_board_repo::{
    complete_task, get_card_execution_detail, get_latest_stage_run_for_thread,
    link_stage_run_thread, record_review_run,
};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::traits::AgentEnding;
use legion_workflows::types::SignalAddress;
use serde_json::json;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board::ProjectWorkflowBoardService;
use crate::workflow_board_checks::sync_card_json_file;
use crate::workflow_board_diff::{
    classify_contract_files, compute_turn_changed_files, load_task_baseline,
};
use crate::workflow_board_fs::{ensure_confined_card_dir, resolve_card_workspace_root};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_review_parse::parse_review_settlement;
use crate::workflow_board_snapshot::compute_product_snapshot_digest;
use crate::workflow_board_stage_m3_preflight::get_stage_run_by_id;
use crate::workflow_driver_pump::read_turn_settlement;

/// Spawns the event-driven observer watching orchestrator turn settlements for board stage runs.
pub(crate) fn spawn_board_turn_observer(
    board: ProjectWorkflowBoardService,
) -> tokio::task::JoinHandle<()> {
    let mut sub = board.orchestrator_ref().subscribe();
    tokio::spawn(async move {
        process_settled_board_turns(&board).await;
        while let Some(persisted) = sub.recv().await {
            if let DomainEventPayload::RunUpdated { .. } = persisted.event.payload {
                maybe_settle_thread_turn(&board, persisted.event.thread_id.as_str()).await;
            }
        }
    })
}

/// Scans all `'running'` Execute/Review stage runs and settles any whose thread turn has completed.
pub(crate) async fn process_settled_board_turns(board: &ProjectWorkflowBoardService) {
    let candidates: Vec<(String, String)> = {
        let Ok(conn) = board.orchestrator_ref().sink().db().lock() else {
            return;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT thread_id, stage
             FROM workflow_board_stage_runs
             WHERE status = 'running'
               AND stage IN ('execute', 'review')
               AND thread_id IS NOT NULL",
        ) else {
            return;
        };
        let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        else {
            return;
        };
        rows.filter_map(Result::ok).collect()
    };
    for (thread_id, _) in candidates {
        maybe_settle_thread_turn(board, &thread_id).await;
    }
}

async fn maybe_settle_thread_turn(board: &ProjectWorkflowBoardService, thread_id: &str) {
    settle_thread_if_ready(
        board.orchestrator_ref(),
        board.workflow_manager_ref(),
        thread_id,
        &|pid| board.notify_project_change(pid),
    )
    .await;
}

/// Settles an active Execute/Review stage run bound to `thread_id` if its turn has already ended.
pub(crate) async fn settle_thread_if_ready(
    orchestrator: &Arc<Orchestrator>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    thread_id: &str,
    notify_project: &impl Fn(&str),
) {
    let stage_run = {
        let Ok(conn) = orchestrator.sink().db().lock() else {
            return;
        };
        get_latest_stage_run_for_thread(&conn, thread_id)
            .ok()
            .flatten()
    };
    let Some(sr) = stage_run else {
        return;
    };
    if sr.status != "running" || !matches!(sr.stage.as_str(), "execute" | "review") {
        return;
    }
    let Ok(Some(settlement)) = read_turn_settlement(orchestrator, thread_id, "") else {
        return;
    };

    let _guard = acquire_card_lock(orchestrator, &sr.card_id).await;
    if sr.stage == "execute" {
        settle_execute_turn(
            orchestrator,
            workflow_manager,
            &sr.id,
            &settlement,
            notify_project,
        )
        .await;
    } else {
        settle_review_turn(
            orchestrator,
            workflow_manager,
            &sr.id,
            &settlement,
            notify_project,
        )
        .await;
    }
}

async fn settle_execute_turn(
    orchestrator: &Arc<Orchestrator>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    stage_run_id: &str,
    settlement: &legion_workflows::traits::AgentResult,
    notify_project: &impl Fn(&str),
) {
    let outcome_status = match settlement.ending {
        AgentEnding::Completed => "completed",
        AgentEnding::Failed | AgentEnding::Stopped => "failed",
    };
    let err_msg = (outcome_status == "failed").then_some(settlement.detail.as_str());
    let signal_info =
        persist_execute_settlement(orchestrator, stage_run_id, outcome_status, err_msg);
    if let Some((pid, wf_run_id)) = signal_info {
        notify_project(&pid);
        if let Some(rid) = wf_run_id
            && let Some(mgr) = workflow_manager
        {
            let _ = mgr
                .engine()
                .signal(
                    SignalAddress::Run {
                        run_id: rid.clone(),
                        wait: "await_execute_completion".to_owned(),
                    },
                    json!({ "status": outcome_status }),
                    &format!("exec-{stage_run_id}"),
                    None,
                )
                .await;
        }
    }
}

fn persist_execute_settlement(
    orchestrator: &Arc<Orchestrator>,
    stage_run_id: &str,
    outcome_status: &str,
    err_msg: Option<&str>,
) -> Option<(String, Option<String>)> {
    let conn = orchestrator.sink().db().lock().ok()?;
    let sr = get_stage_run_by_id(&conn, stage_run_id).ok().flatten()?;
    if sr.status != "running" {
        return None;
    }
    let task_id = sr.task_id.as_deref()?;
    let exec = get_card_execution_detail(&conn, &sr.card_id).ok()?;
    let card = &exec.planning.card_detail.card;
    let ws_root = resolve_card_workspace_root(&conn, card).ok()?;
    let card_dir = ensure_confined_card_dir(&ws_root, &sr.card_id).ok()?;
    let baseline = load_task_baseline(&card_dir, &sr.id);
    let changed = compute_turn_changed_files(&ws_root, baseline.as_ref());
    let task_row = exec.planning.tasks.iter().find(|t| t.task_id == task_id);
    let allowed: Vec<String> = task_row
        .and_then(|t| serde_json::from_str(&t.files_allowed_json).ok())
        .unwrap_or_default();
    let forbidden: Vec<String> = task_row
        .and_then(|t| serde_json::from_str(&t.files_forbidden_json).ok())
        .unwrap_or_default();
    let classified = classify_contract_files(&ws_root, &changed, &allowed, &forbidden);
    let snap = compute_product_snapshot_digest(&ws_root);
    let now = utc_now_iso();

    let changed_refs: Vec<&str> = classified
        .changed_files
        .iter()
        .map(String::as_str)
        .collect();
    let unexp_refs: Vec<&str> = classified
        .unexpected_files
        .iter()
        .map(String::as_str)
        .collect();
    let forb_refs: Vec<&str> = classified
        .forbidden_files
        .iter()
        .map(String::as_str)
        .collect();
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let viol_id = format!("viol-{}", Uuid::new_v4().simple());

    let updated = complete_task(
        &conn,
        &CompleteTaskInput {
            card_id: &sr.card_id,
            revision_id: &rev_id,
            expected_revision: card.revision,
            task_id,
            outcome_status,
            stage_run_id: Some(&sr.id),
            thread_id: sr.thread_id.as_deref(),
            workflow_run_id: sr.workflow_run_id.as_deref(),
            diff_digest: Some(&classified.diff_digest),
            product_snapshot_digest: Some(&snap),
            error_message: err_msg,
            violation_id: Some(&viol_id),
            changed_files: &changed_refs,
            unexpected_files: &unexp_refs,
            forbidden_files: &forb_refs,
        },
        &now,
    )
    .ok()?;
    let sr_status =
        if unexp_refs.is_empty() && forb_refs.is_empty() && outcome_status == "completed" {
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
            last_error: err_msg,
            ended_at: Some(&now),
        },
        &now,
    );
    let _ = sync_card_json_file(&card_dir, &updated.planning.card_detail.card);
    Some((
        updated.planning.card_detail.card.project_id,
        sr.workflow_run_id,
    ))
}

async fn settle_review_turn(
    orchestrator: &Arc<Orchestrator>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    stage_run_id: &str,
    settlement: &legion_workflows::traits::AgentResult,
    notify_project: &impl Fn(&str),
) {
    let signal_info = persist_review_settlement(orchestrator, stage_run_id, settlement);
    if let Some((pid, wf_run_id, verdict)) = signal_info {
        notify_project(&pid);
        if let Some(rid) = wf_run_id
            && let Some(mgr) = workflow_manager
        {
            let _ = mgr
                .engine()
                .signal(
                    SignalAddress::Run {
                        run_id: rid.clone(),
                        wait: "await_review_completion".to_owned(),
                    },
                    json!({ "verdict": verdict }),
                    &format!("rev-{stage_run_id}"),
                    None,
                )
                .await;
        }
    }
}

fn persist_review_settlement(
    orchestrator: &Arc<Orchestrator>,
    stage_run_id: &str,
    settlement: &legion_workflows::traits::AgentResult,
) -> Option<(String, Option<String>, String)> {
    let conn = orchestrator.sink().db().lock().ok()?;
    let sr = get_stage_run_by_id(&conn, stage_run_id).ok().flatten()?;
    if sr.status != "running" {
        return None;
    }
    let exec = get_card_execution_detail(&conn, &sr.card_id).ok()?;
    let card = &exec.planning.card_detail.card;
    let ws_root = resolve_card_workspace_root(&conn, card).ok()?;
    let card_dir = ensure_confined_card_dir(&ws_root, &sr.card_id).ok()?;
    let now = utc_now_iso();
    let already_recorded = exec
        .review_runs
        .iter()
        .any(|r| r.stage_run_id.as_deref() == Some(&sr.id) && r.status != "historical");

    let mut verdict_str = "failed".to_owned();
    let mut failure_reason: Option<String> = None;
    if !already_recorded {
        let parsed = parse_review_settlement(settlement);
        verdict_str.clone_from(&parsed.verdict);
        if parsed.verdict == "failed" {
            failure_reason.clone_from(&parsed.summary);
        }
        let snap = compute_product_snapshot_digest(&ws_root);
        let review_id = format!("rvw-{}", Uuid::new_v4().simple());
        let rev_id = format!("rev-{}", Uuid::new_v4().simple());
        let finding_refs: Vec<ReviewFindingInput<'_>> = parsed
            .findings
            .iter()
            .map(|f| ReviewFindingInput {
                id: f.id.as_str(),
                severity: f.severity.as_str(),
                criterion_id: f.criterion_id.as_deref(),
                file_path: f.file.as_deref(),
                line: f.line,
                message: f.message.as_str(),
            })
            .collect();
        match record_review_run(
            &conn,
            &RecordReviewRunInput {
                card_id: &sr.card_id,
                revision_id: &rev_id,
                review_id: &review_id,
                expected_revision: card.revision,
                stage_run_id: Some(&sr.id),
                thread_id: sr.thread_id.as_deref(),
                snapshot_digest: Some(&snap),
                verdict: &parsed.verdict,
                summary: parsed.summary.as_deref(),
                findings: &finding_refs,
            },
            &now,
        ) {
            Ok(updated) => {
                let _ = sync_card_json_file(&card_dir, &updated.planning.card_detail.card);
            }
            Err(err) => {
                "failed".clone_into(&mut verdict_str);
                failure_reason = Some(err.to_string());
            }
        }
    } else if let Some(existing_rev) = exec
        .review_runs
        .iter()
        .find(|r| r.stage_run_id.as_deref() == Some(&sr.id))
    {
        verdict_str.clone_from(&existing_rev.verdict);
    }

    let sr_status = if settlement.ending == AgentEnding::Completed
        && verdict_str != "failed"
        && failure_reason.is_none()
    {
        "completed"
    } else {
        "failed"
    };
    let last_err = if sr_status == "failed" {
        failure_reason.or_else(|| Some(settlement.detail.clone()))
    } else {
        None
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
            last_error: last_err.as_deref(),
            ended_at: Some(&now),
        },
        &now,
    );
    Some((card.project_id.clone(), sr.workflow_run_id, verdict_str))
}
