#![forbid(unsafe_code)]
//! Preflight validation, DAG readiness, worktree copy helpers, and stage-run lookup
//! for M3 Execute and Review stage launches.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput,
};
use legion_persistence::workflow_board_repo::types::StageRunRow;
use legion_persistence::workflow_board_repo::types_m2::TaskContractRow;
use legion_persistence::workflow_board_repo::types_m3::CardExecutionDetail;
use legion_persistence::workflow_board_repo::{
    effective_execute_plan_approval, find_reusable_stage_run_for_task, get_card_execution_detail,
    next_stage_attempt, verify_task_claim_preconditions,
};
use rusqlite::{Connection, OptionalExtension as _};

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_fs::resolve_card_workspace_root;
use crate::workflow_board_snapshot::reconcile_card_and_snapshot;
use crate::workflow_board_stage_prep::{
    ResolvedLaunchConfig, StageLaunchEnv, resolve_launch_config, resolve_resume_launch_config,
};
use crate::workflow_board_types::{check_revision, db_lock_err, map_repo_err, parse_expected_rev};

pub(crate) struct M3LaunchContext {
    pub stage: WorkflowBoardStageRunStage,
    pub exec: CardExecutionDetail,
    pub ws_root: PathBuf,
    pub snapshot_digest: String,
    pub target_task_id: Option<String>,
    pub cfg: ResolvedLaunchConfig,
    pub reusable: Option<StageRunRow>,
    pub next_attempt: i64,
    pub expected_rev_i64: i64,
    pub now_iso: String,
}

pub(crate) fn load_and_validate_m3_launch(
    env: &StageLaunchEnv<'_>,
    card_id: &str,
    input: &WorkflowBoardStartStageInput,
    notify_project: &impl Fn(&str),
) -> Result<M3LaunchContext, WorkflowBoardRequestError> {
    let now_iso = utc_now_iso();
    let mut expected_rev_i64 = parse_expected_rev(input.expected_revision)?;
    let conn = env
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| db_lock_err())?;

    let initial = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    let ws_root = resolve_card_workspace_root(&conn, &initial.planning.card_detail.card)?;
    let (invalidated, snapshot_digest) =
        reconcile_card_and_snapshot(&conn, card_id, &ws_root, &now_iso)?;
    if invalidated {
        notify_project(&initial.planning.card_detail.card.project_id);
    }

    let mut exec = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    let (plan_approval_rev, stage_str) = validate_m3_non_task_preconditions(&exec, card_id, input)?;
    if input.stage == WorkflowBoardStageRunStage::Execute
        && let Some(next_rev) =
            crate::workflow_board_stage_m3_policy::enforce_or_acknowledge_cli_policies(
                &conn,
                &exec,
                &ws_root,
                card_id,
                input,
                expected_rev_i64,
                &now_iso,
            )?
    {
        expected_rev_i64 = next_rev;
        notify_project(&exec.planning.card_detail.card.project_id);
        exec = get_card_execution_detail(&conn, card_id).map_err(map_repo_err)?;
    }
    let target_task_id = match input.stage {
        WorkflowBoardStageRunStage::Execute => {
            let tid = resolve_target_task_id(&exec, input)?;
            verify_task_claim_preconditions(&conn, card_id, expected_rev_i64, &tid)
                .map_err(map_repo_err)?;
            Some(tid)
        }
        _ => None,
    };
    let detail = &exec.planning.card_detail;

    // Review always starts on a fresh thread unless caller provides an explicit idempotent key for an active running review.
    let reusable =
        find_reusable_stage_run_for_task(&conn, card_id, stage_str, target_task_id.as_deref())
            .map_err(map_repo_err)?
            .filter(|sr| {
                sr.card_revision >= plan_approval_rev
                    && sr.thread_id.is_some()
                    && (stage_str != "review"
                        || (sr.status == "running"
                            && input.stable_launch_key.as_deref()
                                == Some(sr.stable_launch_key.as_str())))
                    && (input.stable_launch_key.is_none()
                        || input.stable_launch_key.as_deref()
                            == Some(sr.stable_launch_key.as_str()))
            });
    let cfg = match reusable {
        Some(ref sr) => resolve_resume_launch_config(&conn, detail, sr, input)?,
        None => resolve_launch_config(&conn, detail, input, env.settings_path)?,
    };
    let next_attempt = next_stage_attempt(&conn, card_id, stage_str).map_err(map_repo_err)?;
    Ok(M3LaunchContext {
        stage: input.stage,
        exec,
        ws_root,
        snapshot_digest,
        target_task_id,
        cfg,
        reusable,
        next_attempt,
        expected_rev_i64,
        now_iso,
    })
}

