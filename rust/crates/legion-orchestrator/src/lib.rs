#![forbid(unsafe_code)]
//! Event-sourced `OrchestratorV2` command decider, transactional `EventSink`, outbox, and projections.
//!
//! Exports all orchestration building blocks for Wave C (`legion-cli`) and embedded desktop/server hosts:
//! - [`locks`]: Typed rank-enforced lock hierarchy (`DeletionGuard -> MessageIntake -> ParentThread -> ChildThread`).
//! - [`sink`]: Atomic `SQLite` [`EventSink`] with savepoint-safe post-commit `PubSub` (`BUG-01`) and command receipts.
//! - [`decider`]: Pure [`decide_command`] emitting durable `"rollback.pending"` intents before git restore (`BUG-02`).
//! - [`outbox`]: [`EffectOutboxWorker`] with zero-sleep deterministic `drain().await`.
//! - [`projections`]: [`ShellSnapshot`] (`shellThreadFamilies` + `shellOlderSettled` 25-thread cut) and 24h [`prune_event_store`].
//! - [`orchestrator`]: High-level [`Orchestrator`] tying lock hierarchy, decider, sink, and outbox together.

pub mod decider;
pub mod locks;
pub mod orchestrator;
mod orchestrator_replay;
pub mod outbox;
pub mod projections;
pub mod scheduled_tasks;
pub mod session_recovery;
pub mod sink;
pub mod subagent_lifecycle;
pub mod time;

pub use decider::{CommandDecision, DeciderError, RollbackIntent, decide_command};
pub use locks::{
    CanAcquire, ChildThreadRank, DeletionRank, LockError, LockRank, LockRankMarker, LockSession,
    MessageIntakeRank, OrchestratorLocks, ParentThreadRank, Unlocked,
};
pub use orchestrator::{DispatchResult, Orchestrator, OrchestratorError};
pub use outbox::{
    EffectOutboxWorker, OutboxEffect, OutboxEffectExecutor, OutboxEffectRequest, OutboxSignal,
    OutboxStatus, OutboxStoreError, OutboxWorkerError, OutboxWorkerOptions, OutboxWorkerStats,
    PROCESS_BOUND_EFFECT_TYPES, PendingOutboxEffect, REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS,
    ROLLBACK_PENDING_INTENT, cancel_unsettled_effects, enqueue_effects, get_effect,
    list_by_command_id, prune_settled_effects, reconcile_after_process_loss,
};
pub use projections::{
    EVENT_LOG_PRUNE_MARK_PROJECTION, EVENT_LOG_PRUNED_THROUGH_PROJECTION, OlderSettledProjectCount,
    OlderSettledSummary, ProjectionError, PruneEventStoreReport, RETAINED_EVENT_HISTORY_MS,
    SHELL_NEWEST_SETTLED_KEPT, ShellSnapshot, ShellSnapshotOptions, ShellThread, ShellThreadFamily,
    ShellThreadStatus, list_older_settled_threads, prune_event_store, prune_event_store_now,
    query_shell_snapshot,
};
pub use sink::{
    CommandReceipt, CommitBatch, CommitCheckpoint, CommitCommandInput, CommitCommandOutcome,
    EventPubSub, EventSink, EventSubscriber, ReceiptStatus, SinkError, SinkSavepoint,
    SinkTransaction,
};
