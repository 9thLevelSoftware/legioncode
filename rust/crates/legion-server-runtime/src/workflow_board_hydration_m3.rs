#![forbid(unsafe_code)]
//! Full M3 execution detail hydration (`CardExecutionDetail` -> `WorkflowCardDetail`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use legion_contracts::ids::{RunId, ThreadId};
use legion_contracts::workflow_board::{
    WorkflowBoardCheckStatus, WorkflowBoardFileContractStatus, WorkflowBoardGetCardInput,
    WorkflowBoardPlanDocument, WorkflowBoardPlannedCheckReceipt, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardTaskExecutionState,
    WorkflowBoardTaskFileContractReport, WorkflowCardDetail,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types::WriteJournalRow;
use legion_persistence::workflow_board_repo::types_m3::CardExecutionDetail;
use legion_persistence::workflow_board_repo::{
    get_card_execution_detail, list_incomplete_journal_entries,
};
use rusqlite::Connection;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_fs::{
    enumerate_artifact_keys, resolve_card_workspace_root, validate_safe_card_id,
};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_hydration_ship::{
    hydrate_ac_observations, hydrate_amendments, hydrate_recovery_state, hydrate_review_report,
    hydrate_ship_readiness, to_delivery_receipt,
};
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_snapshot::reconcile_card_and_snapshot;
use crate::workflow_board_types::{
    db_lock_err, internal, map_repo_err, parse_enum, parse_opt_enum,
};

/// Reconciles external/snapshot state and hydrates full [`WorkflowCardDetail`] for `getCard`.
pub(crate) fn get_card_detail_impl(
    orchestrator: &Orchestrator,
    input: &WorkflowBoardGetCardInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let now_iso = utc_now_iso();
    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let initial = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    let card = &initial.planning.card_detail.card;
    if let Some(ref pid) = input.project_id
        && pid.as_str() != card.project_id
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::CardNotFound,
            format!(
                "Card `{card_id}` was not found in project `{}`",
                pid.as_str()
            ),
        ));
    }
    let ws_root = resolve_card_workspace_root(&conn, card)?;
    let invalidated = if card.product_snapshot_digest.is_some()
        || matches!(card.stage.as_str(), "execute" | "review" | "ship")
    {
        reconcile_card_and_snapshot(&conn, card_id, &ws_root, &now_iso)?.0
    } else {
        reconcile_card_external_edits(&conn, card_id, &now_iso)?
    };
    if invalidated {
        notify_project(&card.project_id);
    }
    load_hydrated_card_detail(&conn, &ws_root, card_id)
}

/// Loads and hydrates full M1 + M2 + M3 [`WorkflowCardDetail`] from SQLite + workspace keys.
pub(crate) fn load_hydrated_card_detail(
    conn: &Connection,
    ws_root: &Path,
    card_id: &str,
) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
    let exec = get_card_execution_detail(conn, card_id).map_err(map_repo_err)?;
    let pending_journal =
        list_incomplete_journal_entries(conn, Some(card_id)).map_err(map_repo_err)?;
    let keys = enumerate_artifact_keys(ws_root, card_id);
    to_card_detail_from_execution(&exec, &pending_journal, keys)
}

/// Hydrates full M1 + M2 + M3 fields onto [`WorkflowCardDetail`].
pub(crate) fn to_card_detail_from_execution(
    exec: &CardExecutionDetail,
    pending_journal: &[WriteJournalRow],
    artifact_keys: Vec<String>,
) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
    let mut detail = to_card_detail_from_planning(&exec.planning, artifact_keys)?;
    let card = &exec.planning.card_detail.card;

    detail.task_executions = hydrate_task_executions(exec)?;
    detail.file_contract_reports = hydrate_file_contract_reports(exec)?;
    detail.check_receipts = hydrate_check_receipts(exec, detail.plan.as_ref())?;
    detail.review_report = hydrate_review_report(exec)?;
    detail.ac_observations = hydrate_ac_observations(exec)?;
    detail.delivery_receipt = exec
        .delivery_receipts
        .first()
        .map(to_delivery_receipt)
        .transpose()?;
    detail.ship_readiness = Some(hydrate_ship_readiness(
        exec,
        detail.delivery_receipt.clone(),
    )?);
    if card.stage == "ship" && detail.summary.delivery_status.is_none() {
        detail.summary.delivery_status = detail.ship_readiness.as_ref().map(|r| r.status);
    }
    detail.recovery_state = Some(hydrate_recovery_state(exec, pending_journal)?);
    detail.amendments = hydrate_amendments(exec)?;
    if let Some(ref cli) = exec.cli_import
        && let Ok(parsed) = serde_json::from_str(&cli.import_provenance_json)
    {
        detail.cli_import = Some(parsed);
    }
    Ok(detail)
}

