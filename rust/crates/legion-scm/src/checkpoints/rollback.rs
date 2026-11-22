//! `BUG-02` durable pending-intent rollback guard and safe git restore command planning.
//!
//! Enforces that git `restore` / `clean` arguments can NEVER be emitted until
//! [`RollbackCoordinator::verify_pending_intent`] confirms a live durable
//! `rollback.pending` outbox effect, thread projection `rollbackRequestId`, and
//! `checkpoint.rollback-requested` orchestration event in `SQLite`.

use legion_contracts::ids::{CheckpointId, CheckpointRef, CheckpointScopeId, CommandId, ThreadId};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::checkpoints::CheckpointError;
use crate::checkpoints::diff_query::{MAX_PATHSPEC_CHARS, batch_pathspecs};

/// Caller-supplied rollback target awaiting durable verification against `SQLite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackIntent {
    /// Thread being restored.
    pub thread_id: ThreadId,
    /// Rollback request command identifier (`rollbackRequestId` / `commandId`).
    pub command_id: CommandId,
    /// Target checkpoint identifier stored in the outbox payload.
    pub checkpoint_id: CheckpointId,
    /// Optional scoped worktree identifier expected in the outbox payload.
    pub scope_id: Option<CheckpointScopeId>,
    /// Hidden git checkpoint ref resolved for this rollback.
    pub checkpoint_ref: CheckpointRef,
    /// Whether git workspace files are restored alongside conversation state.
    pub restore_files: bool,
    /// Whether this worktree is shared with other active threads (`CheckpointRestoreSafety`).
    pub worktree_is_shared: bool,
}

/// Capability token proving [`RollbackCoordinator::verify_pending_intent`] succeeded.
///
/// Private field prevents constructing restore plans without database verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRollbackIntent {
    checkpoint_ref: CheckpointRef,
    restore_files: bool,
    _sealed: (),
}

impl VerifiedRollbackIntent {
    /// Returns the verified git checkpoint reference.
    #[must_use]
    pub fn checkpoint_ref(&self) -> &str {
        self.checkpoint_ref.as_str()
    }

    /// Returns whether workspace files should be restored.
    #[must_use]
    pub const fn restore_files(&self) -> bool {
        self.restore_files
    }
}

/// Safe git restore and untracked-cleanup argument sequences ready for VCS execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRestorePlan {
    /// Verified hidden checkpoint ref to restore from.
    pub checkout_ref: String,
    /// Batched `git restore` argument vectors (`--worktree --staged --source <ref> -- ...`).
    pub restore_batches: Vec<Vec<String>>,
    /// Optional `git clean -fd -- .` argument vector when requested.
    pub clean_untracked_args: Option<Vec<String>>,
}

/// Coordinator enforcing `BUG-02` durable intent checks before planning git restore commands.
#[derive(Debug, Default, Clone, Copy)]
pub struct RollbackCoordinator;

impl RollbackCoordinator {
    /// Verifies durable `rollback.pending` outbox, projection, and event state in `conn`.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::RollbackGuard`] if:
    /// - `checkpoint_ref` begins with `-` (git option injection risk),
    /// - `worktree_is_shared && restore_files` (shared worktree overwrite risk),
    /// - no pending/running `provider-thread.rollback` outbox row carries `intent == "rollback.pending"`
    ///   matching `thread_id`, `command_id`, `checkpoint_id`, and `scope_id`,
    /// - `orchestration_v2_projection_threads` lacks `rollbackRequestId == command_id`, or
    /// - `orchestration_events` lacks `checkpoint.rollback-requested` for `(thread_id, command_id)`.
    pub fn verify_pending_intent(
        conn: &Connection,
        intent: &RollbackIntent,
    ) -> Result<VerifiedRollbackIntent, CheckpointError> {
        validate_rollback_target(intent)?;
        verify_outbox_pending(conn, intent)?;
        verify_projection_and_event(conn, intent)?;

        Ok(VerifiedRollbackIntent {
            checkpoint_ref: intent.checkpoint_ref.clone(),
            restore_files: intent.restore_files,
            _sealed: (),
        })
    }

    /// Builds git restore/clean arguments from an already-verified [`VerifiedRollbackIntent`].
    ///
    /// Returns empty `restore_batches` and `clean_untracked_args: None` if `restore_files` is `false`.
    #[must_use]
    pub fn plan_git_restore(
        verified: &VerifiedRollbackIntent,
        tracked_paths: &[&str],
        clean_untracked: bool,
    ) -> GitRestorePlan {
        let ref_str = verified.checkpoint_ref().to_owned();
        if !verified.restore_files() {
            return GitRestorePlan {
                checkout_ref: ref_str,
                restore_batches: Vec::new(),
                clean_untracked_args: None,
            };
        }

        let restore_batches = if tracked_paths.is_empty() {
            vec![vec![
                "restore".to_owned(),
                "--source".to_owned(),
                ref_str.clone(),
                "--worktree".to_owned(),
                "--staged".to_owned(),
                "--".to_owned(),
                ".".to_owned(),
            ]]
        } else {
            batch_pathspecs(tracked_paths, MAX_PATHSPEC_CHARS)
                .into_iter()
                .map(|batch| {
                    let mut args = vec![
                        "restore".to_owned(),
                        "--source".to_owned(),
                        ref_str.clone(),
                        "--worktree".to_owned(),
                        "--staged".to_owned(),
                        "--".to_owned(),
                    ];
                    args.extend(batch);
                    args
                })
                .collect()
        };

        let clean_untracked_args = clean_untracked.then(|| {
            vec![
                "clean".to_owned(),
                "-fd".to_owned(),
                "--".to_owned(),
                ".".to_owned(),
            ]
        });

        GitRestorePlan {
            checkout_ref: ref_str,
            restore_batches,
            clean_untracked_args,
        }
    }

