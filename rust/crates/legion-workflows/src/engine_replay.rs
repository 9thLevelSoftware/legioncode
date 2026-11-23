//! Deterministic workflow replay context executing journaled primitives from the top.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::engine_step::{
    END_STEP, EngineControl, InFlightTracker, PassControl, StepAttempt, StepExecContext, StepRetry,
};
use crate::engine_wait::{WaitCodecFn, WaitOptions, WaitReplayOutcome, execute_wait_replay};
use crate::journal::{JournalEntryStatus, WorkflowJournal};
use crate::schema::validate_json_schema_value;
use crate::store::WorkflowStore;
use crate::time::{format_millis_iso, now_epoch_ms, parse_iso_millis};
use crate::traits::{BoxFuture, WorkflowAgentDriver, WorkflowForgeDriver};
use crate::types::{WorkflowRunStatus, WorkflowStepKind, WorkflowStepStatus};

/// Replay context passed into workflow bodies.
pub struct ReplayContext {
    pub store: WorkflowStore,
    pub journal: WorkflowJournal,
    pub agents: Arc<dyn WorkflowAgentDriver>,
    pub forge: Arc<dyn WorkflowForgeDriver>,
    pub engine: Arc<dyn EngineControl>,
    pub in_flight: InFlightTracker,
    pub run_id: String,
    pub workflow: String,
    pub input: Value,
    pub(crate) used_names: HashSet<String>,
    pub(crate) title_seq: usize,
}

