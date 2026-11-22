//! Live `TerminalRuntime` WS RPC handlers (`terminal.open`, `terminal.write`,
//! `terminal.resize`, `terminal.snapshot`, `terminal.close`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::{TerminalOpenRequest, TerminalRuntime};
use serde::Deserialize;
use serde_json::{Value, json};

use super::rpc_wave_h::{encode_standard_base64, handle_terminal_snapshot};
use super::ws_mcp::rpc_fail;

/// Dispatches terminal WS RPC calls against the shared live [`TerminalRuntime`].
pub(crate) fn dispatch(runtime: &Arc<TerminalRuntime>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "terminal.open" => open_session(runtime, payload),
        "terminal.write" => write_session(runtime, payload),
        "terminal.resize" => resize_session(runtime, payload),
        "terminal.close" => close_session(runtime, payload),
        "terminal.snapshot" => snapshot_or_stateless(runtime, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenPayload {
    thread_id: String,
    terminal_id: String,
    cols: Option<u16>,
    rows: Option<u16>,
    cwd: Option<String>,
    resume: Option<bool>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env_overrides: BTreeMap<String, String>,
}

fn open_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: OpenPayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    let mut open_req = TerminalOpenRequest::new(
        req.thread_id,
        req.terminal_id,
        req.cols.unwrap_or(80),
        req.rows.unwrap_or(24),
    );
    open_req.cwd = req.cwd.map(PathBuf::from);
    open_req.resume = req.resume.unwrap_or(false);
    open_req.command = req.command.map(OsString::from);
    open_req.args = req.args.into_iter().map(OsString::from).collect();
    open_req.env_overrides = req.env_overrides;

    match runtime.terminal_open(&open_req) {
        Ok(res) => RpcExit::Success {
            value: json!({
                "key": res.key.to_wire_key(),
                "threadId": res.key.thread_id,
                "terminalId": res.key.terminal_id,
                "resumed": res.resumed,
                "wasEvicted": res.was_evicted,
                "cols": res.cols,
                "rows": res.rows,
                "status": terminal_status_str(res.status),
            }),
        },
        Err(err) => rpc_fail("TerminalOpenError", &err.to_string()),
    }
}

const fn terminal_status_str(
    status: legion_protocol::provider_env::TerminalResumeStatus,
) -> &'static str {
    use legion_protocol::provider_env::TerminalResumeStatus;
    match status {
        TerminalResumeStatus::ActiveOrRetained => "activeOrRetained",
        TerminalResumeStatus::EvictedResumable => "evictedResumable",
        TerminalResumeStatus::Closed => "closed",
        TerminalResumeStatus::Unknown => "unknown",
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WritePayload {
    thread_id: String,
    terminal_id: String,
    data: String,
    #[serde(default)]
    as_paste: bool,
}

fn write_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: WritePayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    match runtime.terminal_write(&req.thread_id, &req.terminal_id, &req.data, req.as_paste) {
        Ok(()) => RpcExit::Success {
            value: json!({ "ok": true }),
        },
        Err(err) => rpc_fail("TerminalWriteError", &err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizePayload {
    thread_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
}

fn resize_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: ResizePayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    match runtime.terminal_resize(&req.thread_id, &req.terminal_id, req.cols, req.rows) {
        Ok(()) => RpcExit::Success {
            value: json!({ "ok": true }),
        },
        Err(err) => rpc_fail("TerminalResizeError", &err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClosePayload {
    thread_id: String,
    terminal_id: String,
}

fn close_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: ClosePayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    match runtime.terminal_close(&req.thread_id, &req.terminal_id) {
        Ok(()) => RpcExit::Success {
            value: json!({ "ok": true }),
        },
        Err(err) => rpc_fail("TerminalCloseError", &err.to_string()),
    }
}

fn snapshot_or_stateless(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    if let Some(obj) = payload.as_object()
        && let (Some(thread_id), Some(terminal_id)) = (
            obj.get("threadId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            obj.get("terminalId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        && !obj.contains_key("feed")
        && !obj.contains_key("feedBytes")
    {
        return match runtime.terminal_snapshot(thread_id, terminal_id) {
            Ok(snap) => {
                let encoded = encode_standard_base64(&snap.lcvt_bytes);
                RpcExit::Success {
                    value: json!({
                        "threadId": thread_id,
                        "terminalId": terminal_id,
                        "cols": snap.cols,
                        "rows": snap.rows,
                        "cursorX": snap.cursor_x,
                        "cursorY": snap.cursor_y,
                        "viewportText": snap.viewport_text,
                        "lcvtBytes": snap.lcvt_bytes,
                        "lcvtBase64": encoded,
                    }),
                }
            }
            Err(err) => rpc_fail("TerminalSnapshotError", &err.to_string()),
        };
    }

    handle_terminal_snapshot(payload)
}
