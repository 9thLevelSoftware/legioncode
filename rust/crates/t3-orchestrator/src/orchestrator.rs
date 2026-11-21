//! High-level [`Orchestrator`] coordinating lock hierarchy, pure decider, atomic `EventSink`, and outbox.
//!
//! # Examples
//!
//! ```
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_contracts::ids::{CommandId, ProjectId, ProviderInstanceId, ThreadId};
//! use t3_contracts::orchestration::{
//!     CreationSource, ModelSelection, OrchestrationActor, OrchestrationCommand,
//!     ProviderInteractionMode, ThreadCreateCommand,
//! };
//! use t3_contracts::policy::RuntimeMode;
//! use t3_orchestrator::{Orchestrator, ShellSnapshotOptions};
//! use t3_persistence::sqlite::SqliteStore;
//!
//! let store = SqliteStore::open_in_memory()?;
//! let orch = Orchestrator::from_sqlite_store(store);
//! let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
//!     created_by: OrchestrationActor::User,
//!     creation_source: CreationSource::Web,
//!     command_id: CommandId::new("cmd-1")?,
//!     thread_id: ThreadId::new("thread-1")?,
//!     project_id: ProjectId::new("proj-1")?,
//!     title: "Orchestrate safely".to_owned(),
//!     generate_title: None,
//!     model_selection: ModelSelection {
//!         instance_id: ProviderInstanceId::new("codex")?,
//!         model: "gpt-5".to_owned(),
//!         options: None,
//!     },
//!     runtime_mode: RuntimeMode::FullAccess,
//!     interaction_mode: ProviderInteractionMode::Default,
//!     branch: None,
//!     worktree_path: None,
//!     link_origin: None,
//!     delegated_from: None,
//!     imported_native_thread: None,
//!     initial_message: None,
//! });
//! let res = orch.dispatch_command(&cmd, "2026-11-20T00:00:00.000Z").await?;
//! assert!(!res.outcome.replayed);
//! let snap = orch.shell_snapshot("2026-11-20T00:00:00.000Z", ShellSnapshotOptions::default())?;
//! assert_eq!(snap.threads.len(), 1);
//! # Ok(())
//! # }
//! ```

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use t3_contracts::ids::{CommandId, ThreadId};
use t3_contracts::orchestration::{AppThread, OrchestrationCommand, SubmitRunOutputCommand};
use t3_persistence::sqlite::SqliteStore;
use thiserror::Error;

use crate::decider::{
    CommandDecision, DeciderError, RollbackIntent, decide_command, decide_pop_next_queued_run,
    decide_submit_run_output,
};
use crate::locks::{LockError, OrchestratorLocks};
use crate::orchestrator_replay::{command_thread_id, replay_existing_receipt};
use crate::outbox::{EffectOutboxWorker, OutboxEffectExecutor, OutboxSignal, OutboxWorkerOptions};
use crate::projections::{
    ProjectionError, PruneEventStoreReport, ShellSnapshot, ShellSnapshotOptions, prune_event_store,
    query_shell_snapshot,
};
use crate::sink::projection_apply::{DeciderThreadState, load_decider_state};
use crate::sink::tx::load_receipt;
use crate::sink::{CommitCommandOutcome, EventPubSub, EventSink, EventSubscriber, SinkError};
use crate::time::now_iso_utc;

