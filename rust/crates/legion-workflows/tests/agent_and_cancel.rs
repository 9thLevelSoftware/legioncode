#![forbid(unsafe_code)]
//! Agent step tests: quiet window forwarding, validation correction, turn-scoped cancellation, onEnd.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use legion_workflows::engine::{WorkflowBodyDriver, WorkflowDefinition, WorkflowEngine};
use legion_workflows::engine_agent::{AgentStepAttempt, AgentStepOptions};
use legion_workflows::engine_replay::ReplayContext;
use legion_workflows::engine_step::{PassControl, PassOutcome, RunEndInfo, StepExecContext};
use legion_workflows::schema::ensure_workflow_and_feedback_schema;
use legion_workflows::store::WorkflowStore;
use legion_workflows::traits::{
    AgentEnding, AgentInterruptOutcome, AgentInterruptTarget, AgentResult, AgentStartInput,
    AgentTurn, BoxFuture, UnavailableForgeDriver, WorkflowAgentDriver, WorkflowAgentError,
};
use legion_workflows::types::WorkflowRunStatus;
use rusqlite::Connection;
use serde_json::{Map, Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn open_store() -> Result<WorkflowStore, Box<dyn std::error::Error>> {
    let conn = Arc::new(Mutex::new(Connection::open_in_memory()?));
    {
        let g = conn.lock().map_err(|_| "lock poisoned")?;
        ensure_workflow_and_feedback_schema(&g)?;
    }
    Ok(WorkflowStore::new(conn)?)
}

struct MockAgentDriver {
    quiet_seen: Mutex<Vec<Option<i64>>>,
    interrupts: Mutex<Vec<AgentInterruptTarget>>,
    always_invalid_output: bool,
    wait_notify: Option<Arc<tokio::sync::Notify>>,
    started_notify: Option<Arc<tokio::sync::Notify>>,
}

impl WorkflowAgentDriver for MockAgentDriver {
    fn start(
        &self,
        input: AgentStartInput,
    ) -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>> {
        Box::pin(async move {
            let tid = input.thread.unwrap_or_else(|| "thread-1".to_owned());
            if let Some(n) = &self.started_notify {
                n.notify_waiters();
            }
            Ok(AgentTurn {
                thread_id: tid,
                turn_id: format!("turn-{}", input.key),
                environment: input.environment,
                started_at: Some(1_000),
                failed: None,
            })
        })
    }

    fn result(
        &self,
        _turn: AgentTurn,
        quiet_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<AgentResult, WorkflowAgentError>> {
        Box::pin(async move {
            if let Ok(mut g) = self.quiet_seen.lock() {
                g.push(quiet_ms);
            }
            if let Some(n) = &self.wait_notify {
                n.notified().await;
                return Ok(AgentResult {
                    ending: AgentEnding::Stopped,
                    detail: "cancelled".to_owned(),
                    reply: String::new(),
                    output: None,
                });
            }
            let output = if self.always_invalid_output {
                Some(json!({ "wrong": 1 }))
            } else {
                Some(json!({ "ok": true }))
            };
            Ok(AgentResult {
                ending: AgentEnding::Completed,
                detail: String::new(),
                reply: "done".to_owned(),
                output,
            })
        })
    }

    fn interrupt(
        &self,
        target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>> {
        Box::pin(async move {
            if let Ok(mut g) = self.interrupts.lock() {
                g.push(target);
            }
            if let Some(n) = &self.wait_notify {
                n.notify_waiters();
            }
            Ok(AgentInterruptOutcome::Stopped)
        })
    }
}

struct AgentWorkflowDriver {
    quiet_ms: Option<i64>,
    on_end_called: Arc<AtomicBool>,
}

impl WorkflowBodyDriver for AgentWorkflowDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            let schema = json!({
                "type": "object",
                "properties": { "ok": { "type": "boolean" } },
                "required": ["ok"],
                "additionalProperties": false
            })
            .as_object()
            .cloned()
            .unwrap_or_default();
            let opts = AgentStepOptions {
                brief: "check".to_owned(),
                title: None,
                returns_schema: Some(schema),
                model: None,
                project: None,
                environment: None,
                thread: None,
                issue: None,
                launch: None,
                timeout_ms: None,
                quiet_ms: self.quiet_ms,
            };
            match ctx.agent_try("try_agent", &opts).await {
                Ok(AgentStepAttempt::Failed { reason, .. }) => {
                    PassOutcome::Completed(json!({ "failed": reason }))
                }
                Ok(AgentStepAttempt::Done { value, thread }) => {
                    match ctx.agent("main_agent", &opts).await {
                        Ok((v, _)) => PassOutcome::Completed(
                            json!({ "try": value, "main": v, "tid": thread.thread_id }),
                        ),
                        Err(PassControl::Suspend) => PassOutcome::Suspended,
                        Err(e) => PassOutcome::Faulted(format!("{e:?}")),
                    }
                }
                Err(PassControl::Suspend) => PassOutcome::Suspended,
                Err(e) => PassOutcome::Faulted(format!("{e:?}")),
            }
        })
    }

    fn run_on_end<'a>(
        &'a self,
        _ctx: &'a mut StepExecContext,
        _payload: Value,
        _end: RunEndInfo,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.on_end_called.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn agent_quiet_window_corrections_and_cancel_turn_scoped_interrupt() -> TestResult {
    let store = open_store()?;
    let mock = Arc::new(MockAgentDriver {
        quiet_seen: Mutex::new(Vec::new()),
        interrupts: Mutex::new(Vec::new()),
        always_invalid_output: true,
        wait_notify: None,
        started_notify: None,
    });
    let on_end_called = Arc::new(AtomicBool::new(false));
    let def = WorkflowDefinition {
        name: "auditor".to_owned(),
        version: 1,
        payload_schema: Map::new(),
        returns_schema: None,
        resume_after_ms: vec![1_000],
        triggers: Vec::new(),
        has_on_end: true,
        driver: Arc::new(AgentWorkflowDriver {
            quiet_ms: Some(15_000),
            on_end_called: Arc::clone(&on_end_called),
        }),
    };
    let engine = WorkflowEngine::new(
        store.clone(),
        vec![def],
        mock.clone(),
        Arc::new(UnavailableForgeDriver),
        true,
    )?;
    let run_id = engine.start("auditor", json!({}), "a1", None, None).await?;
    let detail = engine.get(&run_id)?;
    assert_eq!(detail.run.status, WorkflowRunStatus::Completed);
    assert!(
        detail
            .run
            .result
            .as_ref()
            .and_then(|v| v.get("failed"))
            .and_then(Value::as_str)
            .is_some()
    );
    assert_eq!(mock.quiet_seen.lock().map_err(|_| "lock")?[0], Some(15_000));
    assert!(on_end_called.load(Ordering::SeqCst));

    let wait_notify = Arc::new(tokio::sync::Notify::new());
    let started_notify = Arc::new(tokio::sync::Notify::new());
    let cancel_mock = Arc::new(MockAgentDriver {
        quiet_seen: Mutex::new(Vec::new()),
        interrupts: Mutex::new(Vec::new()),
        always_invalid_output: false,
        wait_notify: Some(Arc::clone(&wait_notify)),
        started_notify: Some(Arc::clone(&started_notify)),
    });
    let on_end_cancel = Arc::new(AtomicBool::new(false));
    let def2 = WorkflowDefinition {
        name: "cancelable".to_owned(),
        version: 1,
        payload_schema: Map::new(),
        returns_schema: None,
        resume_after_ms: vec![],
        triggers: Vec::new(),
        has_on_end: true,
        driver: Arc::new(AgentWorkflowDriver {
            quiet_ms: None,
            on_end_called: Arc::clone(&on_end_cancel),
        }),
    };
    let engine2 = WorkflowEngine::new(
        store,
        vec![def2],
        cancel_mock.clone(),
        Arc::new(UnavailableForgeDriver),
        true,
    )?;
    let e_bg = Arc::clone(&engine2);
    let handle =
        tokio::spawn(async move { e_bg.start("cancelable", json!({}), "c1", None, None).await });
    started_notify.notified().await;
    let runs = engine2.list(None)?;
    let cid = runs
        .iter()
        .find(|r| r.workflow == "cancelable")
        .ok_or("missing cancelable")?
        .run_id
        .clone();
    engine2
        .cancel(&cid, Some("user aborted".to_owned()))
        .await?;
    let _ = handle.await?;
    assert_eq!(engine2.get(&cid)?.run.status, WorkflowRunStatus::Cancelled);
    assert!(on_end_cancel.load(Ordering::SeqCst));
    let intr = cancel_mock.interrupts.lock().map_err(|_| "lock")?;
    assert!(!intr.is_empty());
    assert!(intr[0].turn_id.is_some());
    Ok(())
}
