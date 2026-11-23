//! Worktree/checkout queue unblocking (`#41052`), shared-folder capture timeout tracking
//! (`#40411`), post-delete rollback suppression (`#41113`), cancellation preservation during
//! capture (`#31349`), error-checkpoint rewind eligibility (`#40745`), and single-walk repo
//! scope collection (`#40671`).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use legion_contracts::ids::{CheckpointId, RunAttemptId};
use legion_contracts::orchestration::{
    AppThread, CheckpointScope, Run, RunStatus, WorkspaceStrategy,
};
use serde::{Deserialize, Serialize};

/// SQL predicate preserving `"missing"` and `"error"` run checkpoints alongside `"ready"`
/// snapshots for rollback context (`#40745`, `ProjectionStore.ts:6379`).
pub const PRESERVED_CHECKPOINT_SQL_FILTER: &str =
    "(status = 'ready' OR (status IN ('missing', 'error') AND run_id IS NOT NULL))";

/// Returns `true` when queued messages must remain held waiting for worktree setup (`#41052`).
///
/// Only runs that failed or were interrupted BEFORE ever starting (`started_at.is_none()`) while
/// preparing a worktree indicate missing setup; once a worktree thread has started and later moved
/// back to the project checkout (`worktree_path.is_none()`), queued messages are unblocked.
#[must_use]
pub fn worktree_setup_pending(thread: &AppThread, runs: &[Run]) -> bool {
    thread.worktree_path.is_none()
        && runs.iter().any(|run| {
            matches!(run.status, RunStatus::Failed | RunStatus::Interrupted)
                && run.started_at.is_none()
                && matches!(
                    run.workspace_preparation,
                    Some(WorkspaceStrategy::Worktree { .. })
                )
        })
}

/// Returns `true` when `stored_scope` belongs to the thread's current `execution_cwd` (`#41052`).
#[must_use]
pub fn should_reuse_checkpoint_scope(
    stored_scope: Option<&CheckpointScope>,
    execution_cwd: &str,
) -> bool {
    stored_scope.is_some_and(|scope| scope.cwd == execution_cwd)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FolderCaptureState {
    count: u64,
    timed_out: bool,
}

/// Per-folder monotonic capture outcome counter used so queued threads sharing a workspace folder
/// do not wait out back-to-back checkpoint timeouts (`#40411`, `CheckpointService.ts`).
#[derive(Debug, Default)]
pub struct CheckpointCaptureTracker {
    folders: Mutex<HashMap<String, FolderCaptureState>>,
}

impl CheckpointCaptureTracker {
    /// Creates an empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns how many checkpoint captures have finished in `cwd` so far.
    #[must_use]
    pub fn captures_so_far(&self, cwd: &str) -> u64 {
        self.folders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(cwd)
            .map_or(0, |entry| entry.count)
    }

    /// Records a capture completion; baseline budget timeouts do not poison post-turn captures.
    pub fn note_capture(&self, cwd: &str, post_turn_timed_out: bool) {
        let mut guard = self.folders.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard.entry(cwd.to_owned()).or_default();
        entry.count = entry.count.saturating_add(1);
        entry.timed_out = post_turn_timed_out;
    }

    /// Returns `true` when this non-clean-head capture waited on the folder lock while another
    /// capture in the same `cwd` finished with a timeout (`#40411`).
    #[must_use]
    pub fn should_skip_after_peer_timeout(
        &self,
        cwd: &str,
        captures_before_lock: u64,
        has_clean_head: bool,
    ) -> bool {
        if has_clean_head {
            return false;
        }
        let guard = self.folders.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .get(cwd)
            .is_some_and(|last| last.timed_out && last.count > captures_before_lock)
    }
}

/// Returns `true` when an async queued `checkpoint.rollback` job must abort because the thread was
/// deleted before acquiring execution lock (`#41113`).
#[must_use]
pub fn should_skip_pending_rollback(thread: &AppThread) -> bool {
    thread.deleted_at.is_some()
}

/// Decision produced under the thread lock when finalizing an async checkpoint capture (`#31349`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedCaptureRun {
    /// Updated run preserving `Interrupted` / `Cancelled` if stopped while git captured.
    pub run: Run,
    /// Only `true` when the run was still `Waiting` and was transitioned to `Completed`.
    pub complete_root_node: bool,
}

/// Re-evaluates `current_run` under thread lock before committing `checkpoint.capture` (`#31349`).
///
/// Preserves `Interrupted` and `Cancelled` statuses set while git captured files, attaching only
/// `checkpoint_id` and never falsely marking `root_node` completed.
#[must_use]
pub fn plan_checkpoint_capture_finalization(
    current_run: Option<&Run>,
    captured_active_attempt_id: Option<&RunAttemptId>,
    checkpoint_id: &CheckpointId,
    captured_at: &str,
) -> Option<FinalizedCaptureRun> {
    let run = current_run?;
    if run.active_attempt_id.as_ref() != captured_active_attempt_id {
        return None;
    }
    let stopped_now = matches!(run.status, RunStatus::Interrupted | RunStatus::Cancelled);
    if run.status != RunStatus::Waiting && !stopped_now {
        return None;
    }
    let mut updated = run.clone();
    updated.checkpoint_id = Some(checkpoint_id.clone());
    if !stopped_now {
        updated.status = RunStatus::Completed;
        updated.completed_at = Some(captured_at.to_owned());
    }
    Some(FinalizedCaptureRun {
        run: updated,
        complete_root_node: !stopped_now,
    })
}