/// Errors returned by [`Orchestrator`] command dispatch and queries.
#[derive(Debug, Error)]
pub enum OrchestratorError {
    /// Rank-enforced lock acquisition failure.
    #[error(transparent)]
    Lock(#[from] LockError),
    /// Pure command validation or state-transition rejection.
    #[error(transparent)]
    Decider(#[from] DeciderError),
    /// Atomic `EventSink` transaction or receipt error.
    #[error(transparent)]
    Sink(#[from] SinkError),
    /// Projection or retention query failure.
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    /// Target command carries no thread aggregate ID for thread-scoped dispatch.
    #[error("command `{command_type}` does not target a thread aggregate")]
    MissingThreadTarget {
        /// Wire command type string.
        command_type: String,
    },
}

/// Complete result returned by [`Orchestrator::dispatch_command`].
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchResult {
    /// Transactional commit outcome (`Committed` or idempotent `Replayed`).
    pub outcome: CommitCommandOutcome,
    /// Durable `"rollback.pending"` intent when dispatching `checkpoint.rollback`.
    pub rollback_intent: Option<RollbackIntent>,
    /// Updated materialized thread snapshot when available.
    pub resulting_thread: Option<AppThread>,
}

/// Top-level facade wiring [`OrchestratorLocks`], `decide_command`, [`EventSink`], and outbox signaling.
#[derive(Clone)]
pub struct Orchestrator {
    locks: OrchestratorLocks,
    sink: EventSink,
    pubsub: Arc<EventPubSub>,
    outbox_signal: Arc<OutboxSignal>,
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("locks", &self.locks)
            .finish_non_exhaustive()
    }
}

impl Orchestrator {
    /// Creates an [`Orchestrator`] wrapping an opened [`SqliteStore`].
    pub fn from_sqlite_store(store: SqliteStore) -> Self {
        Self::new(Arc::new(Mutex::new(store.into_connection())))
    }

    /// Creates an [`Orchestrator`] sharing `db` with default `PubSub` and outbox wakeup signal.
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self::with_signal(db, Arc::new(OutboxSignal::new()))
    }

    /// Creates an [`Orchestrator`] wired to a caller-provided [`OutboxSignal`].
    pub fn with_signal(db: Arc<Mutex<Connection>>, outbox_signal: Arc<OutboxSignal>) -> Self {
        let pubsub = EventPubSub::new();
        let sink = EventSink::new(db, Arc::clone(&pubsub), Some(Arc::clone(&outbox_signal)));
        Self {
            locks: OrchestratorLocks::new(),
            sink,
            pubsub,
            outbox_signal,
        }
    }

    /// Returns the rank-enforced lock hierarchy.
    pub const fn locks(&self) -> &OrchestratorLocks {
        &self.locks
    }

    /// Returns the atomic transactional [`EventSink`].
    pub const fn sink(&self) -> &EventSink {
        &self.sink
    }

    /// Returns the shared `PubSub` hub.
    pub const fn pubsub(&self) -> &Arc<EventPubSub> {
        &self.pubsub
    }

    /// Subscribes to post-commit domain events (`BUG-01`).
    pub fn subscribe(&self) -> EventSubscriber {
        self.pubsub.subscribe()
    }

    /// Returns the shared [`OutboxSignal`].
    pub const fn outbox_signal(&self) -> &Arc<OutboxSignal> {
        &self.outbox_signal
    }

    /// Spawns a background [`EffectOutboxWorker`] wired to this orchestrator's DB and [`OutboxSignal`].
    pub fn spawn_outbox_worker<E: OutboxEffectExecutor>(
        &self,
        executor: E,
        options: OutboxWorkerOptions,
    ) -> EffectOutboxWorker {
        EffectOutboxWorker::spawn_with_signal(
            Arc::clone(self.sink.db()),
            executor,
            options,
            Arc::clone(&self.outbox_signal),
        )
    }

    /// Creates a manual [`EffectOutboxWorker`] wired to this orchestrator's DB and [`OutboxSignal`].
    pub fn manual_outbox_worker<E: OutboxEffectExecutor>(
        &self,
        executor: E,
        options: OutboxWorkerOptions,
    ) -> EffectOutboxWorker {
        EffectOutboxWorker::new_manual_with_signal(
            Arc::clone(self.sink.db()),
            executor,
            options,
            Arc::clone(&self.outbox_signal),
        )
    }

    /// Dispatches `command` using the current UTC wall-clock timestamp.
    pub async fn dispatch_now(
        &self,
        command: &OrchestrationCommand,
    ) -> Result<DispatchResult, OrchestratorError> {
        self.dispatch_command(command, &now_iso_utc()).await
    }

    /// Dispatches `command` through lock hierarchy -> receipt check -> pure decider -> atomic `EventSink`.
    pub async fn dispatch_command(
        &self,
        command: &OrchestrationCommand,
        now_iso: &str,
    ) -> Result<DispatchResult, OrchestratorError> {
        let thread_id =
            command_thread_id(command).ok_or_else(|| OrchestratorError::MissingThreadTarget {
                command_type: command.command_type().to_owned(),
            })?;

        let s0 = self.locks.session();
        match command {
            OrchestrationCommand::ThreadDelete { .. } => {
                let s1 = s0.acquire_deletion_guard().await?;
                let _s2 = s1.acquire_child_thread(thread_id.clone()).await?;
                self.execute_locked_command(command, &thread_id, now_iso)
            }
            OrchestrationCommand::MessageDispatch(_) => {
                let s1 = s0.acquire_message_intake(thread_id.clone()).await?;
                let _s2 = s1.acquire_child_thread(thread_id.clone()).await?;
                self.execute_locked_command(command, &thread_id, now_iso)
            }
            OrchestrationCommand::ThreadArchive { .. } => {
                let _s1 = s0.acquire_parent_thread(thread_id.clone()).await?;
                self.execute_locked_command(command, &thread_id, now_iso)
            }
            _ => {
                let _s1 = s0.acquire_child_thread(thread_id.clone()).await?;
                self.execute_locked_command(command, &thread_id, now_iso)
            }
        }
    }

    /// Dispatches a server-internal `run.output.submit` command atomically against `SQLite`.
    pub async fn dispatch_submit_run_output(
        &self,
        command: &SubmitRunOutputCommand,
        now_iso: &str,
    ) -> Result<DispatchResult, OrchestratorError> {
        let _s1 = self
            .locks
            .session()
            .acquire_child_thread(command.thread_id.clone())
            .await?;
        self.execute_locked_decision(
            &command.command_id,
            "run.output.submit",
            &command.thread_id,
            now_iso,
            |state| decide_submit_run_output(state, command, now_iso),
        )
    }

    /// Pops and starts the next ready queued run on `thread_id`, inheriting live model selection.
    pub async fn pop_next_queued_run(
        &self,
        command_id: &CommandId,
        thread_id: &ThreadId,
        now_iso: &str,
    ) -> Result<DispatchResult, OrchestratorError> {
        let _s1 = self
            .locks
            .session()
            .acquire_child_thread(thread_id.clone())
            .await?;
        self.execute_locked_decision(command_id, "queued-run.pop", thread_id, now_iso, |state| {
            decide_pop_next_queued_run(state, command_id, thread_id, now_iso)
        })
    }

    /// Queries the current [`ShellSnapshot`] at `now_iso`.
    pub fn shell_snapshot(
        &self,
        now_iso: &str,
        options: ShellSnapshotOptions,
    ) -> Result<ShellSnapshot, OrchestratorError> {
        let conn = self.sink.db().lock().map_err(|_| SinkError::LockPoisoned)?;
        Ok(query_shell_snapshot(&conn, now_iso, options)?)
    }

    /// Runs 24-hour event-store pruning at `now_iso`.
    pub fn prune_event_store(
        &self,
        now_iso: &str,
    ) -> Result<PruneEventStoreReport, OrchestratorError> {
        let conn = self.sink.db().lock().map_err(|_| SinkError::LockPoisoned)?;
        Ok(prune_event_store(&conn, now_iso)?)
    }

    /// Loads current [`DeciderThreadState`] for `thread_id`.
    pub fn load_thread_state(
        &self,
        thread_id: &ThreadId,
    ) -> Result<DeciderThreadState, OrchestratorError> {
        let conn = self.sink.db().lock().map_err(|_| SinkError::LockPoisoned)?;
        Ok(load_decider_state(&conn, thread_id)?)
    }

    fn execute_locked_command(
        &self,
        command: &OrchestrationCommand,
        thread_id: &ThreadId,
        now_iso: &str,
    ) -> Result<DispatchResult, OrchestratorError> {
        self.execute_locked_decision(
            command.command_id(),
            command.command_type(),
            thread_id,
            now_iso,
            |state| decide_command(state, command, now_iso),
        )
    }

    fn execute_locked_decision<F>(
        &self,
        command_id: &CommandId,
        command_type: &str,
        thread_id: &ThreadId,
        now_iso: &str,
        decide: F,
    ) -> Result<DispatchResult, OrchestratorError>
    where
        F: FnOnce(&DeciderThreadState) -> Result<CommandDecision, DeciderError>,
    {
        let mut tx = self.sink.begin_tx()?;

        if let Some(existing) = load_receipt(tx.conn()?, command_id)? {
            return replay_existing_receipt(tx, existing, command_id.clone(), thread_id);
        }

        let state = load_decider_state(tx.conn()?, thread_id)?;
        let decision: CommandDecision = match decide(&state) {
            Ok(dec) => dec,
            Err(err) => {
                let msg = err.to_string();
                tx.commit_rejected_command(
                    command_id,
                    command_type,
                    thread_id.as_str(),
                    &msg,
                    now_iso,
                )?;
                tx.commit()?;
                return Err(OrchestratorError::Decider(err));
            }
        };

        let rollback_intent = decision.rollback_intent.clone();
        let resulting_thread = decision.resulting_thread.clone();
        let outcome = tx.commit_command(&decision.into_commit_input("server", now_iso))?;
        tx.commit()?;

        Ok(DispatchResult {
            outcome,
            rollback_intent,
            resulting_thread,
        })
    }
}
