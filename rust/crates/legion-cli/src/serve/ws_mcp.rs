//! WebSocket `/ws` RPC v2 handler (`?orchestrationProtocol=2`, `?frameParts=1`) and `/mcp` re-export.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequestParts as _, Request, State, WebSocketUpgrade};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use legion_contracts::environment::{
    ORCHESTRATION_PROTOCOL_QUERY_PARAM, ORCHESTRATION_PROTOCOL_VERSION,
};
use legion_contracts::orchestration::OrchestrationCommand;
use legion_orchestrator::projections::ShellSnapshotOptions;
use legion_orchestrator::time::now_iso_utc;
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcCauseReason, RpcExit, RpcId, ServerRpcFrame, decode_client_frames,
    encode_server_frame, validate_orchestration_protocol,
};
use legion_shared::frame_parts::{
    FrameOutput, FramePartAssembler, SOCKET_FRAME_PARTS_QUERY_PARAM, split_socket_frame,
};
use serde_json::{Value, json};

use super::AppState;

pub(crate) use super::mcp::handle_mcp;

/// Handles `GET /ws`, enforcing `orchestrationProtocol=2` (`426 Upgrade Required` otherwise)
/// and supporting `?frameParts=1` and Effect RPC v2 envelopes.
pub(crate) async fn handle_ws(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    req: Request,
) -> Response {
    let proto = query_param(uri.query(), ORCHESTRATION_PROTOCOL_QUERY_PARAM);
    if validate_orchestration_protocol(proto.as_deref()).is_err() {
        return (
            StatusCode::UPGRADE_REQUIRED,
            Json(json!({
                "code": "orchestration_protocol_incompatible",
                "message": format!(
                    "Update this client to one that supports orchestration protocol {ORCHESTRATION_PROTOCOL_VERSION}."
                ),
                "orchestrationProtocolVersion": ORCHESTRATION_PROTOCOL_VERSION,
            })),
        )
            .into_response();
    }

    let (mut parts, _body) = req.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };

    let frame_parts = has_query_flag(uri.query(), SOCKET_FRAME_PARTS_QUERY_PARAM);
    ws.on_upgrade(move |socket| handle_ws_socket(socket, state, frame_parts))
}

struct ClientGuard {
    client_id: u64,
    state: Arc<AppState>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        if let Ok(mut live) = self.state.live_clients.lock() {
            live.remove(&self.client_id);
        }
        if let Ok(mut windows) = self.state.output_windows.lock() {
            windows.close_client(self.client_id);
        }
    }
}

async fn handle_ws_socket(mut socket: WebSocket, state: Arc<AppState>, frame_parts: bool) {
    let client_id = state.next_client_id.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut live) = state.live_clients.lock() {
        live.insert(client_id);
    }
    let _guard = ClientGuard {
        client_id,
        state: Arc::clone(&state),
    };
    let mut assembler = FramePartAssembler::new();

    while let Some(msg_res) = socket.recv().await {
        let Ok(msg) = msg_res else {
            break;
        };
        match msg {
            Message::Text(text) => {
                let whole = if frame_parts {
                    match assembler.push(text.as_str()) {
                        FrameOutput::IntermediateAck(ack) => {
                            if socket.send(Message::Text(ack.into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        FrameOutput::Complete(complete) => complete.into_owned(),
                    }
                } else {
                    text.to_string()
                };

                if !process_client_payload(&mut socket, &state, client_id, &whole, frame_parts)
                    .await
                {
                    break;
                }
            }
            Message::Ping(bytes) => {
                if socket.send(Message::Pong(bytes)).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Binary(_) | Message::Pong(_) => {}
        }
    }
}

async fn process_client_payload(
    socket: &mut WebSocket,
    state: &Arc<AppState>,
    client_id: u64,
    raw: &str,
    frame_parts: bool,
) -> bool {
    let frames = match decode_client_frames(raw) {
        Ok(frames) => frames,
        Err(err) => {
            let defect = ServerRpcFrame::Defect {
                defect: json!({ "message": err.to_string() }),
            };
            let _ = send_server_frame(socket, &defect, frame_parts).await;
            return true;
        }
    };

    for frame in frames {
        match frame {
            ClientRpcFrame::Ping => {
                if send_server_frame(socket, &ServerRpcFrame::Pong, frame_parts)
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            ClientRpcFrame::Ack { request_id } => {
                if let Ok(mut windows) = state.output_windows.lock() {
                    let _ = windows.on_client_ack(client_id, &request_id);
                }
            }
            ClientRpcFrame::Interrupt { request_id } => {
                if let Ok(mut windows) = state.output_windows.lock() {
                    let _ = windows.on_send_exit(client_id, &request_id);
                }
            }
            ClientRpcFrame::Eof => return false,
            ClientRpcFrame::Request {
                id,
                tag,
                payload,
                is_notification,
                ..
            } => {
                open_request_window(state, client_id, &id, &tag);
                let exit = dispatch_ws_rpc(state, &tag, payload).await;
                if let Ok(mut windows) = state.output_windows.lock() {
                    let _ = windows.on_send_exit(client_id, &id);
                }
                if !is_notification {
                    let reply = ServerRpcFrame::Exit {
                        request_id: id,
                        exit,
                    };
                    if send_server_frame(socket, &reply, frame_parts)
                        .await
                        .is_err()
                    {
                        return false;
                    }
                }
            }
        }
    }

    true
}

fn open_request_window(state: &Arc<AppState>, client_id: u64, id: &RpcId, tag: &str) {
    let live_snapshot = state
        .live_clients
        .lock()
        .ok()
        .map_or_default(|set| set.clone());
    if let Ok(mut windows) = state.output_windows.lock() {
        let _ = windows.open_for_request(client_id, id.clone(), tag, &live_snapshot);
    }
}

async fn send_server_frame(
    socket: &mut WebSocket,
    frame: &ServerRpcFrame,
    frame_parts: bool,
) -> Result<(), ()> {
    let encoded = encode_server_frame(frame).map_err(|_| ())?;
    if frame_parts {
        for part in split_socket_frame(&encoded) {
            socket
                .send(Message::Text(part.into()))
                .await
                .map_err(|_| ())?;
        }
    } else {
        socket
            .send(Message::Text(encoded.into()))
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

async fn dispatch_ws_rpc(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "orchestration.v2.dispatchCommand" | "orchestration.dispatchCommand" => {
            let cmd: OrchestrationCommand = match serde_json::from_value(payload) {
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
                Ok(snapshot) => match serde_json::to_value(&snapshot) {
                    Ok(val) => RpcExit::Success { value: val },
                    Err(err) => rpc_fail("CodecError", &err.to_string()),
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

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let q = query?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(v.to_owned());
        }
    }
    None
}

fn has_query_flag(query: Option<&str>, key: &str) -> bool {
    let Some(q) = query else {
        return false;
    };
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return !v.eq_ignore_ascii_case("0") && !v.eq_ignore_ascii_case("false");
        }
    }
    false
}
