#![forbid(unsafe_code)]
//! Replay engine tests: step pause/resume, step.try, child invoke, and webhook/schedule triggers.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use legion_workflows::engine::{WorkflowBodyDriver, WorkflowDefinition, WorkflowEngine};
use legion_workflows::engine_replay::ReplayContext;
use legion_workflows::engine_step::{PassControl, PassOutcome, StepAttempt, StepRetry};
use legion_workflows::schema::ensure_workflow_and_feedback_schema;
use legion_workflows::store::WorkflowStore;
use legion_workflows::time::ScheduleSpec;
use legion_workflows::traits::BoxFuture;
use legion_workflows::triggers_hooks::{HookDeliveryInput, TriggerDef, WebhookRequest};
use legion_workflows::types::WorkflowRunStatus;
use rusqlite::Connection;
use serde_json::{Map, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn open_store() -> Result<WorkflowStore, Box<dyn std::error::Error>> {
    let conn = Arc::new(Mutex::new(Connection::open_in_memory()?));
    {
        let g = conn.lock().map_err(|_| "lock poisoned")?;
        ensure_workflow_and_feedback_schema(&g)?;
    }
    Ok(WorkflowStore::new(conn)?)
}

struct ReplayStepDriver {
    step1_calls: Arc<AtomicUsize>,
    fail_step2: Arc<AtomicBool>,
}

impl WorkflowBodyDriver for ReplayStepDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            let s1 = match ctx
                .step("fetch", None, StepRetry::default(), |_| {
                    let c = Arc::clone(&self.step1_calls);
                    Box::pin(async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        Ok(json!("fetched"))
                    })
                })
                .await
            {
                Ok(v) => v,
                Err(PassControl::Suspend) => return PassOutcome::Suspended,
                Err(PassControl::Stop { skipped, reason }) => {
                    return PassOutcome::Stopped { skipped, reason };
                }
                Err(PassControl::Fault(e)) => return PassOutcome::Faulted(e),
            };

            let Ok(try_res) = ctx
                .step_try("optional_check", None, StepRetry::default(), |_| {
                    Box::pin(async { Err("non-fatal error".to_owned()) })
                })
                .await
            else {
                return PassOutcome::Faulted("unexpected step_try error".to_owned());
            };
            let StepAttempt::Failed { reason } = try_res else {
                return PassOutcome::Faulted("expected failed StepAttempt".to_owned());
            };

            let s2 = match ctx
                .step("deploy", None, StepRetry::default(), |_| {
                    let fail = Arc::clone(&self.fail_step2);
                    Box::pin(async move {
                        if fail.swap(false, Ordering::SeqCst) {
                            Err("transient deploy fault".to_owned())
                        } else {
                            Ok(json!("deployed"))
                        }
                    })
                })
                .await
            {
                Ok(v) => v,
                Err(PassControl::Suspend) => return PassOutcome::Suspended,
                Err(_) => return PassOutcome::Faulted("step2 control fault".to_owned()),
            };

            PassOutcome::Completed(json!({ "s1": s1, "tryReason": reason, "s2": s2 }))
        })
    }
}

#[tokio::test]
async fn deterministic_replay_skips_completed_steps_on_resume_and_supports_step_try() -> TestResult
{
    let store = open_store()?;
    let step1_calls = Arc::new(AtomicUsize::new(0));
    let fail_step2 = Arc::new(AtomicBool::new(true));
    let def = WorkflowDefinition {
        name: "ci".to_owned(),
        version: 1,
        payload_schema: Map::new(),
        returns_schema: None,
        resume_after_ms: vec![1_000],
        triggers: Vec::new(),
        has_on_end: false,
        driver: Arc::new(ReplayStepDriver {
            step1_calls: Arc::clone(&step1_calls),
            fail_step2,
        }),
    };
    let engine = WorkflowEngine::minimal(store, vec![def], true)?;
    let run_id = engine.start("ci", json!({}), "r1", None, None).await?;
    let paused = engine.get(&run_id)?;
    assert_eq!(paused.run.status, WorkflowRunStatus::Paused);
    assert_eq!(paused.run.pauses, 1);
    assert_eq!(step1_calls.load(Ordering::SeqCst), 1);

    engine.resume(&run_id).await?;
    let done = engine.get(&run_id)?;
    assert_eq!(done.run.status, WorkflowRunStatus::Completed);
    assert_eq!(step1_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        done.run.result,
        Some(json!({ "s1": "fetched", "tryReason": "non-fatal error", "s2": "deployed" }))
    );
    Ok(())
}

