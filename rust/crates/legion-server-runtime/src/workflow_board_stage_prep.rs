#![forbid(unsafe_code)]
//! Launch configuration resolution and stage-run metadata helpers for board stage jobs.

use std::path::PathBuf;
use std::sync::Arc;

use legion_contracts::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::project_store::{ProjectRecord, find_project_by_id_or_workspace};
use legion_persistence::workflow_board_repo::link_stage_run_thread;
use legion_persistence::workflow_board_repo::types::{
    CardDetail, LinkStageRunThreadInput, StageRunRow,
};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::outbox_state::utc_now_iso;
use crate::thread_launcher::{resolve_default_model_selection, resolve_default_runtime_mode};
use crate::workflow_board_types::{
    map_launch_err, parse_opt_interaction_mode, parse_opt_model_selection, parse_opt_runtime_mode,
    parse_stage_run_model_selection, parse_workspace_strategy,
};

pub(crate) struct StageLaunchEnv<'a> {
    pub orchestrator: &'a Arc<Orchestrator>,
    pub worktrees_dir: &'a PathBuf,
    pub settings_path: Option<&'a PathBuf>,
    pub environment_id: &'a str,
    pub workflow_manager: Option<&'a Arc<WorkflowDefinitionsManager>>,
}

pub(crate) struct ResolvedLaunchConfig {
    pub project: ProjectRecord,
    pub model_selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub workspace_strategy: WorkspaceStrategy,
}

fn load_project_for_card(
    conn: &Connection,
    project_id: &str,
) -> Result<ProjectRecord, WorkflowBoardRequestError> {
    find_project_by_id_or_workspace(conn, project_id)
        .map_err(|e| internal_err(&e.to_string()))?
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ProjectNotFound,
                format!("Project `{project_id}` was not found"),
            )
        })
}

pub(crate) fn resolve_launch_config(
    conn: &Connection,
    detail: &CardDetail,
    input: &WorkflowBoardStartStageInput,
    settings_path: Option<&PathBuf>,
) -> Result<ResolvedLaunchConfig, WorkflowBoardRequestError> {
    let project = load_project_for_card(conn, &detail.card.project_id)?;
    let settings_p = settings_path.map(PathBuf::as_path);
    let model_selection = if let Some(ref sel) = input.model_selection {
        sel.clone()
    } else if let Some(sel) =
        parse_opt_model_selection(detail.card.model_selection_json.as_deref())?
    {
        sel
    } else {
        resolve_default_model_selection(&project, settings_p).map_err(map_launch_err)?
    };
    let runtime_mode = if let Some(rt) = input.runtime_mode {
        rt
    } else if let Some(rt) = parse_opt_runtime_mode(detail.card.runtime_mode.as_deref())? {
        rt
    } else {
        resolve_default_runtime_mode(settings_p).map_err(map_launch_err)?
    };
    let interaction_mode = if let Some(im) = input.interaction_mode {
        im
    } else {
        parse_opt_interaction_mode(detail.card.interaction_mode.as_deref())?
            .unwrap_or(ProviderInteractionMode::Default)
    };
    let raw_strategy = match input.workspace_strategy {
        Some(ref ws) => ws.clone(),
        None => parse_workspace_strategy(&detail.card.workspace_strategy_json)?,
    };
    let workspace_strategy = bind_card_worktree_strategy(&detail.card, raw_strategy, input.stage)?;

    Ok(ResolvedLaunchConfig {
        project,
        model_selection,
        runtime_mode,
        interaction_mode,
        workspace_strategy,
    })
}

pub(crate) fn resolve_resume_launch_config(
    conn: &Connection,
    detail: &CardDetail,
    sr: &StageRunRow,
    input: &WorkflowBoardStartStageInput,
) -> Result<ResolvedLaunchConfig, WorkflowBoardRequestError> {
    let project = load_project_for_card(conn, &detail.card.project_id)?;
    let model_selection = match input.model_selection {
        Some(ref sel) => sel.clone(),
        None => parse_stage_run_model_selection(sr)?,
    };
    let runtime_mode = match input.runtime_mode {
        Some(rt) => rt,
        None => parse_opt_runtime_mode(Some(&sr.runtime_mode))?
            .ok_or_else(|| internal_err("Persisted stage run runtime_mode is empty"))?,
    };
    let interaction_mode = match input.interaction_mode {
        Some(im) => im,
        None => parse_opt_interaction_mode(Some(&sr.interaction_mode))?
            .ok_or_else(|| internal_err("Persisted stage run interaction_mode is empty"))?,
    };
    let raw_strategy = match input.workspace_strategy {
        Some(ref ws) => ws.clone(),
        None => parse_workspace_strategy(&sr.workspace_strategy_json)?,
    };
    let workspace_strategy = bind_card_worktree_strategy(&detail.card, raw_strategy, input.stage)?;
    Ok(ResolvedLaunchConfig {
        project,
        model_selection,
        runtime_mode,
        interaction_mode,
        workspace_strategy,
    })
}

pub(crate) fn query_thread_workspace(
    conn: &Connection,
    thread_id: &str,
) -> (Option<String>, Option<String>) {
    conn.query_row(
        "SELECT json_extract(payload_json, '$.worktreePath'),
                json_extract(payload_json, '$.branch')
         FROM orchestration_v2_projection_threads
         WHERE thread_id = ?1 LIMIT 1",
        params![thread_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or((None, None))
}

pub(crate) fn record_stage_run_failure(env: &StageLaunchEnv<'_>, stage_run_id: &str, reason: &str) {
    if let Ok(conn) = env.orchestrator.sink().db().lock() {
        let now = utc_now_iso();
        let _ = link_stage_run_thread(
            &conn,
            &LinkStageRunThreadInput {
                id: stage_run_id,
                status: "failed",
                thread_id: None,
                workflow_run_id: None,
                worktree_path: None,
                branch: None,
                last_error: Some(reason),
                ended_at: Some(&now),
            },
            &now,
        );
    }
}

pub(crate) fn internal_err(msg: &str) -> WorkflowBoardRequestError {
    WorkflowBoardRequestError::new(WorkflowBoardRequestErrorCode::InternalError, msg)
}

/// Plan resumes and new worktree requests keep the card's materialized workspace.
pub(crate) fn bind_card_worktree_strategy(
    card: &legion_persistence::workflow_board_repo::types::CardRow,
    strategy: WorkspaceStrategy,
    stage: WorkflowBoardStageRunStage,
) -> Result<WorkspaceStrategy, WorkflowBoardRequestError> {
    if let Some(wt) = card
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match strategy {
            WorkspaceStrategy::Worktree { branch, .. } => {
                return Ok(WorkspaceStrategy::ExistingWorktree {
                    worktree_path: wt.to_owned(),
                    branch: card.branch.clone().or(branch),
                });
            }
            WorkspaceStrategy::Root { .. } if stage == WorkflowBoardStageRunStage::Plan => {
                return Ok(WorkspaceStrategy::ExistingWorktree {
                    worktree_path: wt.to_owned(), branch: card.branch.clone(),
                });
            }
            WorkspaceStrategy::ExistingWorktree { ref worktree_path, .. }
                if stage == WorkflowBoardStageRunStage::Plan
                    && legion_persistence::worktree_lookup::normalize_project_path_for_comparison(worktree_path)
                        != legion_persistence::worktree_lookup::normalize_project_path_for_comparison(wt) => {
                return Err(WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::ContractConflict,
                    format!("Plan must reuse card worktree `{wt}`; requested `{worktree_path}`"),
                ));
            }
            _ => {}
        }
    }
    Ok(strategy)
}
