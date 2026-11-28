#![forbid(unsafe_code)]
//! Child workflow invocation (`invoke`) and durable agent step wrappers (`agent`, `agent.try`).

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::engine_agent::{
    AgentStepAttempt, AgentStepDeps, AgentStepOptions, AgentStepOutcome, AgentThreadRef,
    execute_agent_step,
};
use crate::engine_agent_turns::journaled_start;
use crate::engine_replay::ReplayContext;
use crate::engine_step::{ChildOutcome, PassControl, StepRetry};
use crate::engine_wait::{
    WaitOptions, WaitReplayOutcome, ask_schema_and_request, execute_wait_replay, pr_watch_schema,
};
use crate::traits::{AgentTurn, PullRequestAddress, PullRequestStanding, PullRequestUntil};
use crate::types::{RunParent, WorkflowError, WorkflowStepKind};

impl ReplayContext {
    /// Invokes a child workflow run and waits for its terminal outcome.
    pub async fn invoke(
        &mut self,
        name: &str,
        target_workflow: &str,
        target_version: Option<i64>,
        payload: Value,
        key: Option<String>,
    ) -> Result<ChildOutcome, PassControl> {
        self.claim(name, false)?;
        let start_step = format!("{name}/start");
        let idemp_key = key.unwrap_or_else(|| format!("{}/{name}", self.run_id));
        let engine = Arc::clone(&self.engine);
        let parent = RunParent {
            run_id: self.run_id.clone(),
            wait: name.to_owned(),
        };
        let wf = target_workflow.to_owned();
        let _child_id = self
            .run_step_internal(
                &start_step,
                None,
                StepRetry::default(),
                false,
                &mut |_sctx| {
                    let eng = Arc::clone(&engine);
                    let w = wf.clone();
                    let p = payload.clone();
                    let k = idemp_key.clone();
                    let par = parent.clone();
                    Box::pin(async move {
                        eng.start_run(&w, p, k, Some(par), target_version)
                            .await
                            .map(Value::String)
                            .map_err(|e| e.to_string())
                    })
                },
            )
            .await?;

        let schema = child_end_schema();
        let opts = WaitOptions {
            kind: WorkflowStepKind::Wait,
            name,
            channel: None,
            schema: &schema,
            timeout_ms: None,
            request: None,
            watch: None,
            codec: None,
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
            WaitReplayOutcome::Resolved(Some(val)) => {
                serde_json::from_value(val).map_err(|e| PassControl::Fault(e.to_string()))
            }
            WaitReplayOutcome::Resolved(None) => Err(PassControl::Fault(
                "the wait ended without an outcome".to_owned(),
            )),
            WaitReplayOutcome::Waiting => Err(PassControl::Suspend),
        }
    }

    /// Starts an agent turn non-blockingly (journaled start only) without awaiting turn settlement.
    pub async fn start_agent_turn(
        &mut self,
        name: &str,
        opts: &AgentStepOptions,
    ) -> Result<AgentTurn, PassControl> {
        self.claim(name, false)?;
        if opts.thread.is_some()
            && (opts.project.is_some()
                || opts.environment.is_some()
                || opts.model.is_some()
                || opts.title.is_some()
                || opts.launch.is_some())
        {
            return Err(PassControl::Fault(format!(
                "agent({name:?}) continues `thread`, which keeps how it started. Drop `project`/`environment`/`model`/`title`/`launch`."
            )));
        }
        let deps = AgentStepDeps {
            store: &self.store,
            journal: &self.journal,
            agents: &self.agents,
            forge: &self.forge,
            run_id: &self.run_id,
            workflow: &self.workflow,
            step_name: name,
            pause_on_failure: true,
        };
        let fut = journaled_start(&deps, opts, 0, &opts.brief, opts.thread.as_ref(), "start");
        self.in_flight
            .track(&self.run_id, fut)
            .await
            .map_err(|e| match e {
                WorkflowError::Input { problem } => PassControl::Fault(problem),
                other => PassControl::Fault(other.to_string()),
            })
    }

    /// Executes an `agent` step (pauses run on failure with `held = true`).
    pub async fn agent(
        &mut self,
        name: &str,
        opts: &AgentStepOptions,
    ) -> Result<(Value, AgentThreadRef), PassControl> {
        self.claim(name, false)?;
        let deps = AgentStepDeps {
            store: &self.store,
            journal: &self.journal,
            agents: &self.agents,
            forge: &self.forge,
            run_id: &self.run_id,
            workflow: &self.workflow,
            step_name: name,
            pause_on_failure: true,
        };
        let fut = execute_agent_step(&deps, opts);
        match self
            .in_flight
            .track(&self.run_id, fut)
            .await
            .map_err(|e| PassControl::Fault(e.to_string()))?
        {
            AgentStepOutcome::Finished(AgentStepAttempt::Done { value, thread }) => {
                Ok((value, thread))
            }
            AgentStepOutcome::Finished(AgentStepAttempt::Failed { reason, .. }) => {
                Err(PassControl::Fault(reason))
            }
            AgentStepOutcome::Suspended => Err(PassControl::Suspend),
        }
    }

