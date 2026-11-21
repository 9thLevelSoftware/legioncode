//! Core types, errors, receipts, and post-commit `PubSub` buffering for [`crate::sink::EventSink`].

use std::sync::{Arc, Mutex, PoisonError};

use t3_contracts::ids::{CommandId, EventId, ThreadId};
use t3_contracts::orchestration::DomainEvent;
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::outbox::{OutboxSignal, OutboxStoreError, PendingOutboxEffect};
use crate::sink::event_codec::PersistedEvent;

/// Errors returned by [`crate::sink::EventSink`] and transaction helpers.
#[derive(Debug, Error)]
pub enum SinkError {
    /// Underlying `SQLite` error.
    #[error("event sink SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Outbox persistence error.
    #[error(transparent)]
    Outbox(#[from] OutboxStoreError),
    /// Event or projection serialization/decoding failure.
    #[error("event sink codec error: {0}")]
    Codec(String),
    /// Shared `SQLite` mutex was poisoned.
    #[error("event sink SQLite mutex was poisoned")]
    LockPoisoned,
    /// Same `CommandId` was already committed against a different thread aggregate.
    #[error(
        "command `{command_id}` already committed for thread `{existing_thread_id}`, cannot target `{requested_thread_id}`"
    )]
    CommandIdConflict {
        /// Reused command identifier.
        command_id: CommandId,
        /// Thread ID stored in the original receipt.
        existing_thread_id: String,
        /// Thread ID requested by the conflicting submission.
        requested_thread_id: String,
    },
    /// Same `CommandId` previously committed a `rejected` receipt.
    #[error("command `{command_id}` was previously rejected: {error}")]
    CommandPreviouslyRejected {
        /// Rejected command identifier.
        command_id: CommandId,
        /// Stored rejection reason.
        error: String,
    },
}

/// Receipt status persisted in `orchestration_command_receipts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiptStatus {
    /// Command succeeded and committed events/effects.
    Accepted,
    /// Command failed validation and recorded a durable rejection receipt.
    Rejected,
}

impl ReceiptStatus {
    /// Returns the `SQLite` string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    pub(crate) fn from_db(raw: &str) -> Result<Self, SinkError> {
        match raw {
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            other => Err(SinkError::Codec(format!(
                "unknown command receipt status `{other}`"
            ))),
        }
    }
}

/// Persisted row from `orchestration_command_receipts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReceipt {
    /// Unique command ID.
    pub command_id: CommandId,
    /// Aggregate kind (`"thread"`).
    pub aggregate_kind: String,
    /// Aggregate ID (thread ID).
    pub aggregate_id: String,
    /// ISO-8601 UTC timestamp when the command was recorded.
    pub accepted_at: String,
    /// Highest `orchestration_events.sequence` produced (`0` if none).
    pub result_sequence: i64,
    /// Receipt disposition.
    pub status: ReceiptStatus,
    /// Error summary when `status == ReceiptStatus::Rejected`.
    pub error: Option<String>,
    /// Command discriminator (`thread.create`, `checkpoint.rollback`, etc.).
    pub command_type: String,
}

/// Input payload for [`crate::sink::EventSink::commit_command`].
#[derive(Debug, Clone, PartialEq)]
pub struct CommitCommandInput {
    /// Idempotent command identifier.
    pub command_id: CommandId,
    /// Command discriminator stored on `orchestration_command_receipts`.
    pub command_type: String,
    /// Target thread aggregate ID.
    pub thread_id: ThreadId,
    /// Actor kind recorded in `orchestration_events`.
    pub actor_kind: String,
    /// Optional causation event ID.
    pub causation_event_id: Option<EventId>,
    /// Optional correlation ID.
    pub correlation_id: Option<String>,
    /// Transaction timestamp in ISO-8601 UTC.
    pub now_iso: String,
    /// Domain events to append atomically.
    pub events: Vec<DomainEvent>,
    /// Outbox effects to enqueue atomically.
    pub outbox_effects: Vec<PendingOutboxEffect>,
}

