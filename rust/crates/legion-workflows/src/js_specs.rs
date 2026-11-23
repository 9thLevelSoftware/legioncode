#![forbid(unsafe_code)]
//! JS workflow specification DTOs, pass outcome types, and engine definition converter.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::engine::{WorkflowBodyDriver, WorkflowDefinition};
use crate::engine_step::PassOutcome;
use crate::js_driver::JsDriver;
use crate::traits::BoxFuture;
use crate::triggers_hooks::{
    TriggerDef, WebhookKeyFn, check_triggers, daily_trigger, every_trigger, issues_opened_trigger,
    pull_requests_opened_trigger,
};
use crate::types::WorkflowDefinitionProblem;

/// Result returned by a single pass of `__legionDrivePass` or `__legionDriveOnEnd`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum JsDriveResult {
    Completed {
        #[serde(default)]
        value: Option<Value>,
    },
    Suspend {
        #[serde(default)]
        ops: Vec<JsSuspendedOp>,
    },
    Stop {
        reason: String,
    },
    Skip {
        reason: String,
    },
    Fault {
        error: String,
    },
    TargetStepDone {
        ok: bool,
        #[serde(default)]
        value: Option<Value>,
        #[serde(default)]
        error: Option<String>,
    },
}

/// Suspended durable operation yielded by the JS generator trampoline.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JsSuspendedOp {
    #[serde(rename_all = "camelCase")]
    Step {
        name: String,
        retry: JsRetry,
        #[serde(default)]
        schema: Option<Map<String, Value>>,
        #[serde(default)]
        capture_error: bool,
    },
    #[serde(rename_all = "camelCase")]
    Agent {
        name: String,
        opts: Value,
        #[serde(default)]
        try_mode: bool,
    },
    #[serde(rename_all = "camelCase")]
    Wait {
        name: String,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        schema: Map<String, Value>,
        #[serde(default)]
        timeout_ms: Option<i64>,
        #[serde(default)]
        codec_idx: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Ask {
        name: String,
        title: String,
        #[serde(default)]
        choices: Option<Vec<String>>,
        #[serde(default)]
        data: Option<Value>,
        #[serde(default)]
        timeout_ms: Option<i64>,
    },
    #[serde(rename_all = "camelCase")]
    Sleep { name: String, ms: i64 },
    #[serde(rename_all = "camelCase")]
    Title { value: String },
    #[serde(rename_all = "camelCase")]
    Invoke {
        name: String,
        workflow: String,
        #[serde(default)]
        version: Option<i64>,
        #[serde(default)]
        payload: Value,
        #[serde(default)]
        key: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PrWait {
        name: String,
        pr: Value,
        until: Value,
        #[serde(default)]
        timeout_ms: Option<i64>,
    },
    #[serde(rename_all = "camelCase")]
    StepAsync {
        #[serde(default)]
        step: String,
        #[serde(default)]
        seq: usize,
        cache_key: String,
        op: String,
        #[serde(default)]
        payload: Value,
    },
}

/// Normalized step retry descriptor exported from JS.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsRetry {
    #[serde(default)]
    pub times: usize,
    #[serde(default)]
    pub delay_ms: u64,
}

/// Compiled trigger specification returned by `__legionListWorkflowSpecs`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JsTriggerSpec {
    #[serde(rename_all = "camelCase")]
    Every { every_ms: i64 },
    #[serde(rename_all = "camelCase")]
    Daily {
        time_of_day: String,
        #[serde(default)]
        weekdays: Option<Vec<u8>>,
    },
    #[serde(rename_all = "camelCase")]
    Webhook {
        name: String,
        #[serde(default)]
        key_fn_index: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    IssuesOpened {
        project: String,
        #[serde(default)]
        labels: Vec<String>,
        #[serde(default)]
        query: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PullRequestsOpened {
        project: String,
        #[serde(default)]
        query: Option<String>,
    },
}

/// Compiled workflow metadata returned by `__legionListWorkflowSpecs`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsWorkflowSpec {
    pub name: String,
    pub version: i64,
    #[serde(default)]
    pub payload_schema: Map<String, Value>,
    #[serde(default)]
    pub returns_schema: Option<Map<String, Value>>,
    #[serde(default)]
    pub resume_after_ms: Vec<i64>,
    #[serde(default)]
    pub triggers: Vec<JsTriggerSpec>,
    #[serde(default)]
    pub has_on_end: bool,
}