fn validate_m3_non_task_preconditions(
    exec: &CardExecutionDetail,
    card_id: &str,
    input: &WorkflowBoardStartStageInput,
) -> Result<(i64, &'static str), WorkflowBoardRequestError> {
    let detail = &exec.planning.card_detail;
    check_revision(card_id, detail.card.revision, input.expected_revision)?;
    if detail.card.archived_at.is_some()
        || matches!(
            detail.card.status.as_str(),
            "paused" | "completed" | "shipped"
        )
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!(
                "Cannot start `{:?}` while card status is `{}`",
                input.stage, detail.card.status
            ),
        ));
    }

    let stage_str = match input.stage {
        WorkflowBoardStageRunStage::Execute => {
            if detail.card.stage != "execute" {
                return Err(WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InvalidStageTransition,
                    format!(
                        "Cannot start Execute in `{}` stage (expected `execute`)",
                        detail.card.stage
                    ),
                ));
            }
            "execute"
        }
        WorkflowBoardStageRunStage::Review => {
            if !matches!(detail.card.stage.as_str(), "execute" | "ship") {
                return Err(WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InvalidStageTransition,
                    format!("Cannot start Review in `{}` stage", detail.card.stage),
                ));
            }
            "review"
        }
        _ => {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidStageTransition,
                "Unsupported M3 stage",
            ));
        }
    };

    let allow_hist =
        input.stage == WorkflowBoardStageRunStage::Execute && exec.cli_import.is_some();
    let plan_approval =
        effective_execute_plan_approval(allow_hist, &detail.approvals).ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::PlanNotApproved,
                "Cannot start Execute or Review without a valid Plan approval",
            )
        })?;
    if detail.incomplete_journal_count > 0
        || exec
            .external_actions
            .iter()
            .any(|a| a.status == "interrupted")
        || detail
            .card
            .external_edit_state
            .as_deref()
            .is_some_and(|s| s != "clean")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ExternalEditDetected,
            "Unreconciled external edits or interrupted actions must be resolved before starting",
        ));
    }
    if exec
        .contract_violations
        .iter()
        .any(|v| v.status == "violation_pending")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ContractConflict,
            "Unresolved file-contract violation must be resolved or amended before starting",
        ));
    }

    if input.stage == WorkflowBoardStageRunStage::Review {
        validate_review_stage_preconditions(exec, input)?;
    }
    Ok((plan_approval.revision, stage_str))
}

pub(crate) fn resolve_target_task_id(
    exec: &CardExecutionDetail,
    input: &WorkflowBoardStartStageInput,
) -> Result<String, WorkflowBoardRequestError> {
    let tasks = &exec.planning.tasks;
    if let Some(active) = tasks.iter().find(|t| t.status == "in_progress") {
        if let Some(ref requested) = input.task_id
            && requested.trim() != active.task_id
        {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ContractConflict,
                format!(
                    "Task `{}` is already in_progress; only one mutating task can execute at a time",
                    active.task_id
                ),
            ));
        }
        return Ok(active.task_id.clone());
    }
    if let Some(ref requested) = input.task_id {
        let tid = requested.trim();
        let task = tasks.iter().find(|t| t.task_id == tid).ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                format!("Task `{tid}` not found on card"),
            )
        })?;
        if !matches!(task.status.as_str(), "todo" | "failed") {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidStageTransition,
                format!(
                    "Task `{tid}` has status `{}` and cannot be started",
                    task.status
                ),
            ));
        }
        if !is_task_dag_ready(tasks, tid) {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ContractConflict,
                format!("Task `{tid}` is blocked by incomplete DAG dependencies"),
            ));
        }
        return Ok(tid.to_owned());
    }
    tasks
        .iter()
        .find(|t| t.status == "todo" && is_task_dag_ready(tasks, &t.task_id))
        .or_else(|| {
            tasks
                .iter()
                .find(|t| t.status == "failed" && is_task_dag_ready(tasks, &t.task_id))
        })
        .map(|t| t.task_id.clone())
        .ok_or_else(|| crate::workflow_board_stage_m3_policy::blocked_task_conflict_error(tasks))
}