fn hydrate_task_executions(
    exec: &CardExecutionDetail,
) -> Result<Vec<WorkflowBoardTaskExecutionState>, WorkflowBoardRequestError> {
    let stage_runs = &exec.planning.card_detail.stage_runs;
    let card = &exec.planning.card_detail.card;
    let mut out = Vec::with_capacity(exec.planning.tasks.len());
    for t in &exec.planning.tasks {
        let sr = t
            .active_stage_run_id
            .as_deref()
            .and_then(|id| stage_runs.iter().find(|r| r.id == id))
            .or_else(|| {
                stage_runs
                    .iter()
                    .filter(|r| r.stage == "execute" && r.task_id.as_deref() == Some(&t.task_id))
                    .max_by_key(|r| (r.attempt, &r.updated_at))
            });
        out.push(WorkflowBoardTaskExecutionState {
            task_id: t.task_id.clone(),
            status: parse_enum(&t.status, "task_execution.status")?,
            thread_id: sr
                .and_then(|r| r.thread_id.as_deref())
                .map(ThreadId::new)
                .transpose()
                .map_err(|e| internal(e.to_string()))?,
            run_id: sr
                .and_then(|r| r.workflow_run_id.as_deref())
                .map(RunId::new)
                .transpose()
                .map_err(|e| internal(e.to_string()))?,
            stage_run_id: sr
                .map(|r| r.id.clone())
                .or_else(|| t.active_stage_run_id.clone()),
            worktree_path: sr
                .and_then(|r| r.worktree_path.clone())
                .or_else(|| card.worktree_path.clone()),
            branch: sr
                .and_then(|r| r.branch.clone())
                .or_else(|| card.branch.clone()),
            attempt: u64::try_from(t.attempt_count.max(1)).unwrap_or(1),
            started_at: t.started_at.clone(),
            ended_at: t
                .completed_at
                .clone()
                .or_else(|| sr.and_then(|r| r.ended_at.clone())),
            last_error: t
                .last_error
                .clone()
                .or_else(|| sr.and_then(|r| r.last_error.clone())),
        });
    }
    Ok(out)
}

fn hydrate_file_contract_reports(
    exec: &CardExecutionDetail,
) -> Result<Vec<WorkflowBoardTaskFileContractReport>, WorkflowBoardRequestError> {
    let mut out = Vec::new();
    let mut tasks_with_reports = HashSet::new();
    for v in &exec.contract_violations {
        tasks_with_reports.insert(v.task_id.as_str());
        out.push(WorkflowBoardTaskFileContractReport {
            id: Some(v.id.clone()),
            task_id: v.task_id.clone(),
            thread_id: v
                .thread_id
                .as_deref()
                .map(ThreadId::new)
                .transpose()
                .map_err(|e| internal(e.to_string()))?,
            run_id: v
                .workflow_run_id
                .as_deref()
                .map(RunId::new)
                .transpose()
                .map_err(|e| internal(e.to_string()))?,
            status: parse_enum(&v.status, "file_contract.status")?,
            changed_files: serde_json::from_str(&v.changed_files_json).unwrap_or_default(),
            allowed_files: serde_json::from_str(&v.allowed_files_json).unwrap_or_default(),
            unexpected_files: serde_json::from_str(&v.unexpected_files_json).unwrap_or_default(),
            forbidden_files: serde_json::from_str(&v.forbidden_files_json).unwrap_or_default(),
            preserved_in_worktree: v.preserved_in_worktree,
            blocking_reason: v.blocking_reason.clone(),
            resolution_disposition: parse_opt_enum(
                v.resolution_disposition.as_deref(),
                "file_contract.resolution_disposition",
            )?,
            resolution_note: v.resolution_note.clone(),
            resolved_by: v.resolved_by.clone(),
            resolved_at: v.resolved_at.clone(),
            updated_at: v.updated_at.clone(),
        });
    }
    for t in &exec.planning.tasks {
        if t.status == "completed" && !tasks_with_reports.contains(t.task_id.as_str()) {
            let sr = t.active_stage_run_id.as_deref().and_then(|id| {
                exec.planning
                    .card_detail
                    .stage_runs
                    .iter()
                    .find(|r| r.id == id)
            });
            out.push(WorkflowBoardTaskFileContractReport {
                id: None,
                task_id: t.task_id.clone(),
                thread_id: sr
                    .and_then(|r| r.thread_id.as_deref())
                    .map(ThreadId::new)
                    .transpose()
                    .map_err(|e| internal(e.to_string()))?,
                run_id: sr
                    .and_then(|r| r.workflow_run_id.as_deref())
                    .map(RunId::new)
                    .transpose()
                    .map_err(|e| internal(e.to_string()))?,
                status: WorkflowBoardFileContractStatus::Clean,
                changed_files: Vec::new(),
                allowed_files: serde_json::from_str(&t.files_allowed_json).unwrap_or_default(),
                unexpected_files: Vec::new(),
                forbidden_files: Vec::new(),
                preserved_in_worktree: true,
                blocking_reason: None,
                resolution_disposition: None,
                resolution_note: None,
                resolved_by: None,
                resolved_at: None,
                updated_at: t.updated_at.clone(),
            });
        }
    }
    Ok(out)
}

