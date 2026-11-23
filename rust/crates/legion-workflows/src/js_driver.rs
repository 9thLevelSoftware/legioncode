#![forbid(unsafe_code)]
//! Shared JS/TS workflow execution trampoline bridging [`ReplayContext`] and JS runtimes.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::engine::WorkflowBodyDriver;
use crate::engine_replay::ReplayContext;
use crate::engine_step::{PassControl, PassOutcome, RunEndInfo, StepExecContext, StepRetry};
use crate::engine_wait::WaitCodecFn;
use crate::journal::JournalEntryStatus;
use crate::js_specs::{JsDriveResult, JsPassBackend, JsSuspendedOp, terminal_outcome_from_control};
use crate::js_step_forge::parse_agent_opts;
use crate::js_step_ops::{CachedStepOp, apply_step_ops, execute_step_async, run_js_step_attempt};
use crate::traits::{BoxFuture, PullRequestAddress, PullRequestUntil};

/// [`WorkflowBodyDriver`] backed by any [`JsPassBackend`] (Boa or external Node/Deno/Bun).
pub struct JsDriver {
    wf_key: String,
    backend: Arc<dyn JsPassBackend>,
}

impl JsDriver {
    /// Creates a new JS workflow driver for `wf_key`.
    pub fn new(wf_key: String, backend: Arc<dyn JsPassBackend>) -> Self {
        Self { wf_key, backend }
    }
}

impl WorkflowBodyDriver for JsDriver {
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome> {
        Box::pin(async move {
            loop {
                let journal = match snapshot_journal(ctx) {
                    Ok(j) => j,
                    Err(e) => return PassOutcome::Faulted(e),
                };
                let res = match self
                    .backend
                    .drive_pass(
                        &self.wf_key,
                        &ctx.input,
                        &journal,
                        None,
                        &Value::Null,
                        &ctx.run_id,
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(e) => return PassOutcome::Faulted(e),
                };
                match res {
                    JsDriveResult::Completed { value } => {
                        return PassOutcome::Completed(value.unwrap_or(Value::Null));
                    }
                    JsDriveResult::Stop { reason } => {
                        return terminal_outcome_from_control(false, reason);
                    }
                    JsDriveResult::Skip { reason } => {
                        return terminal_outcome_from_control(true, reason);
                    }
                    JsDriveResult::Fault { error } => return PassOutcome::Faulted(error),
                    JsDriveResult::TargetStepDone { error, .. } => {
                        return PassOutcome::Faulted(
                            error.unwrap_or_else(|| "unexpected targetStepDone".to_owned()),
                        );
                    }
                    JsDriveResult::Suspend { ops } => {
                        if ops.is_empty() {
                            return PassOutcome::Suspended;
                        }
                        let mut any_suspended = false;
                        for op in ops {
                            match execute_top_op(ctx, &self.backend, &self.wf_key, &journal, op)
                                .await
                            {
                                Ok(()) => {}
                                Err(PassControl::Suspend) => any_suspended = true,
                                Err(PassControl::Stop { reason, skipped }) => {
                                    return PassOutcome::Stopped { skipped, reason };
                                }
                                Err(PassControl::Fault(err)) => return PassOutcome::Faulted(err),
                            }
                        }
                        if any_suspended {
                            return PassOutcome::Suspended;
                        }
                    }
                }
            }
        })
    }

    fn run_on_end<'a>(
        &'a self,
        ctx: &'a mut StepExecContext,
        payload: Value,
        end: RunEndInfo,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let end_val = serde_json::to_value(end).map_err(|e| e.to_string())?;
            let mut step_ops: HashMap<String, CachedStepOp> = HashMap::new();
            loop {
                let ops_val = serde_json::to_value(&step_ops).map_err(|e| e.to_string())?;
                match self
                    .backend
                    .drive_on_end(&self.wf_key, &payload, &end_val, &ops_val)
                    .await?
                {
                    JsDriveResult::Completed { .. } => return Ok(()),
                    JsDriveResult::Fault { error } => return Err(error),
                    JsDriveResult::Suspend { ops } => {
                        apply_step_ops(ctx, ops, &mut step_ops, "onEnd").await?;
                    }
                    _ => return Err("Unexpected control outcome in onEnd".to_owned()),
                }
            }
        })
    }
}

fn snapshot_journal(ctx: &ReplayContext) -> Result<Value, String> {
    let entries = ctx
        .journal
        .list_entries(&ctx.run_id)
        .map_err(|e| e.to_string())?;
    let mut map = Map::new();
    for entry in entries {
        if entry.status == JournalEntryStatus::Completed {
            let val: Value = entry
                .payload_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or(Value::Null);
            map.insert(
                entry.entry_key,
                serde_json::json!({ "status": "completed", "value": val }),
            );
        }
    }
    Ok(Value::Object(map))
}

