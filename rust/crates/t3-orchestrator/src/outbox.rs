//! Durable outbox effect worker with deterministic `drain().await` (zero sleeps/polling).
//!
//! Mirrors `apps/server/src/orchestration-v2/EffectOutbox.ts` and `EffectWorker.ts`:
//! - Claims pending effects per-thread FIFO from `orchestration_v2_effect_outbox`.
//! - Coordinates wakeups via [`tokio::sync::Notify`] + monotonic generation counter so
//!   [`EffectOutboxWorker::drain`] waits deterministically with zero sleeps or polling.
//! - Retries transient failures up to `max_attempts` and isolates per-effect failures.
//!
//! # Examples
//!
//! ```
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::{Arc, Mutex};
//! use t3_contracts::ids::{CommandId, RunId, ThreadId};
//! use t3_orchestrator::outbox::{
//!     EffectOutboxWorker, OutboxEffect, OutboxEffectRequest, OutboxWorkerOptions,
//!     PendingOutboxEffect, enqueue_effects,
//! };
//! use t3_persistence::sqlite::SqliteStore;
//!
//! let store = SqliteStore::open_in_memory()?;
//! let db = Arc::new(Mutex::new(store.into_connection()));
//! let executed = Arc::new(Mutex::new(Vec::<String>::new()));
//! let log = Arc::clone(&executed);
//! let worker = EffectOutboxWorker::spawn(
//!     Arc::clone(&db),
//!     move |effect: OutboxEffect| {
//!         let log = Arc::clone(&log);
//!         async move {
//!             log.lock().map_err(|e| e.to_string())?.push(effect.id);
//!             Ok::<(), String>(())
//!         }
//!     },
//!     OutboxWorkerOptions::default(),
//! );
//! {
//!     let conn = db.lock().map_err(|e| e.to_string())?;
//!     enqueue_effects(
//!         &conn,
//!         &[PendingOutboxEffect {
//!             id: "eff-1".to_owned(),
//!             command_id: CommandId::new("cmd-1")?,
//!             thread_id: ThreadId::new("thread-1")?,
//!             request: OutboxEffectRequest::ProviderTurnStart {
//!                 run_id: RunId::new("run-1")?,
//!             },
//!             available_at: None,
//!         }],
//!         "2026-11-20T00:00:00.000Z",
//!     )?;
//! }
//! worker.notify_available();
//! let stats = worker.drain().await?;
//! assert_eq!(stats.completed, 1);
//! assert_eq!(*executed.lock().map_err(|e| e.to_string())?, vec!["eff-1"]);
//! worker.shutdown().await?;
//! # Ok(())
//! # }
//! ```

pub mod maintenance;
pub mod store;
pub mod types;
mod worker_loop;
pub mod worker_types;

use std::fmt;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tokio::task::JoinHandle;

pub use maintenance::{prune_settled_effects, reconcile_after_process_loss};
pub use store::{cancel_unsettled_effects, enqueue_effects, get_effect, list_by_command_id};
pub use types::{
    OutboxEffect, OutboxEffectRequest, OutboxStatus, OutboxStoreError, PROCESS_BOUND_EFFECT_TYPES,
    PendingOutboxEffect, REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS, ROLLBACK_PENDING_INTENT,
};
use worker_loop::{
    CoordState, WorkerInner, execute_and_settle, lock_unpoison, stats_from, worker_loop,
};
pub use worker_types::{
    OutboxEffectExecutor, OutboxSignal, OutboxWorkerError, OutboxWorkerOptions, OutboxWorkerStats,
};

use crate::time::now_iso_utc;