/// Result of [`crate::sink::EventSink::commit_command`].
#[derive(Debug, Clone, PartialEq)]
pub struct CommitCommandOutcome {
    /// Events produced by this command (either newly written or replayed from receipt).
    pub events: Vec<PersistedEvent>,
    /// Recorded or replayed command receipt.
    pub receipt: CommandReceipt,
    /// `true` when returned from an idempotent `CommandId` receipt hit.
    pub replayed: bool,
}

/// Snapshot of [`CommitBatch`] lengths taken when entering a `SQLite` savepoint (`BUG-01`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitCheckpoint {
    pub(crate) event_len: usize,
    pub(crate) outbox_len: usize,
}

/// Buffered notifications accumulated during a transaction and emitted ONLY after outermost `COMMIT` (`BUG-01`).
#[derive(Debug, Default)]
pub struct CommitBatch {
    events: Vec<PersistedEvent>,
    pending_outbox_wakeups: usize,
}

impl CommitBatch {
    /// Creates an empty [`CommitBatch`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Captures a restore point before entering a nested savepoint.
    pub fn checkpoint(&self) -> CommitCheckpoint {
        CommitCheckpoint {
            event_len: self.events.len(),
            outbox_len: self.pending_outbox_wakeups,
        }
    }

    /// Discards buffered events and outbox wakeups added after `checkpoint`.
    pub fn rollback_to(&mut self, checkpoint: CommitCheckpoint) {
        self.events.truncate(checkpoint.event_len);
        self.pending_outbox_wakeups = self.pending_outbox_wakeups.min(checkpoint.outbox_len);
    }

    /// Appends persisted events to emit after outermost commit succeeds.
    pub fn push_events(&mut self, events: &[PersistedEvent]) {
        self.events.extend_from_slice(events);
    }

    /// Records `count` outbox rows staged for post-commit wakeup.
    pub fn add_outbox_wakeups(&mut self, count: usize) {
        self.pending_outbox_wakeups = self.pending_outbox_wakeups.saturating_add(count);
    }

    /// Returns the number of buffered domain events waiting on commit.
    pub fn buffered_event_count(&self) -> usize {
        self.events.len()
    }

    /// Returns the number of buffered outbox wakeups waiting on commit.
    pub fn buffered_outbox_wakeups(&self) -> usize {
        self.pending_outbox_wakeups
    }

    pub(crate) fn publish_after_commit(
        self,
        pubsub: &EventPubSub,
        outbox_signal: Option<&OutboxSignal>,
    ) {
        if !self.events.is_empty() {
            pubsub.publish_batch(&self.events);
        }
        if self.pending_outbox_wakeups > 0
            && let Some(signal) = outbox_signal
        {
            signal.notify_available();
        }
    }
}

/// Lossless unbounded subscriber handle receiving committed [`PersistedEvent`] notifications.
#[derive(Debug)]
pub struct EventSubscriber {
    rx: UnboundedReceiver<PersistedEvent>,
}

impl EventSubscriber {
    /// Non-blocking attempt to read the next post-commit event.
    pub fn try_recv(&mut self) -> Result<PersistedEvent, tokio::sync::mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }

    /// Drains all currently queued post-commit events without blocking.
    pub fn drain_ready(&mut self) -> Vec<PersistedEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Waits for the next post-commit event, returning `None` when the [`EventPubSub`] is dropped.
    pub async fn recv(&mut self) -> Option<PersistedEvent> {
        self.rx.recv().await
    }
}

/// Fan-out publisher delivering [`PersistedEvent`] batches strictly after outermost commit (`BUG-01`).
#[derive(Debug, Default)]
pub struct EventPubSub {
    subscribers: Mutex<Vec<UnboundedSender<PersistedEvent>>>,
}

impl EventPubSub {
    /// Creates a new [`EventPubSub`].
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers a new [`EventSubscriber`] that receives every event committed after this call.
    pub fn subscribe(&self) -> EventSubscriber {
        let (tx, rx) = unbounded_channel();
        let mut subs = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subs.push(tx);
        EventSubscriber { rx }
    }

    pub(crate) fn publish_batch(&self, events: &[PersistedEvent]) {
        let mut subs = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subs.retain(|tx| {
            for event in events {
                if tx.send(event.clone()).is_err() {
                    return false;
                }
            }
            true
        });
    }
}