/// Runtime backend implemented by both in-process `BoaHost` and external `JsWorkerHost`.
pub trait JsPassBackend: Send + Sync {
    /// Evaluates `__legionDrivePass` asynchronously.
    fn drive_pass(
        &self,
        wf_key: &str,
        input: &Value,
        journal: &Value,
        target_step: Option<&str>,
        step_ops: &Value,
        run_id: &str,
    ) -> BoxFuture<'_, Result<JsDriveResult, String>>;

    /// Evaluates `__legionDriveOnEnd` asynchronously.
    fn drive_on_end(
        &self,
        wf_key: &str,
        input: &Value,
        end_info: &Value,
        step_ops: &Value,
    ) -> BoxFuture<'_, Result<JsDriveResult, String>>;

    /// Evaluates a registered webhook `key` callback synchronously.
    fn call_webhook_key(&self, idx: usize, req: &Value) -> Result<Option<String>, String>;

    /// Evaluates a registered JS wait codec synchronously.
    fn check_wait_codec(&self, idx: usize, val: &Value) -> Result<Value, String>;
}

/// Converts evaluated JS specs into engine [`WorkflowDefinition`] entries.
pub fn specs_to_definitions(
    specs: Vec<JsWorkflowSpec>,
    backend: &Arc<dyn JsPassBackend>,
    file_name: &str,
) -> Result<Vec<WorkflowDefinition>, WorkflowDefinitionProblem> {
    let mut defs = Vec::with_capacity(specs.len());
    for spec in specs {
        let mut triggers = Vec::with_capacity(spec.triggers.len());
        for t in spec.triggers {
            let def = match t {
                JsTriggerSpec::Every { every_ms } => every_trigger(every_ms),
                JsTriggerSpec::Daily {
                    time_of_day,
                    weekdays,
                } => daily_trigger(&time_of_day, weekdays),
                JsTriggerSpec::Webhook { name, key_fn_index } => {
                    let key_fn: Option<WebhookKeyFn> = key_fn_index.map(|idx| {
                        let b = Arc::clone(backend);
                        let f: WebhookKeyFn = Arc::new(move |req| {
                            let val = serde_json::to_value(req).map_err(|e| e.to_string())?;
                            b.call_webhook_key(idx, &val)
                        });
                        f
                    });
                    Ok(TriggerDef::Webhook { name, key_fn })
                }
                JsTriggerSpec::IssuesOpened {
                    project,
                    labels,
                    query,
                } => Ok(issues_opened_trigger(project, labels, query)),
                JsTriggerSpec::PullRequestsOpened { project, query } => {
                    Ok(pull_requests_opened_trigger(project, query))
                }
            };
            match def {
                Ok(d) => triggers.push(d),
                Err(message) => {
                    return Err(WorkflowDefinitionProblem {
                        message,
                        file: Some(file_name.to_owned()),
                        line: None,
                    });
                }
            }
        }
        if let Err(message) = check_triggers(&spec.name, &triggers) {
            return Err(WorkflowDefinitionProblem {
                message,
                file: Some(file_name.to_owned()),
                line: None,
            });
        }
        let wf_key = format!("{}@{}", spec.name, spec.version);
        let driver: Arc<dyn WorkflowBodyDriver> =
            Arc::new(JsDriver::new(wf_key, Arc::clone(backend)));
        defs.push(WorkflowDefinition {
            name: spec.name,
            version: spec.version,
            payload_schema: spec.payload_schema,
            returns_schema: spec.returns_schema,
            resume_after_ms: spec.resume_after_ms,
            triggers,
            has_on_end: spec.has_on_end,
            driver,
        });
    }
    Ok(defs)
}

pub(crate) fn terminal_outcome_from_control(skipped: bool, reason: String) -> PassOutcome {
    PassOutcome::Stopped { skipped, reason }
}
