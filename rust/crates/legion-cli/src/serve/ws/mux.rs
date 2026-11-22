//! Concurrent WebSocket multiplexer, `OutputWindowRegistry` credit backpressure, and cancellation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use legion_protocol::rpc_codec::{
    AckDisposition, ClientRpcFrame, RpcCauseReason, RpcExit, RpcId, ServerRpcFrame,
    decode_client_frames,
};
use legion_shared::frame_parts::FramePartAssembler;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

use super::auth::WsPrincipal;
use super::frame_parts::{InboundAssembly, assemble_inbound_text, encode_server_texts};
use super::stream_ctx::{ActiveMap, ActiveRequest, ClientGuard, OutboundItem, StreamContext};
use crate::serve::AppState;

/// Runs the multiplexed reader loop + dedicated outbound writer task for one WebSocket connection.
pub(super) async fn run_ws_connection(
    socket: WebSocket,
    state: Arc<AppState>,
    principal: WsPrincipal,
    frame_parts: bool,
) {
    let client_id = state.next_client_id.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut live) = state.live_clients.lock() {
        live.insert(client_id);
    }

    let active: ActiveMap = Arc::new(Mutex::new(HashMap::new()));
    let guard = ClientGuard {
        client_id,
        state: Arc::clone(&state),
        active: Arc::clone(&active),
    };

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<OutboundItem>();

    let writer_handle = tokio::spawn(async move {
        while let Some(item) = writer_rx.recv().await {
            match item {
                OutboundItem::RawText(text) => {
                    if ws_tx.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                OutboundItem::Pong(bytes) => {
                    if ws_tx.send(Message::Pong(bytes)).await.is_err() {
                        break;
                    }
                }
                OutboundItem::Frame(frame) => {
                    let Ok(parts) = encode_server_texts(&frame, frame_parts) else {
                        break;
                    };
                    for part in parts {
                        if ws_tx.send(Message::Text(part.into())).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
        let _ = ws_tx.close().await;
    });

    let mut assembler = FramePartAssembler::new();
    let next_token = AtomicU64::new(1);

    while let Some(msg_res) = ws_rx.next().await {
        let Ok(msg) = msg_res else {
            break;
        };
        match msg {
            Message::Text(text) => {
                match assemble_inbound_text(&mut assembler, text.as_str(), frame_parts) {
                    InboundAssembly::IntermediateAck(ack) => {
                        if writer_tx.send(OutboundItem::RawText(ack)).is_err() {
                            break;
                        }
                    }
                    InboundAssembly::Complete(whole) => {
                        if !process_client_payload(
                            &state,
                            &principal,
                            client_id,
                            &active,
                            &next_token,
                            &writer_tx,
                            &whole,
                        ) {
                            break;
                        }
                    }
                }
            }
            Message::Ping(bytes) => {
                if writer_tx.send(OutboundItem::Pong(bytes)).is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Binary(_) | Message::Pong(_) => {}
        }
    }

    drop(guard);
    drop(writer_tx);
    let _ = writer_handle.await;
}

fn process_client_payload(
    state: &Arc<AppState>,
    principal: &WsPrincipal,
    client_id: u64,
    active: &ActiveMap,
    next_token: &AtomicU64,
    writer_tx: &mpsc::UnboundedSender<OutboundItem>,
    raw: &str,
) -> bool {
    let frames = match decode_client_frames(raw) {
        Ok(frames) => frames,
        Err(err) => {
            let defect = ServerRpcFrame::Defect {
                defect: json!({ "message": err.to_string() }),
            };
            let _ = writer_tx.send(OutboundItem::Frame(defect));
            return true;
        }
    };

    for frame in frames {
        match frame {
            ClientRpcFrame::Ping => {
                if writer_tx
                    .send(OutboundItem::Frame(ServerRpcFrame::Pong))
                    .is_err()
                {
                    return false;
                }
            }
            ClientRpcFrame::Ack { request_id } => handle_ack(state, client_id, active, &request_id),
            ClientRpcFrame::Interrupt { request_id } => {
                if !handle_interrupt(state, client_id, active, writer_tx, request_id) {
                    return false;
                }
            }
            ClientRpcFrame::Eof => return false,
            ClientRpcFrame::Request {
                id,
                tag,
                payload,
                is_notification,
                discard,
                ..
            } => {
                spawn_request(
                    Arc::clone(state),
                    principal.clone(),
                    client_id,
                    active,
                    next_token.fetch_add(1, Ordering::Relaxed),
                    writer_tx.clone(),
                    id,
                    tag,
                    payload,
                    is_notification,
                    discard,
                );
            }
        }
    }
    true
}

fn handle_ack(state: &Arc<AppState>, client_id: u64, active: &ActiveMap, request_id: &RpcId) {
    let wake_target = if let Ok(mut windows) = state.output_windows.lock()
        && windows.on_client_ack(client_id, request_id) == AckDisposition::ForwardAck
        && let Ok(map) = active.lock()
        && let Some(entry) = map.get(request_id)
    {
        Some((
            Arc::clone(&entry.credit_blocked),
            Arc::clone(&entry.credit_notify),
        ))
    } else {
        None
    };
    if let Some((blocked, notify)) = wake_target {
        blocked.store(false, Ordering::SeqCst);
        notify.notify_waiters();
    }
}

fn handle_interrupt(
    state: &Arc<AppState>,
    client_id: u64,
    active: &ActiveMap,
    writer_tx: &mpsc::UnboundedSender<OutboundItem>,
    request_id: RpcId,
) -> bool {
    let removed = active
        .lock()
        .ok()
        .and_then(|mut map| map.remove(&request_id));
    if let Ok(mut windows) = state.output_windows.lock() {
        let _ = windows.on_send_exit(client_id, &request_id);
    }
    let notify_only = if let Some(entry) = removed {
        entry.handle.abort();
        entry.credit_notify.notify_waiters();
        entry.is_notification
    } else {
        false
    };
    if notify_only {
        return true;
    }
    let reply = ServerRpcFrame::Exit {
        request_id,
        exit: RpcExit::Failure {
            cause: vec![RpcCauseReason::Interrupt { fiber_id: Some(0) }],
        },
    };
    writer_tx.send(OutboundItem::Frame(reply)).is_ok()
}

#[expect(
    clippy::too_many_arguments,
    reason = "spawns per-request Tokio task without extra heap allocation"
)]
fn spawn_request(
    state: Arc<AppState>,
    principal: WsPrincipal,
    client_id: u64,
    active: &ActiveMap,
    token: u64,
    writer_tx: mpsc::UnboundedSender<OutboundItem>,
    id: RpcId,
    tag: String,
    payload: Value,
    is_notification: bool,
    discard: bool,
) {
    super::open_request_window(&state, client_id, &id, &tag);
    if discard && !is_notification {
        let _ = writer_tx.send(OutboundItem::Frame(ServerRpcFrame::Exit {
            request_id: id.clone(),
            exit: RpcExit::Success { value: Value::Null },
        }));
    }

    let credit_notify = Arc::new(Notify::new());
    let credit_blocked = Arc::new(AtomicBool::new(false));
    let ctx = StreamContext {
        state: Arc::clone(&state),
        principal,
        client_id,
        request_id: id.clone(),
        discard,
        writer_tx: writer_tx.clone(),
        credit_notify: Arc::clone(&credit_notify),
        credit_blocked: Arc::clone(&credit_blocked),
    };

    let req_id = id.clone();
    let task_active = Arc::clone(active);
    let handle = tokio::spawn(async move {
        let exit = if super::is_streaming_rpc(&tag) {
            super::dispatch_stream_rpc(ctx, &tag, payload).await
        } else {
            super::super::dispatch_unary_ws_rpc(&state, &tag, payload).await
        };

        let still_active = if let Ok(mut map) = task_active.lock() {
            if map.get(&req_id).is_some_and(|e| e.token == token) {
                map.remove(&req_id);
                true
            } else {
                false
            }
        } else {
            false
        };

        if let Ok(mut windows) = state.output_windows.lock() {
            let _ = windows.on_send_exit(client_id, &req_id);
        }
        if still_active && !is_notification && !discard {
            let _ = writer_tx.send(OutboundItem::Frame(ServerRpcFrame::Exit {
                request_id: req_id,
                exit,
            }));
        }
    });

    if let Ok(mut map) = active.lock() {
        map.insert(
            id,
            ActiveRequest {
                token,
                handle,
                credit_notify,
                credit_blocked,
                is_notification,
            },
        );
    }
}
