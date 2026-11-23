//! Step execution context, in-flight tracker, and engine control trait for replay steps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Notify;

use crate::store::WorkflowStore;
use crate::traits::{BoxFuture, WorkflowForgeDriver};
use crate::triggers_hooks::encode_uri_component;
use crate::types::{
    RunFilter, RunParent, RunSummary, SignalAddress, SignalDelivery, WorkflowError,
    WorkflowReferenceInput,
};

/// Reserved step name for `onEnd` cleanup.
pub const END_STEP: &str = "onEnd";
/// Maximum retries for cancelled `onEnd` cleanup.
pub const END_MAX_TRIES: i64 = 5;
/// Retry delays for failed cancelled-run `onEnd` attempts.
pub const END_BACKOFF_MS: [i64; 4] = [300_000, 1_200_000, 4_800_000, 18_000_000];

/// Result of `step.try(...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum StepAttempt {
    /// Step succeeded with JSON value.
    Done { value: Value },
    /// Step failed after retries.
    Failed { reason: String },
}

/// Retry configuration for a workflow `step` or `step.try`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StepRetry {
    /// Number of additional retries after the first failure (default `0`).
    pub times: usize,
    /// Optional delay between retries in milliseconds.
    pub delay_ms: u64,
}

/// Terminal info handed to `onEnd(payload, end)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEndInfo {
    /// Run ID.
    pub id: String,
    /// Outcome (`completed`, `stopped`, `skipped`, `cancelled`).
    pub outcome: String,
    /// Optional stop/skip/cancel reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Outcome of an invoked child workflow run (`invoke`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum ChildOutcome {
    /// Child completed with decoded result value.
    Completed { value: Value },
    /// Child ended via `stop`.
    Stopped { reason: String },
    /// Child ended via `skip`.
    Skipped { reason: String },
    /// Child was cancelled.
    Cancelled { reason: String },
}

/// Control-flow signal unwinding a replay pass when suspending or stopping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassControl {
    /// Run is paused on failure or waiting on a wait/sleep/child/agent.
    Suspend,
    /// Workflow called `stop(reason)` or `skip(reason)`.
    Stop { skipped: bool, reason: String },
    /// Unrecoverable body error outside a step (duplicate step name, invalid args, etc.).
    Fault(String),
}

/// Overall outcome of running one replay pass over a workflow body.
#[derive(Debug, Clone, PartialEq)]
pub enum PassOutcome {
    /// Body returned a result JSON value.
    Completed(Value),
    /// Body called `stop` or `skip`.
    Stopped { skipped: bool, reason: String },
    /// Body suspended on an open wait, sleep, agent, or paused step.
    Suspended,
    /// Body faulted outside a step.
    Faulted(String),
}

/// Tracks steps currently executing in-process so cancelled `onEnd` cleanup waits for them.
#[derive(Default, Clone)]
pub struct InFlightTracker {
    state: Arc<Mutex<HashMap<String, InFlightEntry>>>,
}

struct InFlightEntry {
    count: usize,
    notify: Arc<Notify>,
}

impl InFlightTracker {
    /// Returns `true` when any step of `run_id` is actively running in this process.
    pub fn busy(&self, run_id: &str) -> bool {
        self.state.lock().is_ok_and(|g| g.contains_key(run_id))
    }

    /// Waits until all in-process steps of `run_id` finish.
    pub async fn settled(&self, run_id: &str) {
        let notify = {
            let Ok(guard) = self.state.lock() else {
                return;
            };
            guard.get(run_id).map(|entry| Arc::clone(&entry.notify))
        };
        if let Some(n) = notify {
            n.notified().await;
        }
    }

    /// Tracks execution of `fut` for `run_id`, notifying waiters when the last active step drops.
    pub async fn track<F, T>(&self, run_id: &str, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let _guard = InFlightGuard::acquire(Arc::clone(&self.state), run_id);
        fut.await
    }
}

struct InFlightGuard {
    state: Arc<Mutex<HashMap<String, InFlightEntry>>>,
    run_id: String,
}

impl InFlightGuard {
    fn acquire(state: Arc<Mutex<HashMap<String, InFlightEntry>>>, run_id: &str) -> Self {
        if let Ok(mut guard) = state.lock() {
            let entry = guard
                .entry(run_id.to_owned())
                .or_insert_with(|| InFlightEntry {
                    count: 0,
                    notify: Arc::new(Notify::new()),
                });
            entry.count += 1;
        }
        Self {
            state,
            run_id: run_id.to_owned(),
        }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.state.lock()
            && let Some(entry) = guard.get_mut(&self.run_id)
        {
            entry.count = entry.count.saturating_sub(1);
            if entry.count == 0 {
                let notify = Arc::clone(&entry.notify);
                guard.remove(&self.run_id);
                notify.notify_waiters();
            }
        }
    }
}

