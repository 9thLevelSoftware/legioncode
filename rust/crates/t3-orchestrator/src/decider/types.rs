//! Command decision outputs, rollback intent markers, and pure decider errors.

use serde::{Deserialize, Serialize};
use t3_contracts::ids::{CheckpointId, CheckpointScopeId, CommandId, EventId, ThreadId};
use t3_contracts::orchestration::{AppThread, DomainEvent};
use thiserror::Error;

use crate::outbox::{PendingOutboxEffect, ROLLBACK_PENDING_INTENT};
use crate::sink::CommitCommandInput;

/// Durable rollback intent recorded before destructive git workspace restore (`BUG-02`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackIntent {
    /// Always [`ROLLBACK_PENDING_INTENT`] (`"rollback.pending"`).
    pub status: String,
    /// Rollback command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Target checkpoint scope ID.
    pub scope_id: CheckpointScopeId,
    /// Target checkpoint ID.
    pub checkpoint_id: CheckpointId,
    /// Whether workspace files will be restored alongside conversation rollback.
    pub restore_files: bool,
    /// Timestamp when the rollback intent was decided.
    pub requested_at: String,
}

impl RollbackIntent {
    /// Constructs a `"rollback.pending"` intent record for `checkpoint.rollback`.
    pub fn pending(
        command_id: CommandId,
        thread_id: ThreadId,
        scope_id: CheckpointScopeId,
        checkpoint_id: CheckpointId,
        restore_files: bool,
        requested_at: impl Into<String>,
    ) -> Self {
        Self {
            status: ROLLBACK_PENDING_INTENT.to_owned(),
            command_id,
            thread_id,
            scope_id,
            checkpoint_id,
            restore_files,
            requested_at: requested_at.into(),
        }
    }
}

/// Deterministic output produced by [`crate::decider::decide_command`].
#[derive(Debug, Clone, PartialEq)]
pub struct CommandDecision {
    /// Originating command ID.
    pub command_id: CommandId,
    /// Wire command type string (`"thread.create"`, `"checkpoint.rollback"`, etc.).
    pub command_type: String,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Domain events to persist atomically.
    pub events: Vec<DomainEvent>,
    /// Outbox effects to enqueue atomically in the same transaction.
    pub outbox_effects: Vec<PendingOutboxEffect>,
    /// Durable `"rollback.pending"` marker when deciding `checkpoint.rollback` (`BUG-02`).
    pub rollback_intent: Option<RollbackIntent>,
    /// Updated materialized thread snapshot produced by this command, if any.
    pub resulting_thread: Option<AppThread>,
}

impl CommandDecision {
    /// Converts this decision into a [`CommitCommandInput`] for [`crate::sink::EventSink::commit_command`].
    pub fn into_commit_input(
        self,
        actor_kind: impl Into<String>,
        now_iso: impl Into<String>,
    ) -> CommitCommandInput {
        CommitCommandInput {
            command_id: self.command_id,
            command_type: self.command_type,
            thread_id: self.thread_id,
            actor_kind: actor_kind.into(),
            causation_event_id: None,
            correlation_id: None,
            now_iso: now_iso.into(),
            events: self.events,
            outbox_effects: self.outbox_effects,
        }
    }
}

/// Validation and state-transition errors returned by [`crate::decider::decide_command`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeciderError {
    /// Thread already exists when handling `thread.create`.
    #[error("thread `{0}` already exists")]
    ThreadAlreadyExists(ThreadId),
    /// Target thread does not exist.
    #[error("thread `{0}` does not exist")]
    ThreadNotFound(ThreadId),
    /// Target thread was soft-deleted.
    #[error("thread `{0}` has been deleted")]
    ThreadDeleted(ThreadId),
    /// Target thread is archived.
    #[error("thread `{0}` is archived")]
    ThreadArchived(ThreadId),
    /// Archiving blocked by a live app-owned subagent or active run.
    #[error("thread `{thread_id}` still has active work or live subagent `{child_thread_id}`")]
    ActiveWorkConflict {
        /// Parent thread ID.
        thread_id: ThreadId,
        /// Conflicting child or thread ID.
        child_thread_id: ThreadId,
    },
    /// Snooze requires a valid future ISO-8601 timestamp.
    #[error("invalid snooze timestamp `{snoozed_until}` relative to `{now_iso}`")]
    InvalidSnoozeTimestamp {
        /// Requested wake timestamp.
        snoozed_until: String,
        /// Current command evaluation timestamp.
        now_iso: String,
    },
    /// Thread cannot be snoozed while active runs are in flight.
    #[error("cannot snooze thread `{0}` while runs are active")]
    CannotSnoozeActiveThread(ThreadId),
    /// Thread is not currently snoozed when handling `thread.unsnooze`.
    #[error("thread `{0}` is not snoozed")]
    ThreadNotSnoozed(ThreadId),
    /// State conflict (stale `expected_latest_run_id`, active run, pending rollback, etc.).
    #[error("command conflict: {0}")]
    Conflict(String),
    /// Target run was missing or not in a valid state for the command.
    #[error("invalid run state: {0}")]
    InvalidRunState(String),
    /// Target checkpoint does not exist on the thread stream.
    #[error("checkpoint `{checkpoint_id}` not found for thread `{thread_id}`")]
    CheckpointNotFound {
        /// Owning thread ID.
        thread_id: ThreadId,
        /// Missing checkpoint ID.
        checkpoint_id: CheckpointId,
    },
    /// Command variant is outside the thread decider scope.
    #[error("command `{command_type}` is not supported by thread decider")]
    UnsupportedCommand {
        /// Wire command type.
        command_type: String,
    },
    /// Deterministic ID derivation failed contract validation.
    #[error("invalid identifier derived from command `{0}`")]
    InvalidId(String),
}

pub(crate) fn derived_event_id(
    command_id: &CommandId,
    index: usize,
) -> Result<EventId, DeciderError> {
    let raw = format!("{}-ev-{index}", command_id.as_str());
    EventId::new(raw).map_err(|err| DeciderError::InvalidId(err.to_string()))
}

pub(crate) fn derived_effect_id(command_id: &CommandId, index: usize) -> String {
    format!("{}-eff-{index}", command_id.as_str())
}
