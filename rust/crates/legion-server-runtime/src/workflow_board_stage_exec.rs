#![forbid(unsafe_code)]
//! Stage run completion linking and idempotent thread resume helpers.

use std::sync::Arc;

use legion_contracts::ids::{CommandId, RunId, ThreadId, WorkflowBoardStageRunId, WorkflowCardId};
use legion_contracts::orchestration::{CreationSource, OrchestrationActor};
use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardStageRunStage, WorkflowBoardStartStageResult,
};
use legion_persistence::workflow_board_repo::types::{
    CardDetail, LinkStageRunThreadInput, StageRunRow,
};
use legion_persistence::workflow_board_repo::{
    link_stage_run_thread, record_stage_started_cas, set_card_worktree_if_unset,
};
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::thread_launcher::{LaunchThreadRequest, ThreadLauncher};
use crate::workflow_board_stage_prep::{
    ResolvedLaunchConfig, StageLaunchEnv, internal_err, query_thread_workspace,
};
use crate::workflow_board_types::{map_launch_err, map_repo_err};

#[expect(
    clippy::too_many_arguments,
    reason = "Links thread and advances card revision inside a single lock scope"
)]
pub(crate) fn finalize_fresh_stage_run(
    env: &StageLaunchEnv<'_>,
    stage: WorkflowBoardStageRunStage,
    card_id: &str,
    project_id: &str,
    expected_rev_i64: i64,
    next_action: &str,
    stage_run_id: &str,
    wf_run_id: &str,
    thread_id_str: &str,
    notify_project: &impl Fn(&str),
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let stage_label = match stage {
        WorkflowBoardStageRunStage::Spec => "Spec",
        WorkflowBoardStageRunStage::Plan => "Plan",
        WorkflowBoardStageRunStage::Execute => "Execute",
        WorkflowBoardStageRunStage::Review => "Review",
        WorkflowBoardStageRunStage::Ship => "Ship",
    };
    {
        let conn = env
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| internal_err("Database lock poisoned"))?;
        let (wt_path, branch) = query_thread_workspace(&conn, thread_id_str);
        let done_iso = utc_now_iso();
        if let Some(ref wt) = wt_path
            && !wt.trim().is_empty()
        {
            if let Ok(before) =
                legion_persistence::workflow_board_repo::get_card_detail(&conn, card_id)
                && before.card.worktree_path.is_none()
                && let Ok(prev_ws) =
                    crate::workflow_board_fs::resolve_card_workspace_root(&conn, &before.card)
                && let Ok(src_dir) =
                    crate::workflow_board_fs::ensure_confined_card_dir(&prev_ws, card_id)
            {
                crate::workflow_board_stage_m3_preflight::copy_card_dir_if_new_worktree(
                    &src_dir,
                    std::path::Path::new(wt),
                    card_id,
                );
            }
            let _ = set_card_worktree_if_unset(&conn, card_id, wt, branch.as_deref(), &done_iso);
        }
        link_stage_run_thread(
            &conn,
            &LinkStageRunThreadInput {
                id: stage_run_id,
                status: "running",
                thread_id: Some(thread_id_str),
                workflow_run_id: Some(wf_run_id),
                worktree_path: wt_path.as_deref(),
                branch: branch.as_deref(),
                last_error: None,
                ended_at: None,
            },
            &done_iso,
        )
        .map_err(map_repo_err)?;
        record_stage_started_cas(
            &conn,
            card_id,
            expected_rev_i64,
            next_action,
            "client",
            &format!("Started {stage_label} stage ({thread_id_str})"),
            &done_iso,
        )
        .map_err(map_repo_err)?;
    }
    notify_project(project_id);

    let latest_rev = {
        let conn = env
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| internal_err("Database lock poisoned"))?;
        legion_persistence::workflow_board_repo::get_card_detail(&conn, card_id)
            .map_err(map_repo_err)?
            .card
            .revision
    };

    Ok(WorkflowBoardStartStageResult {
        card_id: WorkflowCardId::new(card_id).map_err(|e| internal_err(&e.to_string()))?,
        revision: u64::try_from(latest_rev.max(1)).unwrap_or(1),
        stage,
        task_id: None,
        stage_run_id: WorkflowBoardStageRunId::new(stage_run_id)
            .map_err(|e| internal_err(&e.to_string()))?,
        run_id: Some(RunId::new(wf_run_id).map_err(|e| internal_err(&e.to_string()))?),
        thread_id: ThreadId::new(thread_id_str).map_err(|e| internal_err(&e.to_string()))?,
        resumed: false,
    })
}

pub(crate) async fn resume_existing_stage_run(
    env: &StageLaunchEnv<'_>,
    stage: WorkflowBoardStageRunStage,
    detail: &CardDetail,
    sr: &StageRunRow,
    cfg: &ResolvedLaunchConfig,
) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
    let prefix = match stage {
        WorkflowBoardStageRunStage::Spec => "Spec",
        WorkflowBoardStageRunStage::Plan => "Plan",
        WorkflowBoardStageRunStage::Execute => "Execute",
        WorkflowBoardStageRunStage::Review => "Review",
        WorkflowBoardStageRunStage::Ship => "Ship",
    };
    let thread_launcher =
        ThreadLauncher::new(Arc::clone(env.orchestrator), env.worktrees_dir.clone())
            .with_settings_path(env.settings_path.cloned());
    let cmd_id = CommandId::new(format!("board-resume-{}", Uuid::new_v4()))
        .map_err(|e| internal_err(&e.to_string()))?;
    let launch_res = thread_launcher
        .launch_thread(LaunchThreadRequest {
            command_id: cmd_id,
            created_by: OrchestrationActor::User,
            creation_source: CreationSource::Web,
            thread_id: None,
            stable_launch_key: Some(sr.stable_launch_key.clone()),
            reuse_existing_thread: true,
            project_id: detail.card.project_id.clone(),
            title: format!("{prefix}: {}", detail.card.title),
            title_renamed: Some(true),
            generate_title: Some(false),
            model_selection: cfg.model_selection.clone(),
            runtime_mode: cfg.runtime_mode,
            interaction_mode: cfg.interaction_mode,
            workspace_strategy: cfg.workspace_strategy.clone(),
            workflow_run: None,
            link_origin: None,
            delegated_from: None,
            initial_message: None,
        })
        .await
        .map_err(map_launch_err)?;

    let run_id = sr
        .workflow_run_id
        .as_deref()
        .map(RunId::new)
        .transpose()
        .map_err(|e| internal_err(&e.to_string()))?;
    Ok(WorkflowBoardStartStageResult {
        card_id: WorkflowCardId::new(&detail.card.card_id)
            .map_err(|e| internal_err(&e.to_string()))?,
        revision: u64::try_from(detail.card.revision.max(1)).unwrap_or(1),
        stage,
        task_id: sr.task_id.clone(),
        stage_run_id: WorkflowBoardStageRunId::new(&sr.id)
            .map_err(|e| internal_err(&e.to_string()))?,
        run_id,
        thread_id: launch_res.thread_id,
        resumed: true,
    })
}
