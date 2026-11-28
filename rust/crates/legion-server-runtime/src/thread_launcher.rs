//! Shared worktree-aware thread launcher used by `orchestration.launchThread`,
//! `OrchestratorWorkflowAgentDriver`, and MCP `legion_thread_launch` (`R3`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use legion_contracts::ids::{ProjectId, ThreadId};
use legion_contracts::orchestration::{
    OrchestrationCommand, ThreadCreateCommand, WorkspaceStrategy,
};
use legion_orchestrator::Orchestrator;
use legion_orchestrator::projections::query_wire_thread_detail;
use legion_persistence::project_store::{ProjectRecord, find_project_by_id_or_workspace};
use serde_json::Value;

use crate::outbox_state::utc_now_iso;
use crate::thread_launcher_reuse::try_reuse_existing_conn;
use crate::thread_launcher_sync::{
    StableClaimStep, acquire_launch_key_lock, allocate_thread_id, handle_stable_claim,
    record_launch_failure, record_setup_committed, record_thread_committed,
};
use crate::thread_launcher_workspace::{
    prepare_workspace, validate_model_selection, validate_workspace_strategy,
};

pub use crate::thread_launcher_types::{LaunchThreadRequest, LaunchThreadResult};
pub use crate::thread_launcher_workspace::{
    ThreadLaunchError, parse_interaction_mode_strict, parse_runtime_mode_strict,
    resolve_default_model_selection, resolve_default_runtime_mode,
};

/// Shared launcher coordinating project lookup, crash-safe launch identity, worktree setup,
/// and `Orchestrator::dispatch_now(&OrchestrationCommand::ThreadCreate(...))`.
#[derive(Clone)]
pub struct ThreadLauncher {
    orchestrator: Arc<Orchestrator>,
    worktrees_dir: PathBuf,
    settings_path: Option<PathBuf>,
}

impl ThreadLauncher {
    /// Creates a launcher backed by `orchestrator` and `worktrees_dir`.
    pub fn new(orchestrator: Arc<Orchestrator>, worktrees_dir: PathBuf) -> Self {
        Self {
            orchestrator,
            worktrees_dir,
            settings_path: None,
        }
    }

    /// Attaches an optional `settings.json` path for explicit default resolution.
    #[must_use]
    pub fn with_settings_path(mut self, settings_path: Option<PathBuf>) -> Self {
        self.settings_path = settings_path;
        self
    }

    /// Returns the configured `settings.json` path, if any.
    pub fn settings_path(&self) -> Option<&Path> {
        self.settings_path.as_deref()
    }

