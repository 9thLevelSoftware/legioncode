#![forbid(unsafe_code)]
//! Spec and Plan stage execution driver wiring (`workflowBoard.startStage("spec" | "plan")`).

use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput, WorkflowBoardStartStageResult,
};
use legion_persistence::workflow_board_repo::types::StageRunRow;
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;
use legion_persistence::workflow_board_repo::{
    find_reusable_stage_run, get_card_planning_detail, next_stage_attempt,
};
use legion_workflows::{
    WORKFLOW_BOARD_PLAN_NAME, WORKFLOW_BOARD_PLAN_VERSION, WORKFLOW_BOARD_SPEC_NAME,
    WORKFLOW_BOARD_SPEC_VERSION,
};
use serde_json::json;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_briefs::{
    prepare_plan_brief_inputs, prepare_spec_brief_inputs, stage_brief_and_upsert_run,
};
use crate::workflow_board_fs::validate_safe_card_id;
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_stage_exec::{finalize_fresh_stage_run, resume_existing_stage_run};
use crate::workflow_board_stage_m3::start_m3_stage_impl;
use crate::workflow_board_stage_prep::{
    ResolvedLaunchConfig, StageLaunchEnv, internal_err, record_stage_run_failure,
    resolve_launch_config, resolve_resume_launch_config,
};
use crate::workflow_board_types::map_repo_err;

struct LaunchContext {
    stage: WorkflowBoardStageRunStage,
    planning: CardPlanningDetail,
    cfg: ResolvedLaunchConfig,
    reusable: Option<StageRunRow>,
    next_attempt: i64,
    min_resume_revision: i64,
    spec_approval_id: Option<String>,
    spec_digest: Option<String>,
    expected_rev_i64: i64,
    now_iso: String,
}

pub(crate) async fn start_stage_impl(
    env: &StageLaunchEnv<'_>,
    input: &WorkflowBoardStartStageInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    match input.stage {
        WorkflowBoardStageRunStage::Spec | WorkflowBoardStageRunStage::Plan => {}
        WorkflowBoardStageRunStage::Execute | WorkflowBoardStageRunStage::Review => {
            return start_m3_stage_impl(env, input, notify_project).await;
        }
        WorkflowBoardStageRunStage::Ship => {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidStageTransition,
                "Ship stage does not launch an agent thread; use Ship delivery actions",
            ));
        }
    }
    let _card_guard = acquire_card_lock(env.orchestrator, card_id).await;
    let mut ctx = load_and_validate_launch(env, card_id, input, &notify_project)?;

    if let Some(ref sr) = ctx.reusable
        && is_resumable_stage_run(sr, ctx.min_resume_revision, input)
    {
        return resume_existing_stage_run(env, ctx.stage, &ctx.planning.card_detail, sr, &ctx.cfg)
            .await;
    }

    let stable_launch_key = select_launch_key(input, &ctx, card_id);
    execute_fresh_stage_launch(env, &mut ctx, card_id, &stable_launch_key, &notify_project).await
}

async fn prepare_stage_workspace(
    env: &StageLaunchEnv<'_>,
    ctx: &mut LaunchContext,
    stable_launch_key: &str,
) -> Result<(), WorkflowBoardRequestError> {
    use crate::thread_launcher::{LaunchThreadRequest, ThreadLauncher};
    use legion_contracts::ids::CommandId;
    use legion_contracts::orchestration::{CreationSource, OrchestrationActor};

    if ctx.stage != WorkflowBoardStageRunStage::Plan {
        return Ok(());
    }
    let card = &ctx.planning.card_detail.card;
    let launcher = ThreadLauncher::new(
        std::sync::Arc::clone(env.orchestrator),
        env.worktrees_dir.clone(),
    )
    .with_settings_path(env.settings_path.cloned());
    let (branch, worktree_path) = launcher
        .prepare_launch_workspace(&LaunchThreadRequest {
            command_id: CommandId::new(format!("board-prepare-{}", uuid::Uuid::new_v4()))
                .map_err(|e| internal_err(&e.to_string()))?,
            created_by: OrchestrationActor::System,
            creation_source: CreationSource::Server,
            thread_id: None,
            stable_launch_key: Some(stable_launch_key.to_owned()),
            reuse_existing_thread: true,
            project_id: card.project_id.clone(),
            title: format!("Plan: {}", card.title),
            title_renamed: Some(true),
            generate_title: Some(false),
            model_selection: ctx.cfg.model_selection.clone(),
            runtime_mode: ctx.cfg.runtime_mode,
            interaction_mode: ctx.cfg.interaction_mode,
            workspace_strategy: ctx.cfg.workspace_strategy.clone(),
            workflow_run: None,
            link_origin: None,
            delegated_from: None,
            initial_message: None,
        })
        .await
        .map_err(crate::workflow_board_types::map_launch_err)?;
    if let Some(ref worktree) = worktree_path {
        let previous_root = {
            let conn = env
                .orchestrator
                .sink()
                .db()
                .lock()
                .map_err(|_| internal_err("Database lock poisoned"))?;
            crate::workflow_board_fs::resolve_card_workspace_root(&conn, card)?
        };
        let source =
            crate::workflow_board_fs::ensure_confined_card_dir(&previous_root, &card.card_id)?;
        crate::workflow_board_stage_m3_preflight::copy_card_dir_if_new_worktree(
            &source,
            std::path::Path::new(worktree),
            &card.card_id,
        );
    }
    ctx.planning.card_detail.card.worktree_path = worktree_path;
    ctx.planning.card_detail.card.branch = branch;
    Ok(())
}

