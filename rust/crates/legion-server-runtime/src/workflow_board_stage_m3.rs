#![forbid(unsafe_code)]
//! Execute (`workflowBoard.execute@1`) and fresh-thread Review (`workflowBoard.review@1`)
//! stage launching with baseline capture, worktree backfill, and task claim.

use std::path::Path;

use legion_contracts::ids::{RunId, ThreadId, WorkflowBoardStageRunId, WorkflowCardId};
use legion_contracts::orchestration::WorkspaceStrategy;
use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput, WorkflowBoardStartStageResult,
};
use legion_persistence::workflow_board_repo::types_m3::ClaimOrStartTaskInput;
use legion_persistence::workflow_board_repo::{
    claim_or_start_task, get_card_detail, link_stage_run_thread, set_card_worktree_if_unset,
};
use serde_json::json;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_briefs::stage_brief_and_upsert_run;
use crate::workflow_board_briefs_m3::{
    build_execute_stage_payload, prepare_execute_brief_inputs, prepare_review_brief_inputs,
    sync_materialized_worktree_baseline_and_brief,
};
use crate::workflow_board_checks::sync_card_json_file;
use crate::workflow_board_diff::{capture_worktree_baseline, save_task_baseline};
use crate::workflow_board_fs::{
    ensure_confined_card_dir, resolve_card_workspace_root, validate_safe_card_id,
};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_stage::launch_fresh_workflow_run;
use crate::workflow_board_stage_exec::{finalize_fresh_stage_run, resume_existing_stage_run};
use crate::workflow_board_stage_m3_preflight::{
    M3LaunchContext, copy_card_dir_if_new_worktree, load_and_validate_m3_launch,
};
use crate::workflow_board_stage_prep::{StageLaunchEnv, internal_err, query_thread_workspace};
use crate::workflow_board_turn_observer::settle_thread_if_ready;
use crate::workflow_board_types::{db_lock_err, map_repo_err};

pub(crate) async fn start_m3_stage_impl(
    env: &StageLaunchEnv<'_>,
    input: &WorkflowBoardStartStageInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let mut res = {
        let _card_guard = acquire_card_lock(env.orchestrator, card_id).await;
        let ctx = load_and_validate_m3_launch(env, card_id, input, &notify_project)?;

        if let Some(ref sr) = ctx.reusable {
            return resume_existing_stage_run(
                env,
                ctx.stage,
                &ctx.exec.planning.card_detail,
                sr,
                &ctx.cfg,
            )
            .await;
        }

        let mgr = env.workflow_manager.ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::LaunchFailed,
                "Workflow runtime is unavailable",
            )
        })?;
        let engine = mgr.engine();
        if !engine.is_enabled() {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::LaunchFailed,
                legion_workflows::engine::WORKFLOWS_OFF_MESSAGE,
            ));
        }

        if ctx.stage == WorkflowBoardStageRunStage::Execute {
            launch_execute_stage(env, &engine, &ctx, card_id, input, &notify_project).await?
        } else {
            launch_review_stage(env, &engine, &ctx, card_id, input, &notify_project).await?
        }
    };
    settle_thread_if_ready(
        env.orchestrator,
        env.workflow_manager,
        res.thread_id.as_str(),
        &notify_project,
    )
    .await;
    if let Ok(conn) = env.orchestrator.sink().db().lock()
        && let Ok(detail) = get_card_detail(&conn, card_id)
    {
        res.revision = u64::try_from(detail.card.revision.max(1)).unwrap_or(res.revision);
    }
    Ok(res)
}