impl ReplayContext {
    /// Creates a fresh replay pass context for `run_id`.
    #[expect(
        clippy::too_many_arguments,
        reason = "replay pass bundles run state and injected runtime services"
    )]
    pub fn new(
        store: WorkflowStore,
        journal: WorkflowJournal,
        agents: Arc<dyn WorkflowAgentDriver>,
        forge: Arc<dyn WorkflowForgeDriver>,
        engine: Arc<dyn EngineControl>,
        in_flight: InFlightTracker,
        run_id: String,
        workflow: String,
        input: Value,
    ) -> Self {
        Self {
            store,
            journal,
            agents,
            forge,
            engine,
            in_flight,
            run_id,
            workflow,
            input,
            used_names: HashSet::new(),
            title_seq: 0,
        }
    }

    /// Claims a step name for this replay pass; rejects `"onEnd"` and duplicates.
    pub fn claim(&mut self, name: &str, cleanup: bool) -> Result<(), PassControl> {
        if name == END_STEP && !cleanup {
            return Err(PassControl::Fault(format!(
                "\"{END_STEP}\" is the name of workflow {}'s cleanup step. Name this step something else.",
                self.workflow
            )));
        }
        if !self.used_names.insert(name.to_owned()) {
            return Err(PassControl::Fault(format!(
                "Two steps in workflow {} are named \"{name}\". Names are unique in a run.",
                self.workflow
            )));
        }
        Ok(())
    }

    /// Builds a [`StepExecContext`] for `step_name`.
    pub fn step_context(&self, step_name: &str) -> StepExecContext {
        StepExecContext::new(
            self.store.clone(),
            Arc::clone(&self.forge),
            Arc::clone(&self.engine),
            self.run_id.clone(),
            step_name.to_owned(),
        )
    }

    /// Executes or replays a durable `step` (pausing the run if it fails after retries).
    pub async fn step<F>(
        &mut self,
        name: &str,
        returns_schema: Option<&Map<String, Value>>,
        retry: StepRetry,
        mut code: F,
    ) -> Result<Value, PassControl>
    where
        F: (for<'s> FnMut(&'s mut StepExecContext) -> BoxFuture<'s, Result<Value, String>>) + Send,
    {
        self.claim(name, false)?;
        self.run_step_internal(name, returns_schema, retry, false, &mut code)
            .await
    }

    /// Executes or replays `step.try`, returning failures as [`StepAttempt::Failed`].
    pub async fn step_try<F>(
        &mut self,
        name: &str,
        returns_schema: Option<&Map<String, Value>>,
        retry: StepRetry,
        mut code: F,
    ) -> Result<StepAttempt, PassControl>
    where
        F: (for<'s> FnMut(&'s mut StepExecContext) -> BoxFuture<'s, Result<Value, String>>) + Send,
    {
        self.claim(name, false)?;
        let val = self
            .run_step_internal(name, returns_schema, retry, true, &mut code)
            .await?;
        serde_json::from_value(val).map_err(|e| PassControl::Fault(e.to_string()))
    }

    pub(crate) async fn run_step_internal(
        &self,
        name: &str,
        returns_schema: Option<&Map<String, Value>>,
        retry: StepRetry,
        capture_error: bool,
        code: &mut (
                 dyn for<'s> FnMut(&'s mut StepExecContext) -> BoxFuture<'s, Result<Value, String>>
                     + Send
             ),
    ) -> Result<Value, PassControl> {
        let entry_key = format!("step/{name}");
        if let Some(entry) = self
            .journal
            .get_entry(&self.run_id, &entry_key)
            .map_err(|e| PassControl::Fault(e.to_string()))?
            && entry.status == JournalEntryStatus::Completed
            && let Some(raw) = entry.payload_json
        {
            return serde_json::from_str(&raw).map_err(|e| PassControl::Fault(e.to_string()));
        }

        let mut last_err = String::new();
        for attempt_idx in 0..=retry.times {
            self.store
                .step_started(&self.run_id, name, WorkflowStepKind::Step)
                .map_err(|e| PassControl::Fault(e.to_string()))?;
            let mut sctx = self.step_context(name);
            let res = self.in_flight.track(&self.run_id, code(&mut sctx)).await;
            match res {
                Ok(val) => {
                    if let Some(schema) = returns_schema
                        && let Err(err) = validate_json_schema_value(schema, &val, true)
                    {
                        last_err = format!("the result does not fit schema: {err}");
                        if attempt_idx < retry.times && retry.delay_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(retry.delay_ms))
                                .await;
                        }
                        continue;
                    }
                    let final_val = if capture_error {
                        serde_json::to_value(StepAttempt::Done { value: val })
                            .map_err(|e| PassControl::Fault(e.to_string()))?
                    } else {
                        val
                    };
                    self.store
                        .step_ended(&self.run_id, name, WorkflowStepStatus::Completed, None)
                        .map_err(|e| PassControl::Fault(e.to_string()))?;
                    let _ = self.store.step_result(&self.run_id, name, &final_val, None);
                    self.journal
                        .record_activity_success(&self.run_id, &entry_key, &final_val)
                        .map_err(|e| PassControl::Fault(e.to_string()))?;
                    return Ok(final_val);
                }
                Err(err) => {
                    last_err = err;
                    if attempt_idx < retry.times && retry.delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(retry.delay_ms)).await;
                    }
                }
            }
        }

        if capture_error {
            let attempt = StepAttempt::Failed { reason: last_err };
            let final_val =
                serde_json::to_value(&attempt).map_err(|e| PassControl::Fault(e.to_string()))?;
            self.store
                .step_ended(&self.run_id, name, WorkflowStepStatus::Completed, None)
                .map_err(|e| PassControl::Fault(e.to_string()))?;
            let _ = self.store.step_result(&self.run_id, name, &final_val, None);
            self.journal
                .record_activity_success(&self.run_id, &entry_key, &final_val)
                .map_err(|e| PassControl::Fault(e.to_string()))?;
            return Ok(final_val);
        }

        self.store
            .step_ended(
                &self.run_id,
                name,
                WorkflowStepStatus::Failed,
                Some(&last_err),
            )
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        let pause_msg = format!("{name}: {last_err}");
        self.store
            .run_paused(&self.run_id, &pause_msg, Some(false))
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        self.journal
            .record_activity_failure(&self.run_id, &entry_key, &pause_msg)
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        Err(PassControl::Suspend)
    }

    /// Durable `sleep(name, duration_ms)` surviving restarts.
    pub fn sleep(&mut self, name: &str, duration_ms: i64) -> Result<(), PassControl> {
        self.claim(name, false)?;
        let key = format!("sleep/{name}");
        if let Some(entry) = self
            .journal
            .get_entry(&self.run_id, &key)
            .map_err(|e| PassControl::Fault(e.to_string()))?
        {
            if entry.status == JournalEntryStatus::Completed {
                return Ok(());
            }
            if let Some(wake_iso) = entry.wake_at
                && let Some(wake_ms) = parse_iso_millis(&wake_iso)
                && now_epoch_ms() >= wake_ms
            {
                self.store
                    .step_ended(&self.run_id, name, WorkflowStepStatus::Completed, None)
                    .map_err(|e| PassControl::Fault(e.to_string()))?;
                self.store
                    .run_resumed(&self.run_id)
                    .map_err(|e| PassControl::Fault(e.to_string()))?;
                self.journal
                    .record_sleep_completed(&self.run_id, &key)
                    .map_err(|e| PassControl::Fault(e.to_string()))?;
                return Ok(());
            }
            return Err(PassControl::Suspend);
        }

        let wake_iso = format_millis_iso(now_epoch_ms().saturating_add(duration_ms.max(0)));
        self.store
            .step_started(&self.run_id, name, WorkflowStepKind::Sleep)
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        self.store
            .step_detail(&self.run_id, name, &wake_iso)
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        let detail = format!("sleep {name}");
        self.store
            .run_status(&self.run_id, WorkflowRunStatus::Waiting, Some(&detail))
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        self.journal
            .record_sleep_open(&self.run_id, &key, &wake_iso)
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        if duration_ms <= 0 {
            self.store
                .step_ended(&self.run_id, name, WorkflowStepStatus::Completed, None)
                .map_err(|e| PassControl::Fault(e.to_string()))?;
            self.store
                .run_resumed(&self.run_id)
                .map_err(|e| PassControl::Fault(e.to_string()))?;
            self.journal
                .record_sleep_completed(&self.run_id, &key)
                .map_err(|e| PassControl::Fault(e.to_string()))?;
            return Ok(());
        }
        Err(PassControl::Suspend)
    }

    /// Durable `wait(name, ...)` returning `None` if timed out or `Some(value)` if signalled.
    pub fn wait(
        &mut self,
        name: &str,
        schema: &Map<String, Value>,
        channel: Option<&str>,
        timeout_ms: Option<i64>,
        codec: Option<&WaitCodecFn>,
    ) -> Result<Option<Value>, PassControl> {
        self.claim(name, false)?;
        let opts = WaitOptions {
            kind: WorkflowStepKind::Wait,
            name,
            channel,
            schema,
            timeout_ms,
            request: None,
            watch: None,
            codec,
        };
        match execute_wait_replay(
            &self.store,
            &self.journal,
            &self.run_id,
            &self.workflow,
            &opts,
        )
        .map_err(|e| PassControl::Fault(e.to_string()))?
        {
            WaitReplayOutcome::Resolved(val) => Ok(val),
            WaitReplayOutcome::Waiting => Err(PassControl::Suspend),
        }
    }
}
