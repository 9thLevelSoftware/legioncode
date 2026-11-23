//! WebSocket `/ws` upgrade authentication, framing, multiplexing, and live RPC streams.

mod auth;
mod auth_access;
mod frame_parts;
mod mux;
mod orchestration;
mod orchestration_thread;
mod other_streams;
mod server_config;
mod stream_ctx;
mod telemetry_vcs;
mod terminal;

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use legion_protocol::rpc_codec::{RpcExit, RpcId};
use serde_json::{Value, json};

use self::auth::{authenticate_ws_upgrade, parse_ws_query};
use self::mux::run_ws_connection;
use self::stream_ctx::StreamContext;
use crate::serve::AppState;

/// Handles `GET /ws?orchestrationProtocol=2` upgrades with credential enforcement,
/// optional `?frameParts=1` UTF-16 chunking, and concurrent Effect RPC v2 stream multiplexing.
pub(crate) async fn handle_ws(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let query = match parse_ws_query(req.uri()) {
        Ok(parsed) => parsed,
        Err(rej) => return rej.into_response(),
    };

    let principal = match authenticate_ws_upgrade(&state, req.headers(), &query) {
        Ok(principal) => principal,
        Err(rej) => return rej.into_response(),
    };

    let (mut parts, _) = req.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": rejection.body_text() })),
            )
                .into_response();
        }
    };

    let frame_parts = query.frame_parts;
    ws.on_upgrade(move |socket| run_ws_connection(socket, state, principal, frame_parts))
        .into_response()
}

fn open_request_window(state: &Arc<AppState>, client_id: u64, id: &RpcId, tag: &str) {
    let window_tag = tag
        .strip_prefix("orchestration.v2.")
        .map_or_else(|| tag.to_owned(), |rest| format!("orchestration.{rest}"));
    let live_snapshot = state
        .live_clients
        .lock()
        .ok()
        .map_or_default(|set| set.clone());
    if let Ok(mut windows) = state.output_windows.lock() {
        let _ = windows.open_for_request(client_id, id.clone(), &window_tag, &live_snapshot);
    }
}

fn is_streaming_rpc(tag: &str) -> bool {
    orchestration::handles_tag(tag)
        || server_config::handles_tag(tag)
        || terminal::handles_tag(tag)
        || other_streams::handles_tag(tag)
}

async fn dispatch_stream_rpc(ctx: StreamContext, tag: &str, payload: Value) -> RpcExit {
    if orchestration::handles_tag(tag) {
        return orchestration::dispatch_stream(&ctx, tag, payload).await;
    }
    if server_config::handles_tag(tag) {
        return server_config::dispatch_stream(&ctx, tag, payload).await;
    }
    if terminal::handles_tag(tag) {
        return terminal::dispatch_stream(&ctx, tag, payload).await;
    }
    other_streams::dispatch_stream(&ctx, tag, payload).await
}
