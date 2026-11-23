//! Types, constants, and errors for the durable SQLite outbox.

use legion_contracts::ids::{
    CheckpointId, CheckpointScopeId, CommandId, MessageId, ProviderSessionId, ProviderThreadId,
    ProviderTurnId, RunId, ThreadId,
};
use legion_contracts::orchestration::ModelSelection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Marker persisted inside `provider-thread.rollback` outbox payloads before destructive restore (`BUG-02`).
pub const ROLLBACK_PENDING_INTENT: &str = "rollback.pending";

/// Effect types bound to the live server process and cancelled on process loss.
pub const PROCESS_BOUND_EFFECT_TYPES: &[&str] = &[
    "provider-session.apply-model-selection",
    "provider-session.apply-thread-title",
    "provider-turn.start",
    "provider-turn.interrupt",
    "provider-turn.steer",
    "provider-turn.restart",
    "runtime-request.respond",
];

/// Effect types safe to replay after an unclean server restart (`EffectOutbox.ts:142-153`).
pub const REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS: &[&str] = &[
    "provider-runtime.continue",
    "provider-session.detach",
    "provider-thread.rollback",
    "checkpoint.capture",
    "scheduled-tasks.cleanup",
    "terminal.cleanup",
    "preview.cleanup",
    "attachment.cleanup",
    "thread-title.generate",
    "delegated-tasks.stop",
];

/// Lifecycle status of an outbox row in `orchestration_v2_effect_outbox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutboxStatus {
    /// Queued and waiting to be claimed.
    Pending,
    /// Currently leased to a worker.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Exhausted retries or hit a permanent failure.
    Failed,
    /// Cancelled before execution or during shutdown/archive.
    Cancelled,
}

impl OutboxStatus {
    /// Returns the wire/`SQLite` string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_db(raw: &str) -> Result<Self, OutboxStoreError> {
        match raw {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(OutboxStoreError::Decode(format!(
                "unknown outbox status `{other}`"
            ))),
        }
    }
}

