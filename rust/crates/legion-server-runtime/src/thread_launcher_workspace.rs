//! Workspace preparation and strict mode/model validation
//! for [`ThreadLauncher`](crate::thread_launcher::ThreadLauncher).

use std::fs;
use std::path::Path;
use std::process::Stdio;

use legion_contracts::ids::ProjectId;
use legion_contracts::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_persistence::project_store::ProjectRecord;
use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;
use uuid::Uuid;

/// Errors returned by [`ThreadLauncher`](crate::thread_launcher::ThreadLauncher).
#[derive(Debug, Clone, Error)]
pub enum ThreadLaunchError {
    /// Missing or empty project hint.
    #[error("Missing required project.")]
    MissingProject,
    /// Project was not found or was soft-deleted.
    #[error("Project was not found.")]
    ProjectNotFound,
    /// Missing or invalid provider/model selection.
    #[error("Invalid model selection: {0}")]
    InvalidModelSelection(String),
    /// Missing or invalid runtime permission mode.
    #[error("Invalid runtime mode: {0}")]
    InvalidRuntimeMode(String),
    /// Invalid provider interaction mode.
    #[error("Invalid interaction mode: {0}")]
    InvalidInteractionMode(String),
    /// Invalid workspace launch strategy.
    #[error("Invalid workspace strategy: {0}")]
    InvalidWorkspaceStrategy(String),
    /// Existing worktree directory does not exist on disk.
    #[error("Existing worktree path does not exist: {0}")]
    WorktreeNotFound(String),
    /// `git worktree add` failed.
    #[error("Git worktree creation failed: {0}")]
    GitWorktreeFailed(String),
    /// Stable launch key or thread reuse conflict.
    #[error("Launch reuse conflict: {0}")]
    ReuseConflict(String),
    /// Target thread was soft-deleted and cannot be reused.
    #[error("Thread `{0}` was deleted and cannot be reused.")]
    ThreadDeleted(String),
    /// Orchestrator command dispatch failed.
    #[error("{0}")]
    Orchestration(String),
    /// SQLite persistence or lock failure.
    #[error("{0}")]
    Persistence(String),
}

/// Validates `model_selection` has non-empty provider instance and model name.
pub fn validate_model_selection(sel: &ModelSelection) -> Result<(), ThreadLaunchError> {
    if sel.instance_id.as_str().trim().is_empty() {
        return Err(ThreadLaunchError::InvalidModelSelection(
            "instanceId cannot be empty".to_owned(),
        ));
    }
    if sel.model.trim().is_empty() {
        return Err(ThreadLaunchError::InvalidModelSelection(
            "model cannot be empty".to_owned(),
        ));
    }
    Ok(())
}

/// Parses a `RuntimeMode` string strictly without silent fallback.
pub fn parse_runtime_mode_strict(raw: &str) -> Result<RuntimeMode, ThreadLaunchError> {
    match raw.trim() {
        "approval-required" => Ok(RuntimeMode::ApprovalRequired),
        "auto-accept-edits" => Ok(RuntimeMode::AutoAcceptEdits),
        "auto" => Ok(RuntimeMode::Auto),
        "full-access" => Ok(RuntimeMode::FullAccess),
        other => Err(ThreadLaunchError::InvalidRuntimeMode(format!(
            "unsupported runtimeMode `{other}`"
        ))),
    }
}

/// Parses a `ProviderInteractionMode` string strictly without silent fallback.
pub fn parse_interaction_mode_strict(
    raw: &str,
) -> Result<ProviderInteractionMode, ThreadLaunchError> {
    match raw.trim() {
        "default" => Ok(ProviderInteractionMode::Default),
        "plan" => Ok(ProviderInteractionMode::Plan),
        other => Err(ThreadLaunchError::InvalidInteractionMode(format!(
            "unsupported interactionMode `{other}`"
        ))),
    }
}

fn read_disk_settings_json(
    settings_path: Option<&Path>,
) -> Result<Option<Value>, ThreadLaunchError> {
    let Some(path) = settings_path else {
        return Ok(None);
    };
    match fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => serde_json::from_str::<Value>(&raw)
            .map(Some)
            .map_err(|e| ThreadLaunchError::Persistence(format!("Invalid settings.json: {e}"))),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ThreadLaunchError::Persistence(err.to_string())),
    }
}

/// Resolves an explicitly configured `defaultModelSelection` from `project` or `settings.json` on disk.
pub fn resolve_default_model_selection(
    project: &ProjectRecord,
    settings_path: Option<&Path>,
) -> Result<ModelSelection, ThreadLaunchError> {
    if let Some(raw) = project
        .default_model_selection_json
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let parsed = serde_json::from_str::<ModelSelection>(raw).map_err(|e| {
            ThreadLaunchError::InvalidModelSelection(format!(
                "invalid project defaultModelSelection: {e}"
            ))
        })?;
        validate_model_selection(&parsed)?;
        return Ok(parsed);
    }
    if let Some(settings) = read_disk_settings_json(settings_path)?
        && let Some(val) = settings.get("defaultModelSelection")
        && !val.is_null()
    {
        let parsed = serde_json::from_value::<ModelSelection>(val.clone()).map_err(|e| {
            ThreadLaunchError::InvalidModelSelection(format!(
                "invalid settings defaultModelSelection: {e}"
            ))
        })?;
        validate_model_selection(&parsed)?;
        return Ok(parsed);
    }
    Err(ThreadLaunchError::InvalidModelSelection(
        "Missing required modelSelection (no project or server defaultModelSelection configured)."
            .to_owned(),
    ))
}

