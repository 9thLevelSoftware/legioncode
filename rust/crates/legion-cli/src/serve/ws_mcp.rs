//! WebSocket `/ws` RPC v2 entrypoint (`?orchestrationProtocol=2`, `?frameParts=1`) and `/mcp` re-export.

#[path = "ws/mod.rs"]
mod ws;

use std::sync::Arc;

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::{Value, json};

use super::AppState;

pub(crate) use super::mcp::handle_mcp;
pub(crate) use ws::handle_ws;

/// Dispatches unary WebSocket RPC methods through the full [`super::rpc`] router.
pub(super) async fn dispatch_unary_ws_rpc(
    state: &Arc<AppState>,
    tag: &str,
    payload: Value,
) -> RpcExit {
    super::rpc::dispatch(state, tag, payload).await
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