/// Typed effect payload persisted in `orchestration_v2_effect_outbox.payload_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutboxEffectRequest {
    /// Start executing a run on its provider.
    #[serde(rename = "provider-turn.start", rename_all = "camelCase")]
    ProviderTurnStart {
        /// Run ID to execute.
        run_id: RunId,
    },
    /// Interrupt an active provider turn.
    #[serde(rename = "provider-turn.interrupt", rename_all = "camelCase")]
    ProviderTurnInterrupt {
        /// Optional run ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        /// Optional provider session ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_session_id: Option<ProviderSessionId>,
        /// Optional provider thread ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_thread_id: Option<ProviderThreadId>,
        /// Optional provider turn ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_turn_id: Option<ProviderTurnId>,
    },
    /// Steer an active provider turn with a user/system message.
    #[serde(rename = "provider-turn.steer", rename_all = "camelCase")]
    ProviderTurnSteer {
        /// Target active run ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        /// Dispatched message ID.
        message_id: MessageId,
        /// Optional model selection override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_selection: Option<ModelSelection>,
    },
    /// Interrupt and restart a run with new input.
    #[serde(rename = "provider-turn.restart", rename_all = "camelCase")]
    ProviderTurnRestart {
        /// Target run ID.
        run_id: RunId,
        /// Dispatched message ID.
        message_id: MessageId,
        /// Optional model selection override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_selection: Option<ModelSelection>,
    },
    /// Roll back conversation and optionally workspace files to a checkpoint (`BUG-02`).
    #[serde(rename = "provider-thread.rollback", rename_all = "camelCase")]
    ProviderThreadRollback {
        /// Always `"rollback.pending"` when queued prior to destructive restore.
        intent: String,
        /// Rollback command ID.
        command_id: CommandId,
        /// Optional active provider thread ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_thread_id: Option<ProviderThreadId>,
        /// Target checkpoint ID.
        checkpoint_id: CheckpointId,
        /// Target checkpoint scope ID.
        scope_id: CheckpointScopeId,
        /// Whether workspace files should be restored alongside conversation rollback.
        restore_files: bool,
    },
    /// Stop delegated child tasks spawned by a thread.
    #[serde(rename = "delegated-tasks.stop", rename_all = "camelCase")]
    DelegatedTasksStop {
        /// Optional stop reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Generate or regenerate a thread title.
    #[serde(rename = "thread-title.generate", rename_all = "camelCase")]
    ThreadTitleGenerate {
        /// Generation kind (`initial` or `regenerate`).
        kind: String,
        /// Optional initial message ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<MessageId>,
    },
    /// Detach a provider session from the thread.
    #[serde(rename = "provider-session.detach", rename_all = "camelCase")]
    ProviderSessionDetach {
        /// Provider session ID to detach.
        provider_session_id: ProviderSessionId,
        /// Optional detach detail.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Capture a checkpoint after run completion.
    #[serde(rename = "checkpoint.capture", rename_all = "camelCase")]
    CheckpointCapture {
        /// Completed run ID.
        run_id: RunId,
        /// Checkpoint scope ID.
        scope_id: CheckpointScopeId,
    },
    /// Arbitrary forward-compatible effect payload.
    #[serde(other)]
    Unknown,
}

impl OutboxEffectRequest {
    /// Constructs a `provider-thread.rollback` request carrying the `"rollback.pending"` intent.
    pub fn rollback(
        command_id: CommandId,
        provider_thread_id: Option<ProviderThreadId>,
        checkpoint_id: CheckpointId,
        scope_id: CheckpointScopeId,
        restore_files: bool,
    ) -> Self {
        Self::ProviderThreadRollback {
            intent: ROLLBACK_PENDING_INTENT.to_owned(),
            command_id,
            provider_thread_id,
            checkpoint_id,
            scope_id,
            restore_files,
        }
    }

    /// Returns the wire `effect_type` string stored in `orchestration_v2_effect_outbox`.
    pub const fn effect_type(&self) -> &'static str {
        match self {
            Self::ProviderTurnStart { .. } => "provider-turn.start",
            Self::ProviderTurnInterrupt { .. } => "provider-turn.interrupt",
            Self::ProviderTurnSteer { .. } => "provider-turn.steer",
            Self::ProviderTurnRestart { .. } => "provider-turn.restart",
            Self::ProviderThreadRollback { .. } => "provider-thread.rollback",
            Self::DelegatedTasksStop { .. } => "delegated-tasks.stop",
            Self::ThreadTitleGenerate { .. } => "thread-title.generate",
            Self::ProviderSessionDetach { .. } => "provider-session.detach",
            Self::CheckpointCapture { .. } => "checkpoint.capture",
            Self::Unknown => "unknown",
        }
    }
}

/// Unpersisted outbox effect produced by the command decider for atomic commit in `EventSink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOutboxEffect {
    /// Deterministic idempotency key for this outbox effect.
    pub id: String,
    /// Originating command ID.
    pub command_id: CommandId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Typed effect request payload.
    pub request: OutboxEffectRequest,
    /// Optional earliest ISO-8601 UTC availability timestamp.
    pub available_at: Option<String>,
}

/// Persisted outbox row loaded from `orchestration_v2_effect_outbox`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEffect {
    /// Unique effect identifier.
    pub id: String,
    /// Originating command ID.
    pub command_id: CommandId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Effect type string.
    pub effect_type: String,
    /// Decoded request payload.
    pub request: OutboxEffectRequest,
    /// Raw JSON payload stored in `SQLite`.
    pub raw_payload: Value,
    /// Current lifecycle status.
    pub status: OutboxStatus,
    /// Number of execution claims attempted so far.
    pub attempt_count: u64,
    /// Earliest ISO-8601 UTC claim timestamp.
    pub available_at: String,
    /// Active worker lease owner, if running.
    pub lease_owner: Option<String>,
    /// Row creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Completion timestamp, if terminal.
    pub completed_at: Option<String>,
    /// Last recorded error string, if any.
    pub last_error: Option<String>,
}

/// Errors produced by `SQLite` outbox queries and mutations.
#[derive(Debug, Error)]
pub enum OutboxStoreError {
    /// Underlying `SQLite` failure.
    #[error("outbox SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// JSON serialization/deserialization or enum decode failure.
    #[error("outbox codec error: {0}")]
    Decode(String),
    /// Shared `SQLite` mutex was poisoned by a panic on another thread.
    #[error("shared SQLite connection mutex was poisoned")]
    LockPoisoned,
}
