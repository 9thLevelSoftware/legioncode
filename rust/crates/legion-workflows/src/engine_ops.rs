//! Async execution paths of [`WorkflowEngine`]: start, replay, signal, resume, cancel, sweep.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

use crate::engine::WorkflowEngine;
use crate::engine_replay::ReplayContext;
use crate::engine_step::PassOutcome;
use crate::schema::validate_json_schema_value;
use crate::store::RunRow;
use crate::traits::BoxFuture;
use crate::types::{RunParent, SignalAddress, SignalDelivery, WorkflowError, WorkflowRunStatus};

const CANCEL_WAIT_MS: u64 = 30_000;

impl WorkflowEngine {
    /// Starts a run under `(workflow, idempotency_key)` (deduplicating across versions) and runs
    /// its initial replay pass.
    pub async fn start(
        self: &Arc<Self>,
        workflow: &str,
        input: Value,
        idempotency_key: &str,
        parent: Option<RunParent>,
        version: Option<i64>,
    ) -> Result<String, WorkflowError> {
        self.require_active()?;
        let def = self.resolve_def(workflow, version)?;
        if let Some(earlier) = self.store.run_by_key(workflow, idempotency_key)?
            && (earlier.engine_tag != def.engine_tag() || earlier.ended_at.is_some())
        {
            return self
                .handle_existing_key(&earlier, workflow, parent.as_ref())
                .await;
        }
        if let Err(err) = validate_json_schema_value(&def.payload_schema, &input, true) {
            return Err(WorkflowError::Input {
                problem: format!("The payload does not fit workflow {workflow}: {err}"),
            });
        }
        let run_id = Uuid::new_v4().to_string();
        self.store.run_started(&crate::store::NewRunRecord {
            run_id: run_id.clone(),
            workflow: def.name.clone(),
            version: def.version,
            engine_tag: def.engine_tag(),
            idempotency_key: idempotency_key.to_owned(),
            input: input.clone(),
            parent: parent.clone(),
        })?;
        let Some(holder) = self.store.run_by_key(workflow, idempotency_key)? else {
            return Err(WorkflowError::Storage {
                message: "inserted workflow run disappeared".to_owned(),
            });
        };
        if holder.run_id != run_id {
            return self
                .handle_existing_key(&holder, workflow, parent.as_ref())
                .await;
        }
        if let Some(ref expected_parent) = parent
            && (holder.parent_run_id.as_deref() != Some(expected_parent.run_id.as_str())
                || holder.parent_wait.as_deref() != Some(expected_parent.wait.as_str()))
        {
            return Err(WorkflowError::Input {
                problem: format!(
                    "Run {run_id} of {workflow} was started by something else under this key, so it cannot be invoked. Give the invoke a key of its own."
                ),
            });
        }
        self.drive_run(&run_id).await?;
        Ok(run_id)
    }

    async fn handle_existing_key(
        self: &Arc<Self>,
        earlier: &RunRow,
        workflow: &str,
        parent: Option<&RunParent>,
    ) -> Result<String, WorkflowError> {
        if let Some(expected_parent) = parent
            && (earlier.parent_run_id.as_deref() != Some(expected_parent.run_id.as_str())
                || earlier.parent_wait.as_deref() != Some(expected_parent.wait.as_str()))
        {
            return Err(WorkflowError::Input {
                problem: format!(
                    "Run {} of {workflow} was started by something else under this key, so it cannot be invoked. Give the invoke a key of its own.",
                    earlier.run_id
                ),
            });
        }
        if earlier.ended_at.is_none()
            && earlier.status != WorkflowRunStatus::Paused
            && self.by_tag.contains_key(&earlier.engine_tag)
        {
            let _ = self.drive_run(&earlier.run_id).await;
        }
        Ok(earlier.run_id.clone())
    }

    /// Replays an open non-paused run from the top against its persisted journal.
    pub async fn drive_run(self: &Arc<Self>, run_id: &str) -> Result<(), WorkflowError> {
        if !self.is_enabled() {
            return Ok(());
        }
        let run_lock = self.lock_for_run(run_id);
        let _guard = run_lock.lock().await;
        let Some(row) = self.store.run(run_id)? else {
            return Ok(());
        };
        if row.ended_at.is_some() || row.status == WorkflowRunStatus::Paused {
            return Ok(());
        }
        let Some(def) = self.by_tag.get(&row.engine_tag).cloned() else {
            return Ok(());
        };
        let input: Value = row
            .input_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let mut ctx = ReplayContext::new(
            self.store.clone(),
            self.journal.clone(),
            Arc::clone(&self.agents),
            Arc::clone(&self.forge),
            self.control_handle(),
            self.in_flight.clone(),
            row.run_id.clone(),
            row.workflow.clone(),
            input.clone(),
        );
        let outcome = def.driver.run_pass(&mut ctx).await;
        if self
            .store
            .run(run_id)?
            .is_some_and(|cur| cur.status == WorkflowRunStatus::Cancelled)
        {
            return Ok(());
        }
        match outcome {
            PassOutcome::Suspended => Ok(()),
            PassOutcome::Faulted(reason) => {
                self.store.run_paused(run_id, &reason, None)?;
                Ok(())
            }
            PassOutcome::Completed(raw_val) => {
                if let Some(ref schema) = def.returns_schema
                    && let Err(err) = validate_json_schema_value(schema, &raw_val, true)
                {
                    let msg = format!("The run's result does not fit its returns: {err}");
                    self.store.run_paused(run_id, &msg, None)?;
                    return Ok(());
                }
                if !self
                    .run_on_end_in_body(&mut ctx, &def, input, "completed", None)
                    .await?
                {
                    return Ok(());
                }
                self.store
                    .run_ended(run_id, WorkflowRunStatus::Completed, None, &raw_val)?;
                Box::pin(self.notify_parent_ended(run_id)).await;
                Ok(())
            }
            PassOutcome::Stopped { skipped, reason } => {
                let status = if skipped {
                    WorkflowRunStatus::Skipped
                } else {
                    WorkflowRunStatus::Stopped
                };
                if !self
                    .run_on_end_in_body(&mut ctx, &def, input, status.as_str(), Some(&reason))
                    .await?
                {
                    return Ok(());
                }
                self.store
                    .run_ended(run_id, status, Some(&reason), &Value::Null)?;
                Box::pin(self.notify_parent_ended(run_id)).await;
                Ok(())
            }
        }
    }