pub(crate) fn is_task_dag_ready(tasks: &[TaskContractRow], target_task_id: &str) -> bool {
    let by_id: HashMap<&str, &TaskContractRow> =
        tasks.iter().map(|t| (t.task_id.as_str(), t)).collect();
    let Some(target) = by_id.get(target_task_id).copied() else {
        return false;
    };
    let mut deps: HashSet<String> = serde_json::from_str::<Vec<String>>(&target.blocked_by_json)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_owned())
        .collect();
    for other in tasks {
        if other.task_id != target_task_id {
            let blocks: Vec<String> = serde_json::from_str(&other.blocks_json).unwrap_or_default();
            if blocks.iter().any(|b| b.trim() == target_task_id) {
                deps.insert(other.task_id.clone());
            }
        }
    }
    deps.iter().all(|dep_id| {
        by_id
            .get(dep_id.as_str())
            .is_some_and(|d| matches!(d.status.as_str(), "completed" | "skipped"))
    })
}

pub(crate) use crate::workflow_board_stage_m3_policy::copy_card_dir_if_new_worktree;

pub(crate) fn get_stage_run_by_id(
    conn: &Connection,
    id: &str,
) -> Result<Option<StageRunRow>, rusqlite::Error> {
    conn.query_row(
        "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                card_revision, provider_instance_id, model, model_options_json,
                runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                branch, thread_id, workflow_run_id, brief_digest, result_json,
                last_error, created_at, updated_at, ended_at
         FROM workflow_board_stage_runs WHERE id = ?1",
        [id],
        |row| {
            Ok(StageRunRow {
                id: row.get(0)?,
                card_id: row.get(1)?,
                stage: row.get(2)?,
                task_id: row.get(3)?,
                attempt: row.get(4)?,
                stable_launch_key: row.get(5)?,
                status: row.get(6)?,
                card_revision: row.get(7)?,
                provider_instance_id: row.get(8)?,
                model: row.get(9)?,
                model_options_json: row.get(10)?,
                runtime_mode: row.get(11)?,
                interaction_mode: row.get(12)?,
                workspace_strategy_json: row.get(13)?,
                worktree_path: row.get(14)?,
                branch: row.get(15)?,
                thread_id: row.get(16)?,
                workflow_run_id: row.get(17)?,
                brief_digest: row.get(18)?,
                result_json: row.get(19)?,
                last_error: row.get(20)?,
                created_at: row.get(21)?,
                updated_at: row.get(22)?,
                ended_at: row.get(23)?,
            })
        },
    )
    .optional()
}

fn validate_review_stage_preconditions(
    exec: &CardExecutionDetail,
    input: &WorkflowBoardStartStageInput,
) -> Result<(), WorkflowBoardRequestError> {
    let detail = &exec.planning.card_detail;
    if !matches!(detail.card.stage.as_str(), "execute" | "ship") {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            format!("Cannot start Review in `{}` stage", detail.card.stage),
        ));
    }
    if let Some(in_prog) = exec
        .planning
        .tasks
        .iter()
        .find(|t| t.status == "in_progress")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ContractConflict,
            format!(
                "Cannot start Review while task `{}` is still in_progress",
                in_prog.task_id
            ),
        ));
    }
    if let Some(ref key) = input.stable_launch_key
        && detail
            .stage_runs
            .iter()
            .any(|sr| sr.stable_launch_key == key.trim() && sr.stage != "review")
    {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::ContractConflict,
            "Review stage must use a distinct stableLaunchKey never shared with Spec/Plan/Execute",
        ));
    }
    Ok(())
}