async fn launch_execute_stage(
    env: &StageLaunchEnv<'_>,
    engine: &std::sync::Arc<legion_workflows::WorkflowEngine>,
    ctx: &M3LaunchContext,
    card_id: &str,
    input: &WorkflowBoardStartStageInput,
    notify_project: &impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let task_id = ctx
        .target_task_id
        .as_deref()
        .ok_or_else(|| internal_err("Missing target task_id"))?;
    let detail = &ctx.exec.planning.card_detail;
    let task_row = ctx
        .exec
        .planning
        .tasks
        .iter()
        .find(|t| t.task_id == task_id)
        .ok_or_else(|| internal_err("Target task row missing"))?;
    let stable_launch_key = input.stable_launch_key.clone().unwrap_or_else(|| {
        format!(
            "board:{card_id}:execute:{task_id}:rev-{}:attempt-{}",
            detail.card.revision, ctx.next_attempt
        )
    });
    let prep = prepare_execute_brief_inputs(
        env,
        &ctx.exec,
        &ctx.cfg,
        task_id,
        Some(&ctx.snapshot_digest),
    )?;
    let will_materialize_worktree = detail.card.worktree_path.is_none()
        && matches!(
            ctx.cfg.workspace_strategy,
            WorkspaceStrategy::Worktree { .. }
        );
    let baseline = if will_materialize_worktree {
        None
    } else if let WorkspaceStrategy::ExistingWorktree {
        ref worktree_path, ..
    } = ctx.cfg.workspace_strategy
    {
        Some(capture_worktree_baseline(Path::new(worktree_path)))
    } else {
        Some(capture_worktree_baseline(&ctx.ws_root))
    };
    let stage_run_row = stage_brief_and_upsert_run(
        env,
        detail,
        &ctx.cfg,
        "execute",
        Some(task_id),
        &prep.brief_filename,
        &prep.card_dir,
        &prep.markdown,
        &prep.brief_digest,
        &stable_launch_key,
        ctx.next_attempt,
        &ctx.now_iso,
    )?;
    if let Some(ref base) = baseline {
        save_task_baseline(&prep.card_dir, &stage_run_row.id, base)?;
    }

    let payload = build_execute_stage_payload(
        env,
        detail,
        &ctx.cfg,
        &ctx.snapshot_digest,
        card_id,
        task_id,
        task_row,
        &prep,
        &stable_launch_key,
    );
    let (wf_run_id, thread_id) = launch_fresh_workflow_run(
        env,
        engine,
        legion_workflows::WORKFLOW_BOARD_EXECUTE_NAME,
        legion_workflows::WORKFLOW_BOARD_EXECUTE_VERSION,
        "execute",
        &stage_run_row.id,
        payload,
        &stable_launch_key,
    )
    .await?;
    finalize_execute_stage_claim(
        env,
        ctx,
        card_id,
        task_id,
        &stage_run_row.id,
        &wf_run_id,
        &thread_id,
        &prep.card_dir,
        notify_project,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Commits worktree backfill, thread link, and task claim atomically after launch"
)]
fn finalize_execute_stage_claim(
    env: &StageLaunchEnv<'_>,
    ctx: &M3LaunchContext,
    card_id: &str,
    task_id: &str,
    stage_run_id: &str,
    wf_run_id: &str,
    thread_id: &str,
    prep_card_dir: &Path,
    notify_project: &impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let detail = &ctx.exec.planning.card_detail;
    let latest_rev = {
        let conn = env
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| db_lock_err())?;
        let (wt_path, branch) = query_thread_workspace(&conn, thread_id);
        let done_iso = utc_now_iso();
        let newly_materialized = if let Some(ref wt) = wt_path
            && !wt.trim().is_empty()
        {
            let created =
                set_card_worktree_if_unset(&conn, card_id, wt, branch.as_deref(), &done_iso)
                    .unwrap_or(false);
            if created {
                copy_card_dir_if_new_worktree(prep_card_dir, Path::new(wt), card_id);
            }
            created
        } else {
            false
        };
        let rev_id = format!("rev-{}", Uuid::new_v4().simple());
        let claimed = claim_or_start_task(
            &conn,
            &ClaimOrStartTaskInput {
                card_id,
                revision_id: &rev_id,
                expected_revision: ctx.expected_rev_i64,
                task_id,
                stage_run_id: Some(stage_run_id),
            },
            &done_iso,
        )
        .map_err(map_repo_err)?;
        link_stage_run_thread(
            &conn,
            &legion_persistence::workflow_board_repo::types::LinkStageRunThreadInput {
                id: stage_run_id,
                status: "running",
                thread_id: Some(thread_id),
                workflow_run_id: Some(wf_run_id),
                worktree_path: wt_path.as_deref(),
                branch: branch.as_deref(),
                last_error: None,
                ended_at: None,
            },
            &done_iso,
        )
        .map_err(map_repo_err)?;
        let active_ws = resolve_card_workspace_root(&conn, &claimed.planning.card_detail.card)?;
        if let Ok(active_dir) = ensure_confined_card_dir(&active_ws, card_id) {
            if newly_materialized {
                sync_materialized_worktree_baseline_and_brief(
                    &conn,
                    &active_ws,
                    &active_dir,
                    card_id,
                    stage_run_id,
                    task_id,
                    &ctx.cfg,
                    &done_iso,
                )?;
            }
            let _ = sync_card_json_file(&active_dir, &claimed.planning.card_detail.card);
        }
        claimed.planning.card_detail.card.revision
    };
    notify_project(&detail.card.project_id);

    Ok(WorkflowBoardStartStageResult {
        card_id: WorkflowCardId::new(card_id).map_err(|e| internal_err(&e.to_string()))?,
        revision: u64::try_from(latest_rev.max(1)).unwrap_or(1),
        stage: WorkflowBoardStageRunStage::Execute,
        task_id: Some(task_id.to_owned()),
        stage_run_id: WorkflowBoardStageRunId::new(stage_run_id)
            .map_err(|e| internal_err(&e.to_string()))?,
        run_id: Some(RunId::new(wf_run_id).map_err(|e| internal_err(&e.to_string()))?),
        thread_id: ThreadId::new(thread_id).map_err(|e| internal_err(&e.to_string()))?,
        resumed: false,
    })
}

