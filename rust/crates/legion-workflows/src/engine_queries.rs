#![forbid(unsafe_code)]
//! Read queries, client snapshots, KV endpoints, latest-version resolution, and [`EngineControl`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::engine::{WorkflowDefinition, WorkflowEngine};
use crate::engine_step::{END_MAX_TRIES, END_STEP, EngineControl};
use crate::traits::BoxFuture;
use crate::triggers_hooks::format_trigger_summary;
use crate::types::{
    RunParent, SignalAddress, SignalDelivery, StoredEntry, WorkflowCounts, WorkflowDefinitionState,
    WorkflowError, WorkflowHook, WorkflowQuestion, WorkflowRun, WorkflowRunDetail,
    WorkflowRunStatus, WorkflowRunsSnapshot, WorkflowSummary,
};

impl WorkflowEngine {
    /// Returns registered hook descriptors.
    pub fn hooks(&self) -> Result<Vec<WorkflowHook>, WorkflowError> {
        self.require_enabled()?;
        Ok(self
            .hooks
            .iter()
            .map(|h| WorkflowHook {
                workflow: h.workflow.clone(),
                name: h.name.clone(),
                path: h.path.clone(),
            })
            .collect())
    }

    /// Returns summaries of the newest version of each registered workflow.
    pub fn summaries(&self) -> Vec<WorkflowSummary> {
        compute_latest_defs(&self.definitions)
            .into_iter()
            .map(|def| WorkflowSummary {
                name: def.name,
                version: def.version,
                triggers: def.triggers.iter().map(format_trigger_summary).collect(),
            })
            .collect()
    }

    /// Returns unfinished `(workflow, version)` pairs including cancelled runs awaiting `onEnd`.
    pub fn unfinished_versions(&self) -> Result<Vec<(String, i64)>, WorkflowError> {
        let mut out = self.store.unfinished()?;
        for ending in self.store.runs_to_end(END_STEP, END_MAX_TRIES)? {
            if self
                .by_tag
                .get(&ending.run.engine_tag)
                .is_some_and(|d| d.has_on_end)
            {
                out.push((ending.run.workflow, ending.run.version));
            }
        }
        Ok(out)
    }

    /// Reads full detail (run, steps, references) for `run_id`.
    pub fn get(&self, run_id: &str) -> Result<WorkflowRunDetail, WorkflowError> {
        self.require_enabled()?;
        let row = self.require_run(run_id)?;
        let steps = self.store.steps(run_id)?;
        let references = self.store.references(run_id)?;
        Ok(WorkflowRunDetail {
            run: self.to_wire_run(&row),
            steps,
            references,
        })
    }

    /// Lists runs optionally filtered by status.
    pub fn list(
        &self,
        status: Option<WorkflowRunStatus>,
    ) -> Result<Vec<WorkflowRun>, WorkflowError> {
        self.require_enabled()?;
        Ok(self
            .store
            .runs(status)?
            .iter()
            .map(|r| self.to_wire_run(r))
            .collect())
    }

    /// Lists open human questions.
    pub fn questions(&self) -> Result<Vec<WorkflowQuestion>, WorkflowError> {
        self.require_enabled()?;
        self.store.questions()
    }

    /// Returns lightweight workflow badge counters.
    pub fn counts(&self) -> Result<WorkflowCounts, WorkflowError> {
        if !self.is_enabled() {
            return Ok(WorkflowCounts {
                needs_you: 0,
                active: 0,
                total: 0,
            });
        }
        self.store.counts()
    }

    /// Builds a [`WorkflowRunsSnapshot`] for clients.
    pub fn snapshot(
        &self,
        definition_state: Option<WorkflowDefinitionState>,
    ) -> Result<WorkflowRunsSnapshot, WorkflowError> {
        if !self.is_enabled() {
            return Ok(WorkflowRunsSnapshot {
                workflows: Vec::new(),
                runs: Vec::new(),
                questions: Vec::new(),
                definition: definition_state,
            });
        }
        Ok(WorkflowRunsSnapshot {
            workflows: self.summaries(),
            runs: self.list(None)?,
            questions: self.questions()?,
            definition: definition_state,
        })
    }

    /// Lists key-value store entries beginning with `prefix`.
    pub fn stored(&self, prefix: &str) -> Result<Vec<StoredEntry>, WorkflowError> {
        self.require_enabled()?;
        self.store.kv_list(prefix)
    }

    /// Sets a key in the shared workflow store.
    pub fn store_set(&self, key: &str, value: &Value) -> Result<(), WorkflowError> {
        self.require_active()?;
        self.store.kv_set(key, value)
    }

    /// Deletes a key from the shared workflow store.
    pub fn store_delete(&self, key: &str) -> Result<(), WorkflowError> {
        self.require_active()?;
        self.store.kv_delete(key)
    }

    /// Polls until `pred(&run)` holds for `run_id`.
    pub async fn wait_for(
        &self,
        run_id: &str,
        pred: impl Fn(&WorkflowRun) -> bool,
    ) -> Result<WorkflowRun, WorkflowError> {
        let mut rx = self.store.subscribe_changes();
        loop {
            if let Some(row) = self.store.run(run_id)? {
                let wire = self.to_wire_run(&row);
                if pred(&wire) {
                    return Ok(wire);
                }
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        }
    }
}

pub(crate) struct ArcEngineControl(pub(crate) Arc<WorkflowEngine>);

impl EngineControl for ArcEngineControl {
    fn start_run<'a>(
        &'a self,
        workflow: &'a str,
        input: Value,
        idempotency_key: String,
        parent: Option<RunParent>,
        version: Option<i64>,
    ) -> BoxFuture<'a, Result<String, WorkflowError>> {
        Box::pin(async move {
            self.0
                .start(workflow, input, &idempotency_key, parent, version)
                .await
        })
    }

    fn cancel_run<'a>(
        &'a self,
        run_id: &'a str,
        reason: Option<String>,
    ) -> BoxFuture<'a, Result<(), WorkflowError>> {
        Box::pin(async move { self.0.cancel(run_id, reason).await })
    }

    fn send_signal(
        &self,
        address: SignalAddress,
        value: Value,
        signal_id: String,
        ttl_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<SignalDelivery, WorkflowError>> {
        Box::pin(async move { self.0.signal(address, value, &signal_id, ttl_ms).await })
    }

    fn send_signal_all<'a>(
        &'a self,
        channel: &'a str,
        workflow: Option<&'a str>,
        value: Value,
    ) -> BoxFuture<'a, Result<usize, WorkflowError>> {
        Box::pin(async move { self.0.signal_all(channel, workflow, value).await })
    }
}

pub(crate) fn compute_latest_defs(defs: &[WorkflowDefinition]) -> Vec<WorkflowDefinition> {
    let mut best: HashMap<&str, &WorkflowDefinition> = HashMap::new();
    for def in defs {
        best.entry(def.name.as_str())
            .and_modify(|cur| {
                if def.version > cur.version {
                    *cur = def;
                }
            })
            .or_insert(def);
    }
    let mut names: Vec<&str> = best.keys().copied().collect();
    names.sort_unstable();
    names
        .into_iter()
        .filter_map(|n| best.get(n).map(|d| (*d).clone()))
        .collect()
}