/// Resolves an explicitly configured `defaultRuntimeMode` from `settings.json` on disk.
pub fn resolve_default_runtime_mode(
    settings_path: Option<&Path>,
) -> Result<RuntimeMode, ThreadLaunchError> {
    if let Some(settings) = read_disk_settings_json(settings_path)?
        && let Some(raw) = settings.get("defaultRuntimeMode").and_then(Value::as_str)
    {
        return parse_runtime_mode_strict(raw);
    }
    Err(ThreadLaunchError::InvalidRuntimeMode(
        "Missing required runtimeMode (no defaultRuntimeMode configured in settings.json)."
            .to_owned(),
    ))
}

/// Validates a [`WorkspaceStrategy`] before any filesystem or DB mutation.
pub fn validate_workspace_strategy(strategy: &WorkspaceStrategy) -> Result<(), ThreadLaunchError> {
    match strategy {
        WorkspaceStrategy::Root { branch } => {
            if branch.as_deref().is_some_and(|b| b.trim().is_empty()) {
                return Err(ThreadLaunchError::InvalidWorkspaceStrategy(
                    "branch cannot be empty".to_owned(),
                ));
            }
            Ok(())
        }
        WorkspaceStrategy::ExistingWorktree {
            worktree_path,
            branch,
        } => {
            let trimmed = worktree_path.trim();
            if trimmed.is_empty() {
                return Err(ThreadLaunchError::InvalidWorkspaceStrategy(
                    "worktreePath cannot be empty".to_owned(),
                ));
            }
            if branch.as_deref().is_some_and(|b| b.trim().is_empty()) {
                return Err(ThreadLaunchError::InvalidWorkspaceStrategy(
                    "branch cannot be empty".to_owned(),
                ));
            }
            if !Path::new(trimmed).is_dir() {
                return Err(ThreadLaunchError::WorktreeNotFound(trimmed.to_owned()));
            }
            Ok(())
        }
        WorkspaceStrategy::Worktree {
            base_ref, branch, ..
        } => {
            if base_ref.trim().is_empty() {
                return Err(ThreadLaunchError::InvalidWorkspaceStrategy(
                    "baseRef cannot be empty".to_owned(),
                ));
            }
            if branch.as_deref().is_some_and(|b| b.trim().is_empty()) {
                return Err(ThreadLaunchError::InvalidWorkspaceStrategy(
                    "branch cannot be empty".to_owned(),
                ));
            }
            Ok(())
        }
    }
}

pub(crate) async fn read_worktree_branch(
    worktree_path: &str,
) -> Result<Option<String>, ThreadLaunchError> {
    let output = Command::new("git")
        .current_dir(worktree_path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["branch", "--show-current"])
        .output()
        .await
        .map_err(|e| ThreadLaunchError::GitWorktreeFailed(e.to_string()))?;
    if !output.status.success() {
        return Err(ThreadLaunchError::GitWorktreeFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!branch.is_empty()).then_some(branch))
}

/// Prepares `(branch, worktree_path)` for `strategy`, running `git worktree add` for `Worktree`.
pub async fn prepare_workspace(
    worktrees_dir: &Path,
    project_id: &ProjectId,
    workspace_root: &str,
    strategy: &WorkspaceStrategy,
) -> Result<(Option<String>, Option<String>), ThreadLaunchError> {
    validate_workspace_strategy(strategy)?;
    match strategy {
        WorkspaceStrategy::Root { branch } => {
            Ok((branch.as_deref().map(|b| b.trim().to_owned()), None))
        }
        WorkspaceStrategy::ExistingWorktree {
            worktree_path,
            branch,
        } => Ok((
            branch.as_deref().map(|b| b.trim().to_owned()),
            Some(worktree_path.trim().to_owned()),
        )),
        WorkspaceStrategy::Worktree {
            base_ref, branch, ..
        } => {
            let short = &Uuid::new_v4().simple().to_string()[..8];
            let branch_name = branch
                .as_deref()
                .map_or_else(|| format!("legion/{short}"), |b| b.trim().to_owned());
            let wt_dir = worktrees_dir
                .join(project_id.as_str().replace(':', "_"))
                .join(short);
            if let Some(parent) = wt_dir.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| ThreadLaunchError::GitWorktreeFailed(e.to_string()))?;
            }
            let wt_str = wt_dir.to_string_lossy().into_owned();
            let out = Command::new("git")
                .current_dir(Path::new(workspace_root))
                .env("GIT_TERMINAL_PROMPT", "0")
                .args([
                    "worktree",
                    "add",
                    "-b",
                    &branch_name,
                    &wt_str,
                    base_ref.trim(),
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| {
                    ThreadLaunchError::GitWorktreeFailed(format!(
                        "failed to invoke git worktree add: {e}"
                    ))
                })?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                return Err(ThreadLaunchError::GitWorktreeFailed(if stderr.is_empty() {
                    "git worktree add failed".to_owned()
                } else {
                    stderr
                }));
            }
            Ok((Some(branch_name), Some(wt_str)))
        }
    }
}