async fn launch_review_stage(
    env: &StageLaunchEnv<'_>,
    engine: &std::sync::Arc<legion_workflows::WorkflowEngine>,
    ctx: &M3LaunchContext,
    card_id: &str,
    input: &WorkflowBoardStartStageInput,
    notify_project: &impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let detail = &ctx.exec.planning.card_detail;
    let stable_launch_key = input.stable_launch_key.clone().unwrap_or_else(|| {
        format!(
            "board:{card_id}:review:rev-{}:attempt-{}",
            detail.card.revision, ctx.next_attempt
        )
    });
    let prep = prepare_review_brief_inputs(env, &ctx.exec, &ctx.cfg, &ctx.snapshot_digest)?;
    let stage_run_row = stage_brief_and_upsert_run(
        env,
        detail,
        &ctx.cfg,
        "review",
        None,
        &prep.brief_filename,
        &prep.card_dir,
        &prep.markdown,
        &prep.brief_digest,
        &stable_launch_key,
        ctx.next_attempt,
        &ctx.now_iso,
    )?;
    let payload = json!({
        "cardId": card_id,
        "projectId": detail.card.project_id,
        "title": detail.card.title,
        "briefMarkdown": prep.markdown,
        "briefPath": prep.brief_path,
        "specPath": prep.spec_path,
        "planPath": prep.card_dir.join("plan.md").to_string_lossy().into_owned(),
        "workspaceRoot": prep.ws_root.to_string_lossy().into_owned(),
        "worktreePath": detail.card.worktree_path,
        "modelSelection": ctx.cfg.model_selection,
        "runtimeMode": ctx.cfg.runtime_mode,
        "interactionMode": ctx.cfg.interaction_mode,
        "workspaceStrategy": ctx.cfg.workspace_strategy,
        "environmentId": env.environment_id,
        "stableLaunchKey": stable_launch_key,
        "productSnapshotDigest": ctx.snapshot_digest,
    });
    let (wf_run_id, thread_id) = launch_fresh_workflow_run(
        env,
        engine,
        legion_workflows::WORKFLOW_BOARD_REVIEW_NAME,
        legion_workflows::WORKFLOW_BOARD_REVIEW_VERSION,
        "review",
        &stage_run_row.id,
        payload,
        &stable_launch_key,
    )
    .await?;
    finalize_fresh_stage_run(
        env,
        WorkflowBoardStageRunStage::Review,
        card_id,
        &detail.card.project_id,
        ctx.expected_rev_i64,
        "await_review",
        &stage_run_row.id,
        &wf_run_id,
        &thread_id,
        notify_project,
    )
}
