#![forbid(unsafe_code)]
//! Step-scoped async/sync host service execution (`store`, `signal`, `forge`, `runs`, `start`, `cancel`).

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::engine_step::StepExecContext;
use crate::js_specs::{JsDriveResult, JsPassBackend, JsSuspendedOp};
use crate::js_step_forge::execute_forge_op;
use crate::types::{RunFilter, SignalAddress, WorkflowReferenceInput};

#[derive(Serialize)]
pub(crate) struct CachedStepOp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(crate) async fn apply_step_ops(
    sctx: &mut StepExecContext,
    ops: Vec<JsSuspendedOp>,
    step_ops: &mut HashMap<String, CachedStepOp>,
    context_label: &str,
) -> Result<(), String> {
    for op in ops {
        match op {
            JsSuspendedOp::StepAsync {
                cache_key,
                op,
                payload,
                ..
            } => {
                let entry = match execute_step_async(sctx, &op, payload).await {
                    Ok(v) => CachedStepOp {
                        ok: true,
                        value: Some(v),
                        error: None,
                    },
                    Err(e) => CachedStepOp {
                        ok: false,
                        value: None,
                        error: Some(e),
                    },
                };
                step_ops.insert(cache_key, entry);
            }
            _ => return Err(format!("unsupported op inside {context_label}")),
        }
    }
    Ok(())
}

/// Drives one JS `step(...)` attempt until `TargetStepDone` or failure.
pub(crate) async fn run_js_step_attempt(
    sctx: &mut StepExecContext,
    backend: &Arc<dyn JsPassBackend>,
    wf_key: &str,
    input: &Value,
    journal: &Value,
    step_name: &str,
    run_id: &str,
) -> Result<Value, String> {
    let mut step_ops: HashMap<String, CachedStepOp> = HashMap::new();
    let label = format!("step '{step_name}'");
    loop {
        let ops_val = serde_json::to_value(&step_ops).map_err(|e| e.to_string())?;
        match backend
            .drive_pass(wf_key, input, journal, Some(step_name), &ops_val, run_id)
            .await?
        {
            JsDriveResult::TargetStepDone { ok, value, error } => {
                return if ok {
                    Ok(value.unwrap_or(Value::Null))
                } else {
                    Err(error.unwrap_or_else(|| "step failed".to_owned()))
                };
            }
            JsDriveResult::Fault { error } => return Err(error),
            JsDriveResult::Suspend { ops } => {
                if ops.is_empty() {
                    return Err(format!("step '{step_name}' was unreachable during replay"));
                }
                apply_step_ops(sctx, ops, &mut step_ops, &label).await?;
            }
            _ => return Err(format!("unexpected control in step '{step_name}'")),
        }
    }
}

/// Dispatches a suspended `stepAsync` operation through [`StepExecContext`].
pub(crate) async fn execute_step_async(
    sctx: &mut StepExecContext,
    op: &str,
    payload: Value,
) -> Result<Value, String> {
    if let Some(res) = try_execute_store_or_control(sctx, op, &payload).await {
        return res;
    }
    execute_forge_op(sctx, op, payload).await
}

async fn try_execute_store_or_control(
    sctx: &mut StepExecContext,
    op: &str,
    payload: &Value,
) -> Option<Result<Value, String>> {
    let res = match op {
        "store.get" => {
            let key = payload
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            sctx.store
                .kv_get(key)
                .map(|v| v.unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        "store.set" => {
            let key = payload
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let val = payload.get("value").cloned().unwrap_or(Value::Null);
            sctx.store
                .kv_set(key, &val)
                .map(|()| Value::Null)
                .map_err(|e| e.to_string())
        }
        "store.delete" => {
            let key = payload
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default();
            sctx.store
                .kv_delete(key)
                .map(|()| Value::Null)
                .map_err(|e| e.to_string())
        }
        "store.list" => {
            let prefix = payload
                .get("prefix")
                .and_then(Value::as_str)
                .unwrap_or_default();
            sctx.store
                .kv_list(prefix)
                .map_err(|e| e.to_string())
                .and_then(|items| serde_json::to_value(items).map_err(|e| e.to_string()))
        }
        "runs" => serde_json::from_value::<RunFilter>(payload.clone())
            .map_err(|e| e.to_string())
            .and_then(|f| sctx.runs(&f).map_err(|e| e.to_string()))
            .and_then(|rows| serde_json::to_value(rows).map_err(|e| e.to_string())),
        "noteReference" => {
            let r = payload.get("reference").cloned().unwrap_or(Value::Null);
            let kind = r
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("link")
                .to_owned();
            let key = r
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let title = r
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let url = r.get("url").and_then(Value::as_str).map(str::to_owned);
            let state = r.get("state").and_then(Value::as_str).map(str::to_owned);
            let address = r.get("address").cloned().unwrap_or(Value::Null);
            sctx.note_reference(&WorkflowReferenceInput {
                kind,
                key,
                address,
                title,
                url,
                state,
            });
            Ok(Value::Null)
        }
        "signal" => execute_signal(sctx, payload).await,
        "signalAll" => execute_signal_all(sctx, payload).await,
        "start" => execute_start(sctx, payload).await,
        "cancel" => execute_cancel(sctx, payload).await,
        _ => return None,
    };
    Some(res)
}

async fn execute_signal(sctx: &mut StepExecContext, payload: &Value) -> Result<Value, String> {
    let addr: SignalAddress =
        serde_json::from_value(payload.get("address").cloned().unwrap_or(Value::Null))
            .map_err(|e| e.to_string())?;
    let val = payload.get("value").cloned().unwrap_or(Value::Null);
    let id = payload
        .get("signalId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let ttl_ms = payload.get("ttlMs").and_then(Value::as_i64);
    let res = sctx
        .signal(addr, val, id, ttl_ms)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::to_value(res).map_err(|e| e.to_string())
}

async fn execute_signal_all(sctx: &StepExecContext, payload: &Value) -> Result<Value, String> {
    let addr = payload.get("address").cloned().unwrap_or(Value::Null);
    let channel = addr
        .get("channel")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let wf = addr.get("workflow").and_then(Value::as_str);
    let val = payload.get("value").cloned().unwrap_or(Value::Null);
    let count = sctx
        .signal_all(channel, wf, val)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Value::from(count))
}

async fn execute_start(sctx: &StepExecContext, payload: &Value) -> Result<Value, String> {
    let wf = payload
        .get("workflow")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let p = payload.get("payload").cloned().unwrap_or(Value::Null);
    let key = payload
        .get("key")
        .and_then(Value::as_str)
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let ver = payload.get("version").and_then(Value::as_i64);
    let id = sctx
        .start(wf, p, &key, ver)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "runId": id }))
}

async fn execute_cancel(sctx: &StepExecContext, payload: &Value) -> Result<Value, String> {
    let run_id = payload
        .get("runId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reason = payload
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    sctx.cancel(run_id, reason)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Value::Null)
}
