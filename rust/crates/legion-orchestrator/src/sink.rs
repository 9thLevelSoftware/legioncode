//! Atomic transactional [`EventSink`] with savepoint-safe post-commit `PubSub` emission (`BUG-01`).
//!
//! All domain events, V2 projections (`threads`, `runs`, `messages`, `metadata`),
//! idempotency command receipts (`orchestration_command_receipts`), and durable outbox rows
//! (`orchestration_v2_effect_outbox`) commit in one outermost `SQLite` transaction.
//! `PubSub` and outbox worker wakeups are buffered in [`CommitBatch`] and emitted strictly AFTER
//! outermost `COMMIT` returns `Ok(())` and the database mutex is released.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::{Arc, Mutex};
//! use legion_contracts::ids::{CommandId, EventId, ProjectId, ThreadId};
//! use legion_orchestrator::sink::{EventPubSub, EventSink};
//! use legion_persistence::sqlite::SqliteStore;
//!
//! let store = SqliteStore::open_in_memory()?;
//! let db = Arc::new(Mutex::new(store.into_connection()));
//! let pubsub = EventPubSub::new();
//! let mut sub = pubsub.subscribe();
//! let sink = EventSink::new(Arc::clone(&db), Arc::clone(&pubsub), None);
//!
//! // Rolled-back transaction emits zero PubSub notifications (`BUG-01`).
//! {
//!     let mut tx = sink.begin_tx()?;
//!     let sp = tx.savepoint()?;
//!     sp.commit()?; // Inner savepoint commits, outer tx still rolls back on drop.
//! }
//! assert!(sub.drain_ready().is_empty());
//! # let _ = (CommandId::new("cmd-1")?, EventId::new("ev-1")?, ProjectId::new("p-1")?, ThreadId::new("t-1")?);
//! # Ok(())
//! # }
//! ```

pub mod commit_batch;
pub mod event_codec;
pub mod projection_apply;
pub mod projection_load;
pub mod tx;
pub mod types;

use std::sync::{Arc, Mutex};

use legion_contracts::ids::CommandId;
use rusqlite::Connection;

use crate::outbox::OutboxSignal;
use tx::load_receipt;
pub use tx::{SinkSavepoint, SinkTransaction};
pub use types::{
    CommandReceipt, CommitBatch, CommitCheckpoint, CommitCommandInput, CommitCommandOutcome,
    EventPubSub, EventSubscriber, ReceiptStatus, SinkError,
};

/// Atomic V2 event store, projection writer, receipt registry, and outbox enqueuer.
#[derive(Clone)]
pub struct EventSink {
    db: Arc<Mutex<Connection>>,
    pubsub: Arc<EventPubSub>,
    outbox_signal: Option<Arc<OutboxSignal>>,
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventSink").finish_non_exhaustive()
    }
}

impl EventSink {
    /// Creates an [`EventSink`] sharing `db`, publishing post-commit events to `pubsub`,
    /// and optionally waking `outbox_signal` after outbox inserts commit.
    pub fn new(
        db: Arc<Mutex<Connection>>,
        pubsub: Arc<EventPubSub>,
        outbox_signal: Option<Arc<OutboxSignal>>,
    ) -> Self {
        Self {
            db,
            pubsub,
            outbox_signal,
        }
    }

    /// Returns a reference to the underlying `PubSub` hub.
    pub fn pubsub(&self) -> &Arc<EventPubSub> {
        &self.pubsub
    }

    /// Returns the shared `SQLite` mutex handle.
    pub fn db(&self) -> &Arc<Mutex<Connection>> {
        &self.db
    }

    /// Begins an outermost `IMMEDIATE` transaction owning a [`CommitBatch`].
    pub fn begin_tx(&self) -> Result<SinkTransaction<'_>, SinkError> {
        let guard = self.db.lock().map_err(|_| SinkError::LockPoisoned)?;
        guard.execute_batch("BEGIN IMMEDIATE;")?;
        Ok(SinkTransaction {
            guard: Some(guard),
            batch: CommitBatch::new(),
            pubsub: Arc::clone(&self.pubsub),
            outbox_signal: self.outbox_signal.clone(),
            next_sp_id: 0,
            settled: false,
        })
    }

    /// Runs `f` inside an outermost transaction, committing and publishing strictly on `Ok`.
    pub fn with_tx<R>(
        &self,
        f: impl FnOnce(&mut SinkTransaction<'_>) -> Result<R, SinkError>,
    ) -> Result<R, SinkError> {
        let mut tx = self.begin_tx()?;
        let out = f(&mut tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Commits a command atomically inside an outermost transaction with receipt idempotency.
    pub fn commit_command(
        &self,
        input: &CommitCommandInput,
    ) -> Result<CommitCommandOutcome, SinkError> {
        self.with_tx(|tx| tx.commit_command(input))
    }

    /// Persists a `rejected` command receipt (or replays an existing receipt error/conflict).
    pub fn commit_rejected_command(
        &self,
        command_id: &CommandId,
        command_type: &str,
        thread_id: &str,
        error: &str,
        now_iso: &str,
    ) -> Result<CommandReceipt, SinkError> {
        self.with_tx(|tx| {
            tx.commit_rejected_command(command_id, command_type, thread_id, error, now_iso)
        })
    }

    /// Fetches a receipt by `command_id` outside a write transaction.
    pub fn get_receipt(&self, command_id: &CommandId) -> Result<Option<CommandReceipt>, SinkError> {
        let guard = self.db.lock().map_err(|_| SinkError::LockPoisoned)?;
        load_receipt(&guard, command_id)
    }
}
