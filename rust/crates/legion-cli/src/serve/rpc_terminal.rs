//! Live `TerminalRuntime` WS RPC handlers (`terminal.open`, `terminal.write`,
//! `terminal.resize`, `terminal.clear`, `terminal.snapshot`, `terminal.close`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use legion_server_runtime::terminal_session::TerminalProcessMode;
use legion_server_runtime::{TerminalOpenRequest, TerminalRuntime};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::rpc_wave_h::{encode_standard_base64, handle_terminal_snapshot};
use super::ws_mcp::rpc_fail;

/// Dispatches terminal WS RPC calls against the shared live [`TerminalRuntime`].
pub(crate) fn dispatch(runtime: &Arc<TerminalRuntime>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "terminal.open" => open_session(runtime, payload),
        "terminal.write" => write_session(runtime, payload),
        "terminal.resize" => resize_session(runtime, payload),
        "terminal.clear" => clear_session(runtime, payload),
        "terminal.close" => close_session(runtime, payload),
        "terminal.snapshot" => snapshot_or_stateless(runtime, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn rpc_fail_obj(error: Value) -> RpcExit {
    RpcExit::Failure {
        cause: vec![RpcCauseReason::Fail { error }],
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenPayload {
    thread_id: String,
    #[serde(default)]
    terminal_id: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    cwd: Option<String>,
    worktree_path: Option<String>,
    label: Option<String>,
    resume: Option<bool>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env_overrides: BTreeMap<String, String>,
    interactive: Option<bool>,
}

fn open_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: OpenPayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    let terminal_id = req
        .terminal_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default")
        .to_owned();
    let mut open_req = TerminalOpenRequest::new(
        req.thread_id.clone(),
        terminal_id.clone(),
        req.cols.unwrap_or(120),
        req.rows.unwrap_or(30),
    );
    if let Some(raw_cwd) = req.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let cwd_path = Path::new(raw_cwd);
        match std::fs::metadata(cwd_path) {
            Ok(meta) if !meta.is_dir() => {
                return rpc_fail_obj(json!({
                    "_tag": "TerminalCwdNotDirectoryError",
                    "cwd": raw_cwd,
                    "message": format!("Terminal working directory is not a directory: {raw_cwd}"),
                }));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return rpc_fail_obj(json!({
                    "_tag": "TerminalCwdNotFoundError",
                    "cwd": raw_cwd,
                    "message": format!("Terminal working directory does not exist: {raw_cwd}"),
                }));
            }
            Err(err) => {
                return rpc_fail_obj(json!({
                    "_tag": "TerminalCwdStatError",
                    "cwd": raw_cwd,
                    "message": format!("Failed to inspect terminal working directory '{raw_cwd}': {err}"),
                }));
            }
        }
        open_req.cwd = Some(cwd_path.to_path_buf());
        if req.command.is_none() {
            open_req.process_mode = TerminalProcessMode::InteractivePty;
        }
    } else if req.interactive == Some(true) && req.command.is_none() {
        open_req.process_mode = TerminalProcessMode::InteractivePty;
    }

    open_req.worktree_path = req
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    open_req.label = req.label;
    open_req.resume = req.resume.unwrap_or(false);
    open_req.command = req.command.map(OsString::from);
    open_req.args = req.args.into_iter().map(OsString::from).collect();
    open_req.env_overrides = req.env_overrides;

    match runtime.terminal_open(&open_req) {
        Ok(res) => {
            let mut out = match runtime.session_snapshot(&req.thread_id, &terminal_id, None) {
                Ok(snap) => match serde_json::to_value(snap) {
                    Ok(Value::Object(map)) => map,
                    _ => Map::new(),
                },
                Err(_) => Map::new(),
            };
            out.insert("key".to_owned(), json!(res.key.to_wire_key()));
            out.insert("threadId".to_owned(), json!(res.key.thread_id));
            out.insert("terminalId".to_owned(), json!(res.key.terminal_id));
            out.insert("resumed".to_owned(), json!(res.resumed));
            out.insert("wasEvicted".to_owned(), json!(res.was_evicted));
            out.insert("cols".to_owned(), json!(res.cols));
            out.insert("rows".to_owned(), json!(res.rows));
            out.entry("status")
                .or_insert_with(|| json!(terminal_status_str(res.status)));
            RpcExit::Success {
                value: Value::Object(out),
            }
        }
        Err(err) => rpc_fail_obj(json!({
            "_tag": "TerminalError",
            "operation": "open",
            "message": err.to_string(),
        })),
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
    #[serde(default)]
    terminal_id: Option<String>,
    data: String,
    #[serde(default)]
    as_paste: bool,
}

fn write_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: WritePayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    let terminal_id = req
        .terminal_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    match runtime.terminal_write(&req.thread_id, terminal_id, &req.data, req.as_paste) {
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
    #[serde(default)]
    terminal_id: Option<String>,
    cols: u16,
    rows: u16,
}

fn resize_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: ResizePayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    let terminal_id = req
        .terminal_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("default");
    match runtime.terminal_resize(&req.thread_id, terminal_id, req.cols, req.rows) {
        Ok(()) => RpcExit::Success {
            value: json!({ "ok": true }),
        },
        Err(err) => rpc_fail("TerminalResizeError", &err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadTerminalPayload {
    thread_id: String,
    #[serde(default)]
    terminal_id: Option<String>,
}

fn clear_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: ThreadTerminalPayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    if let Some(tid) = req
        .terminal_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match runtime.terminal_clear(&req.thread_id, tid) {
            Ok(()) => RpcExit::Success {
                value: json!({ "ok": true }),
            },
            Err(err) => rpc_fail("TerminalError", &err.to_string()),
        };
    }
    let summaries = runtime.list_summaries(Some(&req.thread_id));
    if summaries.is_empty() {
        return match runtime.terminal_clear(&req.thread_id, "default") {
            Ok(()) => RpcExit::Success {
                value: json!({ "ok": true }),
            },
            Err(err) => rpc_fail("TerminalError", &err.to_string()),
        };
    }
    for summary in summaries {
        if let Err(err) = runtime.terminal_clear(&req.thread_id, &summary.terminal_id) {
            return rpc_fail("TerminalError", &err.to_string());
        }
    }
    RpcExit::Success {
        value: json!({ "ok": true }),
    }
}

fn close_session(runtime: &TerminalRuntime, payload: Value) -> RpcExit {
    let req: ThreadTerminalPayload = match serde_json::from_value(payload) {
        Ok(parsed) => parsed,
        Err(err) => return rpc_fail("ParseError", &err.to_string()),
    };
    if let Some(tid) = req
        .terminal_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match runtime.terminal_close(&req.thread_id, tid) {
            Ok(()) => RpcExit::Success {
                value: json!({ "ok": true }),
            },
            Err(err) => rpc_fail("TerminalCloseError", &err.to_string()),
        };
    }
    let summaries = runtime.list_summaries(Some(&req.thread_id));
    for summary in summaries {
        if let Err(err) = runtime.terminal_close(&req.thread_id, &summary.terminal_id) {
            return rpc_fail("TerminalCloseError", &err.to_string());
        }
    }
    RpcExit::Success {
        value: json!({ "ok": true }),
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