struct ChildDriver;
impl WorkflowBodyDriver for ChildDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move { PassOutcome::Completed(json!({ "echo": ctx.input })) })
    }
}

struct ParentAndWebhookDriver;
impl WorkflowBodyDriver for ParentAndWebhookDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            let child = match ctx
                .invoke("sub", "child", None, ctx.input.clone(), None)
                .await
            {
                Ok(c) => c,
                Err(PassControl::Suspend) => return PassOutcome::Suspended,
                Err(e) => return PassOutcome::Faulted(format!("{e:?}")),
            };
            PassOutcome::Completed(json!({ "child": child }))
        })
    }
}

#[tokio::test]
async fn invoke_child_and_webhook_secret_delivery_dedup_and_schedule() -> TestResult {
    let store = open_store()?;
    let hook_def = WorkflowDefinition {
        name: "parent".to_owned(),
        version: 1,
        payload_schema: Map::new(),
        returns_schema: None,
        resume_after_ms: vec![],
        triggers: vec![
            TriggerDef::Webhook {
                name: "gh".to_owned(),
                key_fn: None,
            },
            TriggerDef::Schedule {
                id: "every:60000".to_owned(),
                schedule: ScheduleSpec::Interval { every_ms: 60_000 },
            },
        ],
        has_on_end: false,
        driver: Arc::new(ParentAndWebhookDriver),
    };
    let child_def = WorkflowDefinition {
        name: "child".to_owned(),
        version: 1,
        payload_schema: Map::new(),
        returns_schema: None,
        resume_after_ms: vec![],
        triggers: Vec::new(),
        has_on_end: false,
        driver: Arc::new(ChildDriver),
    };
    let engine = WorkflowEngine::minimal(store.clone(), vec![hook_def, child_def], true)?;
    let hooks = engine.hooks()?;
    assert_eq!(hooks.len(), 1);
    let path = hooks[0].path.clone();
    let parts: Vec<&str> = path.trim_start_matches("/api/hooks/").split('/').collect();
    assert_eq!(parts.len(), 2);
    let bad = engine
        .deliver(HookDeliveryInput {
            hook_id: "wf-parent.gh".to_owned(),
            token: "wrong-secret".to_owned(),
            request: WebhookRequest {
                method: "POST".to_owned(),
                headers: BTreeMap::new(),
                query: String::new(),
                body: "{}".to_owned(),
            },
            delivery_id: None,
        })
        .await?;
    assert_eq!(bad, None);

    let mut headers = BTreeMap::new();
    headers.insert("x-github-delivery".to_owned(), "deliv-42".to_owned());
    let req = WebhookRequest {
        method: "POST".to_owned(),
        headers,
        query: String::new(),
        body: "{\"pr\":9}".to_owned(),
    };
    let r1 = engine
        .deliver(HookDeliveryInput {
            hook_id: "wf-parent.gh".to_owned(),
            token: parts[1].to_owned(),
            request: req.clone(),
            delivery_id: None,
        })
        .await?
        .ok_or("expected hook run")?;
    let r2 = engine
        .deliver(HookDeliveryInput {
            hook_id: "wf-parent.gh".to_owned(),
            token: parts[1].to_owned(),
            request: req,
            delivery_id: None,
        })
        .await?
        .ok_or("expected dedup hook run")?;
    assert_eq!(r1, r2);
    assert_eq!(engine.get(&r1)?.run.status, WorkflowRunStatus::Completed);

    // First sweep registers initial next_at in the future without spuriously firing immediately.
    engine.sweep().await?;
    assert_eq!(
        engine
            .list(None)?
            .iter()
            .filter(|r| r.workflow == "parent")
            .count(),
        1
    );

    // Backdate due cursor to past -> next sweep starts scheduled run and advances cursor cleanly.
    store.trigger_set("parent/every:60000", "2020-01-01T00:00:00.000Z")?;
    engine.sweep().await?;
    engine.sweep().await?;
    assert_eq!(
        engine
            .list(None)?
            .iter()
            .filter(|r| r.workflow == "parent")
            .count(),
        2
    );
    Ok(())
}