    /// Strictly resolves a non-deleted project by ID or workspace path without fallback.
    pub fn resolve_project(&self, project_hint: &str) -> Result<ProjectRecord, ThreadLaunchError> {
        let trimmed = project_hint.trim();
        if trimmed.is_empty() {
            return Err(ThreadLaunchError::MissingProject);
        }
        let conn = self
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| ThreadLaunchError::Persistence("database lock poisoned".to_owned()))?;
        find_project_by_id_or_workspace(&*conn, trimmed)
            .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?
            .ok_or(ThreadLaunchError::ProjectNotFound)
    }

    /// Launches a new thread or idempotently resumes an existing thread matching
    /// `stable_launch_key` / `reuse_existing_thread` with strict project and worktree checks.
    pub async fn launch_thread(
        &self,
        req: LaunchThreadRequest,
    ) -> Result<LaunchThreadResult, ThreadLaunchError> {
        validate_model_selection(&req.model_selection)?;
        validate_workspace_strategy(&req.workspace_strategy)?;
        let project = self.resolve_project(&req.project_id)?;
        let canonical_project_id = ProjectId::new(&project.id)
            .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;
        let now_iso = utc_now_iso();
        let stable_key = req
            .stable_launch_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        let _launch_guard = match stable_key.as_deref() {
            Some(key) => Some(acquire_launch_key_lock(&self.orchestrator, key).await),
            None => None,
        };

        let (thread_id, recovered_workspace) = if let Some(ref key) = stable_key {
            match handle_stable_claim(
                &self.orchestrator,
                key,
                &req,
                &canonical_project_id,
                &now_iso,
            )? {
                StableClaimStep::Done(result) => return Ok(*result),
                StableClaimStep::Continue {
                    thread_id,
                    recovered_workspace,
                } => (thread_id, recovered_workspace),
            }
        } else {
            let tid = allocate_thread_id(req.thread_id.as_ref())?;
            if req.reuse_existing_thread
                && let Some(resumed) = self.try_reuse_existing(
                    tid.as_str(),
                    &canonical_project_id,
                    &req.workspace_strategy,
                    None,
                )?
            {
                return Ok(resumed);
            }
            (tid, None)
        };

        let (branch, worktree_path) = self
            .resolve_or_prepare_workspace(
                recovered_workspace,
                stable_key.as_deref(),
                &canonical_project_id,
                &project.workspace_root,
                &req.workspace_strategy,
            )
            .await?;

        self.dispatch_and_finalize_thread(
            req,
            canonical_project_id,
            thread_id,
            branch,
            worktree_path,
            stable_key.as_deref(),
        )
        .await
    }

    /// Reserves the durable launch workspace before callers assemble workspace-specific inputs.
    pub(crate) async fn prepare_launch_workspace(
        &self,
        req: &LaunchThreadRequest,
    ) -> Result<(Option<String>, Option<String>), ThreadLaunchError> {
        validate_model_selection(&req.model_selection)?;
        validate_workspace_strategy(&req.workspace_strategy)?;
        let project = self.resolve_project(&req.project_id)?;
        let project_id = ProjectId::new(&project.id)
            .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?;
        let key = req
            .stable_launch_key
            .as_deref()
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                ThreadLaunchError::Persistence(
                    "Workspace preparation requires a stable launch key".to_owned(),
                )
            })?;
        let _guard = acquire_launch_key_lock(&self.orchestrator, key).await;
        let recovered_workspace =
            match handle_stable_claim(&self.orchestrator, key, req, &project_id, &utc_now_iso())? {
                StableClaimStep::Done(result) => return Ok((result.branch, result.worktree_path)),
                StableClaimStep::Continue {
                    recovered_workspace,
                    ..
                } => recovered_workspace,
            };
        let (mut branch, worktree_path) = self
            .resolve_or_prepare_workspace(
                recovered_workspace,
                Some(key),
                &project_id,
                &project.workspace_root,
                &req.workspace_strategy,
            )
            .await?;
        if branch.is_none()
            && let Some(ref worktree) = worktree_path
        {
            branch = crate::thread_launcher_workspace::read_worktree_branch(worktree).await?;
            record_setup_committed(&self.orchestrator, key, Some(worktree), branch.as_deref())?;
        }
        Ok((branch, worktree_path))
    }

    async fn dispatch_and_finalize_thread(
        &self,
        req: LaunchThreadRequest,
        canonical_project_id: ProjectId,
        thread_id: ThreadId,
        branch: Option<String>,
        worktree_path: Option<String>,
        stable_key: Option<&str>,
    ) -> Result<LaunchThreadResult, ThreadLaunchError> {
        let has_initial_message = req.initial_message.is_some();
        let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
            created_by: req.created_by,
            creation_source: req.creation_source,
            command_id: req.command_id,
            thread_id: thread_id.clone(),
            project_id: canonical_project_id,
            title: req.title,
            generate_title: req.generate_title,
            model_selection: req.model_selection,
            runtime_mode: req.runtime_mode,
            interaction_mode: req.interaction_mode,
            branch: branch.clone(),
            worktree_path: worktree_path.clone(),
            link_origin: req.link_origin,
            delegated_from: req.delegated_from,
            imported_native_thread: None,
            initial_message: req.initial_message,
            title_renamed: req.title_renamed,
            workflow_run: req.workflow_run,
        });

        let committed = match self.orchestrator.dispatch_now(&cmd).await {
            Ok(committed) => committed,
            Err(err) => {
                if let Some(key) = stable_key {
                    record_launch_failure(&self.orchestrator, key, &err.to_string());
                }
                return Err(ThreadLaunchError::Orchestration(err.to_string()));
            }
        };

        if let Some(key) = stable_key {
            record_thread_committed(&self.orchestrator, key, has_initial_message)?;
        }

        let detail = {
            let conn =
                self.orchestrator.sink().db().lock().map_err(|_| {
                    ThreadLaunchError::Persistence("database lock poisoned".to_owned())
                })?;
            query_wire_thread_detail(&conn, thread_id.as_str())
                .map_err(|e| ThreadLaunchError::Persistence(e.to_string()))?
        };
        let run_id = detail
            .projection
            .runs
            .last()
            .and_then(|r| r.get("id").and_then(Value::as_str))
            .map(str::to_owned);
        Ok(LaunchThreadResult {
            thread_id,
            run_id,
            projection: detail.projection,
            resumed: false,
            sequence: committed.outcome.receipt.result_sequence,
            branch,
            worktree_path,
        })
    }

    async fn resolve_or_prepare_workspace(
        &self,
        recovered_workspace: Option<(Option<String>, Option<String>)>,
        stable_key: Option<&str>,
        canonical_project_id: &ProjectId,
        workspace_root: &str,
        workspace_strategy: &WorkspaceStrategy,
    ) -> Result<(Option<String>, Option<String>), ThreadLaunchError> {
        if let Some(pair) = recovered_workspace {
            return Ok(pair);
        }
        let prepared = match prepare_workspace(
            &self.worktrees_dir,
            canonical_project_id,
            workspace_root,
            workspace_strategy,
        )
        .await
        {
            Ok(pair) => pair,
            Err(err) => {
                if let Some(key) = stable_key {
                    record_launch_failure(&self.orchestrator, key, &err.to_string());
                }
                return Err(err);
            }
        };
        if let Some(key) = stable_key {
            record_setup_committed(
                &self.orchestrator,
                key,
                prepared.1.as_deref(),
                prepared.0.as_deref(),
            )?;
        }
        Ok(prepared)
    }

    fn try_reuse_existing(
        &self,
        thread_id: &str,
        canonical_project_id: &ProjectId,
        strategy: &WorkspaceStrategy,
        expected_worktree_path: Option<&str>,
    ) -> Result<Option<LaunchThreadResult>, ThreadLaunchError> {
        let conn = self
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| ThreadLaunchError::Persistence("database lock poisoned".to_owned()))?;
        try_reuse_existing_conn(
            &conn,
            thread_id,
            canonical_project_id,
            strategy,
            expected_worktree_path,
        )
    }
}