async fn execute_top_op(
    ctx: &mut ReplayContext,
    backend: &Arc<dyn JsPassBackend>,
    wf_key: &str,
    journal: &Value,
    op: JsSuspendedOp,
) -> Result<(), PassControl> {
    match op {
        JsSuspendedOp::Step {
            name,
            retry,
            schema,
            capture_error,
        } => {
            execute_step_op(
                ctx,
                backend,
                wf_key,
                journal,
                StepOpArgs {
                    name: &name,
                    retry,
                    schema: schema.as_ref(),
                    capture_error,
                },
            )
            .await
        }
        JsSuspendedOp::Agent {
            name,
            opts,
            try_mode,
        } => {
            let agent_opts = parse_agent_opts(&opts).map_err(PassControl::Fault)?;
            if try_mode {
                let _ = ctx.agent_try(&name, &agent_opts).await?;
            } else {
                let _ = ctx.agent(&name, &agent_opts).await?;
            }
            Ok(())
        }
        JsSuspendedOp::Wait {
            name,
            channel,
            schema,
            timeout_ms,
            codec_idx,
        } => execute_wait_op(
            ctx,
            backend,
            &name,
            channel.as_deref(),
            &schema,
            timeout_ms,
            codec_idx,
        ),
        JsSuspendedOp::Ask {
            name,
            title,
            choices,
            data,
            timeout_ms,
        } => {
            let _ = ctx.ask(
                &name,
                &title,
                &choices.unwrap_or_default(),
                data.as_ref(),
                timeout_ms,
            )?;
            Ok(())
        }
        JsSuspendedOp::Sleep { name, ms } => ctx.sleep(&name, ms),
        JsSuspendedOp::Title { value } => ctx.title(&value),
        JsSuspendedOp::Invoke {
            name,
            workflow,
            version,
            payload,
            key,
        } => {
            let _ = ctx.invoke(&name, &workflow, version, payload, key).await?;
            Ok(())
        }
        JsSuspendedOp::PrWait {
            name,
            pr,
            until,
            timeout_ms,
        } => {
            let addr: PullRequestAddress =
                serde_json::from_value(pr).map_err(|e| PassControl::Fault(e.to_string()))?;
            let until_cond: PullRequestUntil =
                serde_json::from_value(until).map_err(|e| PassControl::Fault(e.to_string()))?;
            let _ = ctx.pull_requests_wait(&name, addr, until_cond, timeout_ms)?;
            Ok(())
        }
        JsSuspendedOp::StepAsync { op, payload, .. } => {
            let mut sctx = ctx.step_context("__body");
            let _ = execute_step_async(&mut sctx, &op, payload)
                .await
                .map_err(PassControl::Fault)?;
            Ok(())
        }
    }
}

struct StepOpArgs<'a> {
    name: &'a str,
    retry: crate::js_specs::JsRetry,
    schema: Option<&'a Map<String, Value>>,
    capture_error: bool,
}

fn execute_wait_op(
    ctx: &mut ReplayContext,
    backend: &Arc<dyn JsPassBackend>,
    name: &str,
    channel: Option<&str>,
    schema: &Map<String, Value>,
    timeout_ms: Option<i64>,
    codec_idx: Option<usize>,
) -> Result<(), PassControl> {
    let codec_box: Option<Box<WaitCodecFn>> = codec_idx.map(|idx| {
        let b = Arc::clone(backend);
        let f: Box<WaitCodecFn> = Box::new(move |val| b.check_wait_codec(idx, val));
        f
    });
    let _ = ctx.wait(name, schema, channel, timeout_ms, codec_box.as_deref())?;
    Ok(())
}

async fn execute_step_op(
    ctx: &mut ReplayContext,
    backend: &Arc<dyn JsPassBackend>,
    wf_key: &str,
    journal: &Value,
    args: StepOpArgs<'_>,
) -> Result<(), PassControl> {
    let retry_cfg = StepRetry {
        times: args.retry.times,
        delay_ms: args.retry.delay_ms,
    };
    let run_id = ctx.run_id.clone();
    let input = ctx.input.clone();
    if args.capture_error {
        let _ = ctx
            .step_try(args.name, args.schema, retry_cfg, |sctx| {
                let b = Arc::clone(backend);
                let wf = wf_key.to_owned();
                let inp = input.clone();
                let j = journal.clone();
                let n = args.name.to_owned();
                let r = run_id.clone();
                Box::pin(async move { run_js_step_attempt(sctx, &b, &wf, &inp, &j, &n, &r).await })
            })
            .await?;
    } else {
        let _ = ctx
            .step(args.name, args.schema, retry_cfg, |sctx| {
                let b = Arc::clone(backend);
                let wf = wf_key.to_owned();
                let inp = input.clone();
                let j = journal.clone();
                let n = args.name.to_owned();
                let r = run_id.clone();
                Box::pin(async move { run_js_step_attempt(sctx, &b, &wf, &inp, &j, &n, &r).await })
            })
            .await?;
    }
    Ok(())
}