    /// Executes `agent.try` returning failures as data.
    pub async fn agent_try(
        &mut self,
        name: &str,
        opts: &AgentStepOptions,
    ) -> Result<AgentStepAttempt, PassControl> {
        self.claim(name, false)?;
        let deps = AgentStepDeps {
            store: &self.store,
            journal: &self.journal,
            agents: &self.agents,
            forge: &self.forge,
            run_id: &self.run_id,
            workflow: &self.workflow,
            step_name: name,
            pause_on_failure: false,
        };
        let fut = execute_agent_step(&deps, opts);
        match self
            .in_flight
            .track(&self.run_id, fut)
            .await
            .map_err(|e| PassControl::Fault(e.to_string()))?
        {
            AgentStepOutcome::Finished(attempt) => Ok(attempt),
            AgentStepOutcome::Suspended => Err(PassControl::Suspend),
        }
    }

    /// Human-in-the-loop question `ask(name, title, choices, data, timeout_ms)`.
    pub fn ask(
        &mut self,
        name: &str,
        title: &str,
        choices: &[String],
        data: Option<&Value>,
        timeout_ms: Option<i64>,
    ) -> Result<Option<Value>, PassControl> {
        self.claim(name, false)?;
        let (schema, req) = ask_schema_and_request(title, choices, data);
        let opts = WaitOptions {
            kind: WorkflowStepKind::Ask,
            name,
            channel: None,
            schema: &schema,
            timeout_ms,
            request: Some(&req),
            watch: None,
            codec: None,
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

    /// Durable `pullRequests.wait(name, pr, until, timeout_ms)`.
    pub fn pull_requests_wait(
        &mut self,
        name: &str,
        pull_request: PullRequestAddress,
        until: PullRequestUntil,
        timeout_ms: Option<i64>,
    ) -> Result<Option<PullRequestStanding>, PassControl> {
        self.claim(name, false)?;
        let (schema, watch) = pr_watch_schema(until, pull_request);
        let opts = WaitOptions {
            kind: WorkflowStepKind::Wait,
            name,
            channel: None,
            schema: &schema,
            timeout_ms,
            request: None,
            watch: Some(&watch),
            codec: None,
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
            WaitReplayOutcome::Resolved(None) => Ok(None),
            WaitReplayOutcome::Resolved(Some(val)) => {
                let standing: PullRequestStanding =
                    serde_json::from_value(val).map_err(|e| PassControl::Fault(e.to_string()))?;
                Ok(Some(standing))
            }
            WaitReplayOutcome::Waiting => Err(PassControl::Suspend),
        }
    }

    /// Updates the run's title journaled under `title/<n>`.
    pub fn title(&mut self, value: &str) -> Result<(), PassControl> {
        let name = loop {
            let candidate = format!("title/{}", self.title_seq);
            self.title_seq += 1;
            if !self.used_names.contains(&candidate) {
                break candidate;
            }
        };
        self.claim(&name, false)?;
        let key = format!("title/{name}");
        if self
            .journal
            .get_entry(&self.run_id, &key)
            .map_err(|e| PassControl::Fault(e.to_string()))?
            .is_some()
        {
            return Ok(());
        }
        self.store
            .run_title(&self.run_id, value)
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        self.journal
            .record_activity_success(&self.run_id, &key, &Value::Null)
            .map_err(|e| PassControl::Fault(e.to_string()))?;
        Ok(())
    }
}

fn child_end_schema() -> Map<String, Value> {
    let mut props = Map::new();
    props.insert(
        "outcome".to_owned(),
        serde_json::json!({ "type": "string", "enum": ["completed", "stopped", "skipped", "cancelled"] }),
    );
    props.insert("value".to_owned(), serde_json::json!({}));
    props.insert("reason".to_owned(), serde_json::json!({ "type": "string" }));
    let mut map = Map::new();
    map.insert("type".to_owned(), Value::String("object".to_owned()));
    map.insert("properties".to_owned(), Value::Object(props));
    map.insert(
        "required".to_owned(),
        serde_json::json!(["outcome", "value"]),
    );
    map
}