struct LoopGuard {
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for LoopGuard {
    fn drop(&mut self) {
        if let Some(handle) = lock_unpoison(&self.handle).take() {
            handle.abort();
        }
    }
}

/// Deterministic `SQLite`-backed outbox effect worker with zero-sleep `drain().await`.
#[derive(Clone)]
pub struct EffectOutboxWorker {
    inner: Arc<WorkerInner>,
    loop_guard: Option<Arc<LoopGuard>>,
}

impl fmt::Debug for EffectOutboxWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EffectOutboxWorker")
            .field("worker_id", &self.inner.worker_id)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl EffectOutboxWorker {
    /// Creates an [`EffectOutboxWorker`] with a background Tokio loop driven purely by `Notify`.
    pub fn spawn<E: OutboxEffectExecutor>(
        db: Arc<Mutex<Connection>>,
        executor: E,
        options: OutboxWorkerOptions,
    ) -> Self {
        Self::spawn_with_signal(db, executor, options, Arc::new(OutboxSignal::new()))
    }

    /// Creates a spawning [`EffectOutboxWorker`] wired to an externally shared [`OutboxSignal`].
    pub fn spawn_with_signal<E: OutboxEffectExecutor>(
        db: Arc<Mutex<Connection>>,
        executor: E,
        options: OutboxWorkerOptions,
        signal: Arc<OutboxSignal>,
    ) -> Self {
        let inner = make_inner(db, executor, options, signal);
        let handle = tokio::spawn(worker_loop(Arc::clone(&inner)));
        Self {
            inner,
            loop_guard: Some(Arc::new(LoopGuard {
                handle: Mutex::new(Some(handle)),
            })),
        }
    }

    /// Creates a manual (non-spawning) [`EffectOutboxWorker`] whose `drain().await` drives work inline.
    pub fn new_manual<E: OutboxEffectExecutor>(
        db: Arc<Mutex<Connection>>,
        executor: E,
        options: OutboxWorkerOptions,
    ) -> Self {
        Self::new_manual_with_signal(db, executor, options, Arc::new(OutboxSignal::new()))
    }

    /// Creates a manual [`EffectOutboxWorker`] wired to an externally shared [`OutboxSignal`].
    pub fn new_manual_with_signal<E: OutboxEffectExecutor>(
        db: Arc<Mutex<Connection>>,
        executor: E,
        options: OutboxWorkerOptions,
        signal: Arc<OutboxSignal>,
    ) -> Self {
        Self {
            inner: make_inner(db, executor, options, signal),
            loop_guard: None,
        }
    }

    /// Returns the shared [`OutboxSignal`] to pass to [`crate::sink::EventSink`].
    pub fn signal(&self) -> Arc<OutboxSignal> {
        Arc::clone(&self.inner.signal)
    }

    /// Wakes the background worker after new outbox rows are committed.
    pub fn notify_available(&self) {
        self.inner.signal.notify_available();
    }

    /// Attempts to claim and execute at most one claimable outbox row. Returns `true` if one ran.
    pub async fn run_once(&self) -> Result<bool, OutboxWorkerError> {
        let now_iso = now_iso_utc();
        let claimed = store::with_locked_conn(&self.inner.db, |conn| {
            store::claim_next_effect(conn, &self.inner.worker_id, &now_iso)
        })?;
        let Some(effect) = claimed else {
            return Ok(false);
        };
        lock_unpoison(&self.inner.coord).in_flight += 1;
        execute_and_settle(&self.inner, effect).await?;
        let mut st = lock_unpoison(&self.inner.coord);
        st.in_flight = st.in_flight.saturating_sub(1);
        if st.in_flight == 0 {
            self.inner.signal.drained.notify_waiters();
        }
        Ok(true)
    }

    /// Deterministically waits until all currently claimable and in-flight effects have settled.
    ///
    /// Uses pre-registered [`tokio::sync::Notify`] notifications with zero sleeps or polling.
    pub async fn drain(&self) -> Result<OutboxWorkerStats, OutboxWorkerError> {
        if self.loop_guard.is_none() {
            while self.run_once().await? {}
            return Ok(self.stats());
        }
        loop {
            let mut drained_notified = std::pin::pin!(self.inner.signal.drained.notified());
            let claimable = {
                let st = lock_unpoison(&self.inner.coord);
                if st.closed {
                    return Err(OutboxWorkerError::Closed);
                }
                let has_work = store::with_locked_conn(&self.inner.db, |conn| {
                    store::has_claimable_work(conn, &now_iso_utc())
                })?;
                if st.in_flight == 0 && !has_work {
                    return Ok(stats_from(&st));
                }
                drained_notified.as_mut().enable();
                has_work
            };
            if claimable {
                self.inner.signal.notify_available();
            }
            drained_notified.await;
        }
    }

    /// Drains remaining work, stops the background loop, and waits for its task to exit.
    pub async fn shutdown(self) -> Result<OutboxWorkerStats, OutboxWorkerError> {
        let stats = self.drain().await?;
        {
            let mut st = lock_unpoison(&self.inner.coord);
            st.accepting = false;
            st.closed = true;
        }
        self.inner.signal.notify_available();
        if let Some(guard) = self.loop_guard {
            let handle = lock_unpoison(&guard.handle).take();
            if let Some(handle) = handle {
                let _ = handle.await;
            }
        }
        Ok(stats)
    }

    /// Returns a snapshot of worker counters.
    pub fn stats(&self) -> OutboxWorkerStats {
        stats_from(&lock_unpoison(&self.inner.coord))
    }
}

fn make_inner<E: OutboxEffectExecutor>(
    db: Arc<Mutex<Connection>>,
    executor: E,
    options: OutboxWorkerOptions,
    signal: Arc<OutboxSignal>,
) -> Arc<WorkerInner> {
    Arc::new(WorkerInner {
        db,
        executor: Arc::new(executor),
        worker_id: options.worker_id,
        max_attempts: options.max_attempts.max(1),
        retry_delay_ms: options.retry_delay_ms.max(0),
        signal,
        coord: Mutex::new(CoordState {
            in_flight: 0,
            completed: 0,
            failed_attempts: 0,
            accepting: true,
            closed: false,
        }),
    })
}