fn is_resumable_stage_run(
    sr: &StageRunRow,
    min_resume_revision: i64,
    input: &WorkflowBoardStartStageInput,
) -> bool {
    sr.card_revision >= min_resume_revision
        && sr.thread_id.is_some()
        && (input.stable_launch_key.is_none()
            || input.stable_launch_key.as_deref() == Some(sr.stable_launch_key.as_str()))
}

fn load_and_validate_launch(
    env: &StageLaunchEnv<'_>,
    card_id: &str,
    input: &WorkflowBoardStartStageInput,
    notify_project: &impl Fn(&str),
) -> Result<LaunchContext, WorkflowBoardRequestError> {
    let now_iso = utc_now_iso();
    let expected_rev_i64 = i64::try_from(input.expected_revision).map_err(|_| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "expectedRevision out of range",
        )
    })?;
    let conn = env
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| internal_err("Database lock poisoned"))?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    let detail = &planning.card_detail;
    if invalidated {
        notify_project(&detail.card.project_id);
    }
    if detail.card.revision != expected_rev_i64 {
        let cur = u64::try_from(detail.card.revision.max(1)).unwrap_or(1);
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::RevisionConflict,
            format!(
                "Revision conflict on card `{card_id}`: expected {}, current {cur}",
                input.expected_revision
            ),
        )
        .with_current_revision(cur));
    }
    if detail.card.archived_at.is_some() {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidStageTransition,
            "Cannot start a stage on an archived card",
        ));
    }

    let (stage_str, min_resume_revision, spec_approval_id, spec_digest) = match input.stage {
        WorkflowBoardStageRunStage::Spec => ("spec", 1, None, None),
        WorkflowBoardStageRunStage::Plan => {
            let valid_spec = detail
                .approvals
                .iter()
                .find(|a| a.gate == "spec" && a.status == "valid")
                .ok_or_else(|| {
                    WorkflowBoardRequestError::new(
                        WorkflowBoardRequestErrorCode::SpecNotApproved,
                        "Cannot start Plan stage without a valid Spec approval",
                    )
                })?;
            let sd = valid_spec
                .spec_digest
                .clone()
                .unwrap_or_else(|| valid_spec.artifact_digest.clone());
            (
                "plan",
                valid_spec.revision,
                Some(valid_spec.id.clone()),
                Some(sd),
            )
        }
        _ => {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidStageTransition,
                "Unsupported stage",
            ));
        }
    };

    let reusable = find_reusable_stage_run(&conn, card_id, stage_str).map_err(map_repo_err)?;
    let cfg = match reusable {
        Some(ref sr) if is_resumable_stage_run(sr, min_resume_revision, input) => {
            resolve_resume_launch_config(&conn, detail, sr, input)?
        }
        _ => resolve_launch_config(&conn, detail, input, env.settings_path)?,
    };
    let next_attempt = next_stage_attempt(&conn, card_id, stage_str).map_err(map_repo_err)?;
    Ok(LaunchContext {
        stage: input.stage,
        planning,
        cfg,
        reusable,
        next_attempt,
        min_resume_revision,
        spec_approval_id,
        spec_digest,
        expected_rev_i64,
        now_iso,
    })
}

fn select_launch_key(
    input: &WorkflowBoardStartStageInput,
    ctx: &LaunchContext,
    card_id: &str,
) -> String {
    if let Some(ref key) = input.stable_launch_key {
        key.clone()
    } else if let Some(ref sr) = ctx.reusable
        && sr.card_revision >= ctx.min_resume_revision
    {
        sr.stable_launch_key.clone()
    } else {
        match ctx.stage {
            WorkflowBoardStageRunStage::Spec => format!(
                "board:{card_id}:spec:rev-{}:attempt-{}",
                ctx.planning.card_detail.card.revision, ctx.next_attempt
            ),
            _ => format!(
                "board:{card_id}:plan:{}:attempt-{}",
                ctx.spec_approval_id.as_deref().unwrap_or("spec"),
                ctx.next_attempt
            ),
        }
    }
}