fn hydrate_check_receipts(
    exec: &CardExecutionDetail,
    plan: Option<&WorkflowBoardPlanDocument>,
) -> Result<Vec<WorkflowBoardPlannedCheckReceipt>, WorkflowBoardRequestError> {
    let check_meta: HashMap<&str, (Option<&str>, Option<&str>)> = plan.map_or_default(|p| {
        p.checks
            .iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    (c.criterion_id.as_deref(), c.description.as_deref()),
                )
            })
            .collect()
    });
    let planned_by_id: HashMap<&str, _> = exec
        .planning
        .planned_checks
        .iter()
        .map(|c| (c.check_id.as_str(), c))
        .collect();

    let mut seen_checks = HashSet::new();
    let mut out = Vec::new();
    for r in &exec.check_receipts {
        seen_checks.insert(r.check_id.as_str());
        let (crit, desc) = check_meta
            .get(r.check_id.as_str())
            .copied()
            .unwrap_or((None, None));
        let p_row = planned_by_id.get(r.check_id.as_str()).copied();
        out.push(WorkflowBoardPlannedCheckReceipt {
            id: Some(r.id.clone()),
            check_id: r.check_id.clone(),
            criterion_id: r
                .criterion_id
                .clone()
                .or_else(|| crit.map(str::to_owned))
                .or_else(|| {
                    p_row
                        .and_then(|p| p.owner_task_id.clone())
                        .filter(|s| s.starts_with("AC-"))
                }),
            owner_task_id: r
                .task_id
                .clone()
                .or_else(|| p_row.and_then(|p| p.owner_task_id.clone())),
            argv: serde_json::from_str(&r.argv_json).unwrap_or_default(),
            description: desc
                .map(str::to_owned)
                .or_else(|| p_row.and_then(|p| p.label.clone())),
            status: parse_enum(&r.status, "check_receipt.status")?,
            exit_code: r.exit_code,
            stdout_excerpt: r.stdout_excerpt.clone(),
            stderr_excerpt: r.stderr_excerpt.clone(),
            log_ref: r.log_ref.clone(),
            product_snapshot_digest: r.snapshot_digest.clone(),
            commit_sha: r.commit_sha.clone(),
            spec_digest: r.spec_digest.clone(),
            plan_digest: r.plan_digest.clone(),
            stale_reason: r.stale_reason.clone(),
            executed_at: r.executed_at.clone(),
            updated_at: r.updated_at.clone(),
        });
    }
    for p in &exec.planning.planned_checks {
        if seen_checks.contains(p.check_id.as_str()) {
            continue;
        }
        let (crit, desc) = check_meta
            .get(p.check_id.as_str())
            .copied()
            .unwrap_or((None, None));
        out.push(WorkflowBoardPlannedCheckReceipt {
            id: p.last_receipt_id.clone(),
            check_id: p.check_id.clone(),
            criterion_id: crit.map(str::to_owned),
            owner_task_id: p.owner_task_id.clone(),
            argv: serde_json::from_str(&p.argv_json).unwrap_or_default(),
            description: desc.map(str::to_owned).or_else(|| p.label.clone()),
            status: parse_enum::<WorkflowBoardCheckStatus>(&p.status, "planned_check.status")?,
            exit_code: p.last_exit_code,
            stdout_excerpt: None,
            stderr_excerpt: p.last_output_excerpt.clone(),
            log_ref: p.last_log_ref.clone(),
            product_snapshot_digest: p.product_snapshot_digest.clone(),
            commit_sha: None,
            spec_digest: None,
            plan_digest: None,
            stale_reason: p.stale_reason.clone(),
            executed_at: p.last_run_at.clone(),
            updated_at: p.updated_at.clone(),
        });
    }
    Ok(out)
}