    /// Delivers or buffers a signal and immediately replays any woken open run.
    pub async fn signal(
        self: &Arc<Self>,
        address: SignalAddress,
        value: Value,
        signal_id: &str,
        ttl_ms: Option<i64>,
    ) -> Result<SignalDelivery, WorkflowError> {
        self.require_active()?;
        let (delivery, woken_wait) = self
            .store
            .signal_or_buffer(&address, &value, signal_id, ttl_ms)?;
        if let Some(wait) = woken_wait
            && !self.is_paused_or_resuming(&wait.run_id)?
        {
            self.drive_run(&wait.run_id).await?;
        }
        Ok(delivery)
    }

    /// Broadcasts `value` to open waits on `channel` without buffering.
    pub async fn signal_all(
        self: &Arc<Self>,
        channel: &str,
        workflow: Option<&str>,
        value: Value,
    ) -> Result<usize, WorkflowError> {
        self.require_active()?;
        let woken = self.store.signal_all_open(channel, workflow, &value)?;
        let count = woken.len();
        for wait in woken {
            if !self.is_paused_or_resuming(&wait.run_id)? {
                self.drive_run(&wait.run_id).await?;
            }
        }
        Ok(count)
    }

    /// Answers a human question (`ask`) by run ID and step name.
    pub async fn answer(
        self: &Arc<Self>,
        run_id: &str,
        name: &str,
        value: Value,
    ) -> Result<(), WorkflowError> {
        self.require_active()?;
        let addr = SignalAddress::Run {
            run_id: run_id.to_owned(),
            wait: name.to_owned(),
        };
        let Some(wait) = self.store.wait_at(&addr)? else {
            return Ok(());
        };
        if wait.request_json.is_none() {
            return Ok(());
        }
        let sid = format!("answer:{}", wait.token);
        let _ = self.signal(addr, value, &sid, None).await?;
        Ok(())
    }

    /// Resumes a paused run, replaying from the top while skipping previously completed steps.
    pub async fn resume(self: &Arc<Self>, run_id: &str) -> Result<(), WorkflowError> {
        self.require_active()?;
        self.resume_internal(run_id, None).await?;
        Ok(())
    }

    pub(crate) async fn resume_internal(
        self: &Arc<Self>,
        run_id: &str,
        expected_pause: Option<(i64, String)>,
    ) -> Result<bool, WorkflowError> {
        let row = self.require_run(run_id)?;
        if !self.by_tag.contains_key(&row.engine_tag) {
            return Err(WorkflowError::NotRegistered {
                workflow: row.engine_tag,
                version: None,
            });
        }
        if row.ended_at.is_some() {
            return Ok(false);
        }
        if let Ok(mut g) = self.resuming.lock() {
            g.insert(run_id.to_owned());
        }
        let exp_ref = expected_pause.as_ref().map(|(p, u)| (*p, u.as_str()));
        let proceeded = if row.status == WorkflowRunStatus::Paused {
            let unpaused = self.store.run_unpaused(run_id, exp_ref)?;
            if !unpaused && expected_pause.is_some() {
                false
            } else {
                self.journal.clear_failed_entries(run_id)?;
                self.store.run_unheld(run_id)?;
                true
            }
        } else {
            self.journal.clear_failed_entries(run_id)?;
            self.store.run_unheld(run_id)?;
            true
        };
        if let Ok(mut g) = self.resuming.lock() {
            g.remove(run_id);
        }
        if proceeded {
            self.drive_run(run_id).await?;
        }
        Ok(proceeded)
    }

    /// Cancels a run, stops its running agent turns and child runs, and executes `onEnd` cleanup.
    pub fn cancel<'a>(
        self: &'a Arc<Self>,
        run_id: &'a str,
        reason: Option<String>,
    ) -> BoxFuture<'a, Result<(), WorkflowError>> {
        Box::pin(async move {
            self.require_active()?;
            let row = self.require_run(run_id)?;
            if row.ended_at.is_some() {
                return Ok(());
            }
            let threads = self.store.running_threads(run_id)?;
            if let Ok(mut g) = self.cancelling.lock() {
                g.insert(run_id.to_owned());
            }
            let cancelled = self.store.run_ended(
                run_id,
                WorkflowRunStatus::Cancelled,
                reason.as_deref(),
                &Value::Null,
            )?;
            if cancelled {
                Box::pin(self.notify_parent_ended(run_id)).await;
                self.stop_steps(run_id, &threads).await;
                for child in self.store.children(run_id)? {
                    let _ = self.cancel(&child.run_id, reason.clone()).await;
                }
                let mut row_with_reason = row.clone();
                row_with_reason.detail = reason;
                let _ = tokio::time::timeout(
                    Duration::from_millis(CANCEL_WAIT_MS),
                    self.end_cancelled(&row_with_reason),
                )
                .await;
            }
            if let Ok(mut g) = self.cancelling.lock() {
                g.remove(run_id);
            }
            Ok(())
        })
    }
}
