//! Shared outbound frame queue item and backpressured stream execution context.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use legion_protocol::rpc_codec::{ChunkDisposition, RegistryChunkAction, RpcId, ServerRpcFrame};
use serde_json::Value;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use super::auth::WsPrincipal;
use crate::serve::AppState;

/// Per-request task handle and flow-control wakeup state.
pub(super) struct ActiveRequest {
    pub token: u64,
    pub handle: JoinHandle<()>,
    pub credit_notify: Arc<Notify>,
    pub credit_blocked: Arc<AtomicBool>,
    pub is_notification: bool,
}

/// Active multiplexed requests indexed by client-supplied `RpcId`.
pub(super) type ActiveMap = Arc<Mutex<HashMap<RpcId, ActiveRequest>>>;

/// Aborts all active per-request tasks and prunes client window state on socket drop.
pub(super) struct ClientGuard {
    pub client_id: u64,
    pub state: Arc<AppState>,
    pub active: ActiveMap,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.active.lock() {
            for (_, req) in map.drain() {
                req.handle.abort();
                req.credit_notify.notify_waiters();
            }
        }
        if let Ok(mut live) = self.state.live_clients.lock() {
            live.remove(&self.client_id);
        }
        if let Ok(mut windows) = self.state.output_windows.lock() {
            windows.close_client(self.client_id);
        }
    }
}

/// Outbound queue messages serialized onto the WebSocket sink task.
pub(super) enum OutboundItem {
    Frame(ServerRpcFrame),
    RawText(String),
    Pong(Bytes),
}

/// Execution context passed to every concurrent streaming RPC handler.
#[derive(Clone)]
pub(super) struct StreamContext {
    pub state: Arc<AppState>,
    pub principal: WsPrincipal,
    pub client_id: u64,
    pub request_id: RpcId,
    pub discard: bool,
    pub writer_tx: mpsc::UnboundedSender<OutboundItem>,
    pub credit_notify: Arc<Notify>,
    pub credit_blocked: Arc<AtomicBool>,
}

impl StreamContext {
    /// Emits a single `Chunk` item, awaiting window credit when saturated.
    pub(super) async fn send_chunk(&self, value: Value) -> bool {
        self.send_chunks(vec![value]).await
    }

    /// Emits a batch of `Chunk` items, applying `OutputWindowRegistry` backpressure.
    pub(super) async fn send_chunks(&self, values: Vec<Value>) -> bool {
        if values.is_empty() {
            return true;
        }
        if self.discard {
            return !self.writer_tx.is_closed();
        }

        let need_wait = {
            let Ok(mut windows) = self.state.output_windows.lock() else {
                return false;
            };
            let action = windows.on_send_chunk(self.client_id, &self.request_id, &values);
            let blocked = matches!(
                action,
                RegistryChunkAction::Windowed(ChunkDisposition::AwaitClientAck)
            );
            if blocked {
                self.credit_blocked.store(true, Ordering::SeqCst);
            }
            blocked
        };

        let frame = ServerRpcFrame::Chunk {
            request_id: self.request_id.clone(),
            values,
        };
        if self.writer_tx.send(OutboundItem::Frame(frame)).is_err() {
            return false;
        }

        if need_wait {
            loop {
                let notified = self.credit_notify.notified();
                if !self.credit_blocked.load(Ordering::SeqCst) {
                    break;
                }
                if self.writer_tx.is_closed() {
                    return false;
                }
                notified.await;
            }
        }
        true
    }
}