/// Checkpoint status values recognized during rollback validation and timeline targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointTargetStatus {
    /// Full git ref + conversation snapshot ready.
    Ready,
    /// Snapshot git ref missing (e.g. clean tree or unsupported VCS); valid for conversation rewind.
    Missing,
    /// Capture failed/timed out; remains valid for conversation rewind ("Edit from here", `#40745`).
    Error,
    /// Checkpoint invalidated by an earlier rollback.
    Stale,
}

/// Returns `true` when `status` may remain a rollback target after a rewind (`#40745`).
#[must_use]
pub const fn is_rollback_preserved_checkpoint(
    status: CheckpointTargetStatus,
    has_run_id: bool,
) -> bool {
    match status {
        CheckpointTargetStatus::Ready => true,
        CheckpointTargetStatus::Missing | CheckpointTargetStatus::Error => has_run_id,
        CheckpointTargetStatus::Stale => false,
    }
}

/// Whether a rewind operation modifies git files alongside conversation state (`#40745`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindMode {
    /// Restore both git workspace files and conversation history.
    RestoreWorkspace,
    /// Rewind conversation history only (`restoreFiles: false`, `#40745`).
    ConversationOnly,
}

/// Parameters checked when validating a user rewind/rollback checkpoint target (`#40745`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointRewindCheck<'a> {
    /// Target checkpoint scope `repositoryPath`, if sub-repository-scoped.
    pub scope_repository_path: Option<&'a str>,
    /// Target scope ID requested for rewind.
    pub scope_id: &'a str,
    /// Scope ID recorded on the checkpoint.
    pub checkpoint_scope_id: &'a str,
    /// Persisted checkpoint capture status.
    pub status: CheckpointTargetStatus,
    /// Workspace vs conversation-only rewind mode.
    pub mode: RewindMode,
    /// Whether the scope uses linked multi-repo project folders (missing snapshots still restore repos).
    pub linked_project_folder: bool,
}

/// Validates whether a user rewind/rollback can target a checkpoint (`#40745`).
#[must_use]
pub fn can_rewind_to_checkpoint(check: &CheckpointRewindCheck<'_>) -> bool {
    if check.scope_repository_path.is_some()
        || check.checkpoint_scope_id != check.scope_id
        || check.status == CheckpointTargetStatus::Stale
    {
        return false;
    }
    let restore_files = check.mode == RewindMode::RestoreWorkspace;
    match check.status {
        CheckpointTargetStatus::Ready => true,
        CheckpointTargetStatus::Error => !restore_files,
        CheckpointTargetStatus::Missing => !restore_files || check.linked_project_folder,
        CheckpointTargetStatus::Stale => false,
    }
}

/// Folder descriptor yielded by a single workspace scan (`#40671`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderWalkEntry {
    /// Absolute directory path.
    pub path: String,
    /// Whether this directory is a git repository root.
    pub is_git_repository: bool,
    /// Canonical repository root path if inside a repository.
    pub repository_path: Option<String>,
}

/// Input entry passed into submodule discovery using the already-scanned folders (`#40671`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmoduleScanInput {
    /// Candidate repository directory path.
    pub path: String,
    /// Project identity root used to stabilize checkpoint scope identities.
    pub identity: String,
}

/// Builds primary repository targets and submodule inputs from ONE workspace folder walk (`#40671`).
#[must_use]
pub fn plan_checkpoint_scope_walk_once(
    workspace_root: &str,
    project_root: Option<&str>,
    folders: &[FolderWalkEntry],
) -> (Vec<FolderWalkEntry>, Vec<SubmoduleScanInput>) {
    let git_folders: Vec<FolderWalkEntry> = folders
        .iter()
        .filter(|folder| folder.is_git_repository)
        .cloned()
        .collect();
    let root_identity = project_root.unwrap_or(workspace_root).to_owned();
    let mut submodule_inputs = Vec::with_capacity(git_folders.len() + 1);
    submodule_inputs.push(SubmoduleScanInput {
        path: workspace_root.to_owned(),
        identity: root_identity,
    });
    for folder in &git_folders {
        submodule_inputs.push(SubmoduleScanInput {
            path: folder.path.clone(),
            identity: folder
                .repository_path
                .clone()
                .unwrap_or_else(|| folder.path.clone()),
        });
    }
    (git_folders, submodule_inputs)
}