/// Control callbacks exposed by [`WorkflowEngine`](crate::engine::WorkflowEngine) to steps and bodies.
pub trait EngineControl: Send + Sync {
    /// Starts or returns an existing run for `(workflow, idempotency_key)`.
    fn start_run<'a>(
        &'a self,
        workflow: &'a str,
        input: Value,
        idempotency_key: String,
        parent: Option<RunParent>,
        version: Option<i64>,
    ) -> BoxFuture<'a, Result<String, WorkflowError>>;

    /// Cancels `run_id`.
    fn cancel_run<'a>(
        &'a self,
        run_id: &'a str,
        reason: Option<String>,
    ) -> BoxFuture<'a, Result<(), WorkflowError>>;

    /// Sends or buffers a signal.
    fn send_signal(
        &self,
        address: SignalAddress,
        value: Value,
        signal_id: String,
        ttl_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<SignalDelivery, WorkflowError>>;

    /// Broadcasts a signal to open waits on `channel`.
    fn send_signal_all<'a>(
        &'a self,
        channel: &'a str,
        workflow: Option<&'a str>,
        value: Value,
    ) -> BoxFuture<'a, Result<usize, WorkflowError>>;
}

/// Services available inside a `step`, `step.try`, or `onEnd` body.
pub struct StepExecContext {
    pub store: WorkflowStore,
    pub forge: Arc<dyn WorkflowForgeDriver>,
    pub engine: Arc<dyn EngineControl>,
    pub run_id: String,
    pub step_name: String,
    sent_counts: HashMap<String, usize>,
}

impl StepExecContext {
    /// Creates a step execution context with per-step signal sequence numbering.
    pub fn new(
        store: WorkflowStore,
        forge: Arc<dyn WorkflowForgeDriver>,
        engine: Arc<dyn EngineControl>,
        run_id: impl Into<String>,
        step_name: impl Into<String>,
    ) -> Self {
        Self {
            store,
            forge,
            engine,
            run_id: run_id.into(),
            step_name: step_name.into(),
            sent_counts: HashMap::new(),
        }
    }

    /// Sends a signal with deterministic per-step ID numbering unless `explicit_id` is provided.
    pub async fn signal(
        &mut self,
        address: SignalAddress,
        value: Value,
        explicit_id: Option<String>,
        ttl_ms: Option<i64>,
    ) -> Result<SignalDelivery, WorkflowError> {
        let to = match &address {
            SignalAddress::Run { run_id, wait } => format!(
                "run/{}/{}",
                encode_uri_component(run_id),
                encode_uri_component(wait)
            ),
            SignalAddress::Channel { workflow, channel } => format!(
                "key/{}/{}",
                encode_uri_component(workflow),
                encode_uri_component(channel)
            ),
        };
        let count = self.sent_counts.entry(to.clone()).or_insert(0);
        let seq = *count;
        *count += 1;
        let id = explicit_id
            .unwrap_or_else(|| format!("{}/{}/signal/{to}/{seq}", self.run_id, self.step_name));
        self.engine.send_signal(address, value, id, ttl_ms).await
    }

    /// Broadcasts a signal to open waits without buffering.
    pub async fn signal_all(
        &self,
        channel: &str,
        workflow: Option<&str>,
        value: Value,
    ) -> Result<usize, WorkflowError> {
        self.engine.send_signal_all(channel, workflow, value).await
    }

    /// Starts another workflow run without waiting for it.
    pub async fn start(
        &self,
        workflow: &str,
        payload: Value,
        key: &str,
        version: Option<i64>,
    ) -> Result<String, WorkflowError> {
        self.engine
            .start_run(workflow, payload, key.to_owned(), None, version)
            .await
    }

    /// Cancels another run; refuses self-cancellation.
    pub async fn cancel(
        &self,
        target_run_id: &str,
        reason: Option<String>,
    ) -> Result<(), WorkflowError> {
        if target_run_id == self.run_id {
            return Err(WorkflowError::Input {
                problem: "A run cannot cancel itself. End it from its body with `stop`.".to_owned(),
            });
        }
        self.engine.cancel_run(target_run_id, reason).await
    }

    /// Queries runs matching `filter`.
    pub fn runs(&self, filter: &RunFilter) -> Result<Vec<RunSummary>, WorkflowError> {
        let rows = self.store.runs_where(filter)?;
        Ok(rows
            .into_iter()
            .map(|r| RunSummary {
                id: r.run_id,
                workflow: r.workflow,
                key: r.idempotency_key,
                title: r.title,
                status: r.status,
                detail: r.detail,
                started_at: r.started_at,
                ended_at: r.ended_at,
            })
            .collect())
    }

    /// Notes an external reference touched by this step (best-effort).
    pub fn note_reference(&self, reference: &WorkflowReferenceInput) {
        let _ = self
            .store
            .reference_noted(&self.run_id, Some(&self.step_name), reference);
    }
}