async fn execute_fresh_stage_launch(
    env: &StageLaunchEnv<'_>,
    ctx: &mut LaunchContext,
    card_id: &str,
    stable_launch_key: &str,
    notify_project: &impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
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

    prepare_stage_workspace(env, ctx, stable_launch_key).await?;
    let detail = &ctx.planning.card_detail;
    let (stage_str, wf_name, wf_version, next_action, prep) = match ctx.stage {
        WorkflowBoardStageRunStage::Spec => (
            "spec",
            WORKFLOW_BOARD_SPEC_NAME,
            WORKFLOW_BOARD_SPEC_VERSION,
            "author_spec",
            prepare_spec_brief_inputs(env, &ctx.planning, &ctx.cfg)?,
        ),
        _ => (
            "plan",
            WORKFLOW_BOARD_PLAN_NAME,
            WORKFLOW_BOARD_PLAN_VERSION,
            "author_plan",
            prepare_plan_brief_inputs(
                env,
                &ctx.planning,
                &ctx.cfg,
                ctx.spec_digest.as_deref().unwrap_or(""),
            )?,
        ),
    };

    let stage_run_row = stage_brief_and_upsert_run(
        env,
        detail,
        &ctx.cfg,
        stage_str,
        None,
        &prep.brief_filename,
        &prep.card_dir,
        &prep.markdown,
        &prep.brief_digest,
        stable_launch_key,
        ctx.next_attempt,
        &ctx.now_iso,
    )?;

    if stage_run_row.thread_id.is_some()
        && !matches!(
            stage_run_row.status.as_str(),
            "failed" | "cancelled" | "completed"
        )
    {
        return resume_existing_stage_run(env, ctx.stage, detail, &stage_run_row, &ctx.cfg).await;
    }

    let payload = json!({
        "cardId": card_id,
        "projectId": detail.card.project_id,
        "title": detail.card.title,
        "briefMarkdown": prep.markdown,
        "briefPath": prep.brief_path,
        "specPath": prep.spec_path,
        "workspaceRoot": ctx.cfg.project.workspace_root,
        "worktreePath": detail.card.worktree_path,
        "modelSelection": ctx.cfg.model_selection,
        "runtimeMode": ctx.cfg.runtime_mode,
        "interactionMode": ctx.cfg.interaction_mode,
        "workspaceStrategy": ctx.cfg.workspace_strategy,
        "environmentId": env.environment_id,
        "stableLaunchKey": stable_launch_key,
    });
    let (wf_run_id, thread_id) = launch_fresh_workflow_run(
        env,
        &engine,
        wf_name,
        wf_version,
        stage_str,
        &stage_run_row.id,
        payload,
        stable_launch_key,
    )
    .await?;

    finalize_fresh_stage_run(
        env,
        ctx.stage,
        card_id,
        &detail.card.project_id,
        ctx.expected_rev_i64,
        next_action,
        &stage_run_row.id,
        &wf_run_id,
        &thread_id,
        notify_project,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Starts workflow run and verifies initial thread creation for the stage run"
)]
pub(crate) async fn launch_fresh_workflow_run(
    env: &StageLaunchEnv<'_>,
    engine: &std::sync::Arc<legion_workflows::WorkflowEngine>,
    wf_name: &str,
    wf_version: i64,
    stage_str: &str,
    stage_run_id: &str,
    payload: serde_json::Value,
    stable_launch_key: &str,
) -> Result<(String, String), WorkflowBoardRequestError> {
    let wf_run_id = match engine
        .start(wf_name, payload, stable_launch_key, None, Some(wf_version))
        .await
    {
        Ok(rid) => rid,
        Err(err) => {
            record_stage_run_failure(env, stage_run_id, &err.to_string());
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::LaunchFailed,
                err.to_string(),
            ));
        }
    };

    let launched_thread = engine.store().latest_thread(&wf_run_id).map_err(|e| {
        WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::LaunchFailed, e.to_string())
    })?;
    let Some(q_thread) = launched_thread else {
        let reason = engine
            .store()
            .run(&wf_run_id)
            .ok()
            .flatten()
            .and_then(|r| r.detail)
            .unwrap_or_else(|| format!("{stage_str} stage failed to launch thread"));
        let _ = engine.cancel(&wf_run_id, Some(reason.clone())).await;
        record_stage_run_failure(env, stage_run_id, &reason);
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::LaunchFailed,
            reason,
        ));
    };
    Ok((wf_run_id, q_thread.thread_id))
}