    /// Convenience helper combining [`Self::verify_pending_intent`] and [`Self::plan_git_restore`].
    ///
    /// # Errors
    ///
    /// Propagates any [`CheckpointError`] raised by pending-intent verification.
    pub fn plan_git_restore_checked(
        conn: &Connection,
        intent: &RollbackIntent,
        tracked_paths: &[&str],
        clean_untracked: bool,
    ) -> Result<GitRestorePlan, CheckpointError> {
        let verified = Self::verify_pending_intent(conn, intent)?;
        Ok(Self::plan_git_restore(
            &verified,
            tracked_paths,
            clean_untracked,
        ))
    }
}

fn validate_rollback_target(intent: &RollbackIntent) -> Result<(), CheckpointError> {
    let ref_str = intent.checkpoint_ref.as_str();
    if ref_str.trim().is_empty()
        || ref_str.starts_with('-')
        || ref_str.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(CheckpointError::RollbackGuard(format!(
            "unsafe checkpoint ref `{ref_str}`"
        )));
    }
    if intent.worktree_is_shared && intent.restore_files {
        return Err(CheckpointError::RollbackGuard(
            "cannot restore files in a shared worktree with multiple threads".to_owned(),
        ));
    }
    Ok(())
}

fn verify_outbox_pending(
    conn: &Connection,
    intent: &RollbackIntent,
) -> Result<(), CheckpointError> {
    let outbox_payload_raw: Option<String> = conn
        .query_row(
            "SELECT payload_json
             FROM orchestration_v2_effect_outbox
             WHERE thread_id = ?1
               AND effect_type = 'provider-thread.rollback'
               AND status IN ('pending', 'running')
               AND json_extract(payload_json, '$.intent') = 'rollback.pending'
               AND json_extract(payload_json, '$.commandId') = ?2
               AND json_extract(payload_json, '$.checkpointId') = ?3
             LIMIT 1",
            params![
                intent.thread_id.as_str(),
                intent.command_id.as_str(),
                intent.checkpoint_id.as_str(),
            ],
            |row| row.get(0),
        )
        .optional()?;

    let Some(payload_json) = outbox_payload_raw else {
        return Err(CheckpointError::RollbackGuard(format!(
            "missing durable rollback.pending outbox effect for thread `{}` command `{}`",
            intent.thread_id.as_str(),
            intent.command_id.as_str(),
        )));
    };

    let payload: Value = serde_json::from_str(&payload_json).map_err(|err| {
        CheckpointError::RollbackGuard(format!("corrupt rollback outbox JSON payload: {err}"))
    })?;
    if let Some(expected_scope) = &intent.scope_id {
        let actual_scope = payload.get("scopeId").and_then(Value::as_str);
        if actual_scope != Some(expected_scope.as_str()) {
            return Err(CheckpointError::RollbackGuard(format!(
                "outbox scopeId mismatch: expected `{}` got `{actual_scope:?}`",
                expected_scope.as_str(),
            )));
        }
    }
    if let Some(restore_flag) = payload.get("restoreFiles").and_then(Value::as_bool)
        && restore_flag != intent.restore_files
    {
        return Err(CheckpointError::RollbackGuard(format!(
            "outbox restoreFiles mismatch: expected {} got {restore_flag}",
            intent.restore_files
        )));
    }
    Ok(())
}

fn verify_projection_and_event(
    conn: &Connection,
    intent: &RollbackIntent,
) -> Result<(), CheckpointError> {
    let proj_match: Option<i64> = conn
        .query_row(
            "SELECT 1
             FROM orchestration_v2_projection_threads
             WHERE thread_id = ?1
               AND json_extract(payload_json, '$.rollbackRequestId') = ?2
             LIMIT 1",
            params![intent.thread_id.as_str(), intent.command_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    if proj_match.is_none() {
        return Err(CheckpointError::RollbackGuard(format!(
            "thread projection missing active rollbackRequestId `{}`",
            intent.command_id.as_str()
        )));
    }

    let event_match: Option<i64> = conn
        .query_row(
            "SELECT 1
             FROM orchestration_events
             WHERE stream_id = ?1
               AND event_type = 'checkpoint.rollback-requested'
               AND (
                 command_id = ?2
                 OR json_extract(payload_json, '$.rollbackRequestId') = ?2
               )
             LIMIT 1",
            params![intent.thread_id.as_str(), intent.command_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    if event_match.is_none() {
        return Err(CheckpointError::RollbackGuard(format!(
            "orchestration_events missing checkpoint.rollback-requested for `{}`",
            intent.command_id.as_str()
        )));
    }
    Ok(())
}
