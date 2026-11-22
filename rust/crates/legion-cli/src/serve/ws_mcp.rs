//! WebSocket `/ws` RPC v2 entrypoint (`?orchestrationProtocol=2`, `?frameParts=1`),
//! `/mcp` re-export, and cleanly factored unary RPC fallback dispatcher.

#[path = "ws/mod.rs"]
mod ws;

use std::sync::Arc;

use legion_contracts::orchestration::decode_wire_orchestration_command;
use legion_orchestrator::projections::ShellSnapshotOptions;
use legion_orchestrator::time::now_iso_utc;
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::{Value, json};

use super::AppState;

pub(crate) use super::mcp::handle_mcp;
pub(crate) use ws::handle_ws;

/// Dispatches unary WebSocket RPC methods (factored so Stage C can plug in `super::rpc::dispatch`).
pub(super) async fn dispatch_unary_ws_rpc(
    state: &Arc<AppState>,
    tag: &str,
    payload: Value,
) -> RpcExit {
    match tag {
        "orchestration.v2.dispatchCommand" | "orchestration.dispatchCommand" => {
            let cmd = match decode_wire_orchestration_command(payload) {
                Ok(cmd) => cmd,
                Err(err) => return rpc_fail("ParseError", &err.to_string()),
            };
            match state.orchestrator.dispatch_now(&cmd).await {
                Ok(res) => RpcExit::Success {
                    value: json!({ "sequence": res.outcome.receipt.result_sequence }),
                },
                Err(err) => rpc_fail("OrchestrationV2DispatchCommandError", &err.to_string()),
            }
        }
        "orchestration.v2.getShellSnapshot" | "orchestration.getShellSnapshot" => {
            let options = parse_shell_snapshot_options(&payload);
            match state.orchestrator.shell_snapshot(&now_iso_utc(), options) {
                Ok(snap) => match serde_json::to_value(snap) {
                    Ok(value) => RpcExit::Success { value },
                    Err(err) => rpc_fail("SerializeError", &err.to_string()),
                },
                Err(err) => rpc_fail("OrchestrationV2GetShellSnapshotError", &err.to_string()),
            }
        }
        "providers.listDrivers"
        | "checkpoints.getDiff"
        | "diagnostics.traceSummary"
        | "themes.read" => super::rpc_wave_d::dispatch(state, tag, payload),
        "terminal.open" | "terminal.write" | "terminal.resize" | "terminal.snapshot"
        | "terminal.close" => super::rpc_terminal::dispatch(&state.terminal_runtime, tag, payload),
        "resources.sample" | "surfaces.decorateDiff" | "surfaces.mobileChrome" => {
            super::rpc_wave_h::dispatch(tag, payload)
        }
        other => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{other}`"),
        ),
    }
}

fn parse_shell_snapshot_options(payload: &Value) -> ShellSnapshotOptions {
    let mut opts = ShellSnapshotOptions::default();
    if let Some(obj) = payload.as_object() {
        if let Some(v) = obj.get("omitIdleSubagents").and_then(Value::as_bool) {
            opts.omit_idle_subagents = v;
        }
        if let Some(v) = obj.get("omitOlderSettled").and_then(Value::as_bool) {
            opts.omit_older_settled = v;
        }
        if let Some(v) = obj.get("includeArchived").and_then(Value::as_bool) {
            opts.include_archived = v;
        }
    }
    opts
}

pub(super) fn rpc_fail(tag: &str, message: &str) -> RpcExit {
    RpcExit::Failure {
        cause: vec![RpcCauseReason::Fail {
            error: json!({
                "_tag": tag,
                "message": message,
            }),
        }],
    }
}
