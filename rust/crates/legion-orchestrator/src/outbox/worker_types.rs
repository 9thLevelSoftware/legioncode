//! Executor trait, configuration, stats, error, and signal types for [`crate::outbox::EffectOutboxWorker`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;
use tokio::sync::Notify;

use crate::outbox::types::{OutboxEffect, OutboxStoreError};

/// Async handler executed by [`crate::outbox::EffectOutboxWorker`] for each claimed outbox row.
pub trait OutboxEffectExecutor: Send + Sync + 'static {
    /// Executes `effect`. Returning `Err` schedules a retry when `will_retry` is `true`, or fails permanently.
    fn execute<'a>(
        &'a self,
        effect: OutboxEffect,
        will_retry: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}

impl<F, Fut, E> OutboxEffectExecutor for F
where
    F: Fn(OutboxEffect) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), E>> + Send + 'static,
    E: fmt::Display,
{
    fn execute<'a>(
        &'a self,
        effect: OutboxEffect,
        _will_retry: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        let fut = (self)(effect);
        Box::pin(async move { fut.await.map_err(|err| err.to_string()) })
    }
}

/// Configuration for [`crate::outbox::EffectOutboxWorker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxWorkerOptions {
    /// Worker lease owner identifier stamped onto claimed rows.
    pub worker_id: String,
    /// Maximum execution attempts before transitioning a row to `failed`.
    pub max_attempts: u64,
    /// Retry delay in milliseconds (`0` executes immediate deterministic retries during `drain`).
    pub retry_delay_ms: i64,
}

impl Default for OutboxWorkerOptions {
    fn default() -> Self {
        Self {
            worker_id: format!("orchestration-v2:{}", std::process::id()),
            max_attempts: 5,
            retry_delay_ms: 0,
        }
    }
}

/// Counters summarizing [`crate::outbox::EffectOutboxWorker`] progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxWorkerStats {
    /// Currently executing in-flight effects.
    pub in_flight: usize,
    /// Total effects settled with `succeeded`.
    pub completed: usize,
    /// Total execution attempts that returned `Err`.
    pub failed_attempts: usize,
    /// Whether the worker has been closed or shut down.
    pub closed: bool,
}

/// Errors returned by [`crate::outbox::EffectOutboxWorker`] operations.
#[derive(Debug, Error)]
pub enum OutboxWorkerError {
    /// Outbox `SQLite` store error.
    #[error(transparent)]
    Store(#[from] OutboxStoreError),
    /// Worker was closed before draining completed.
    #[error("outbox worker was closed")]
    Closed,
}

/// Lock-free wakeup signal shared between [`crate::sink::EventSink`] and [`crate::outbox::EffectOutboxWorker`].
#[derive(Debug, Default)]
pub struct OutboxSignal {
    pub(crate) generation: AtomicU64,
    pub(crate) work_available: Notify,
    pub(crate) drained: Notify,
}

impl OutboxSignal {
    /// Creates a new [`OutboxSignal`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Signals that new outbox work may be claimable (must be called strictly after commit).
    pub fn notify_available(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.work_available.notify_waiters();
    }
}
