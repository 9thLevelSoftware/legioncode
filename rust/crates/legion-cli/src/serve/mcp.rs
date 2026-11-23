//! Pre-auth `/mcp` HTTP boundary handler and authenticated MCP JSON-RPC tool dispatcher.

use std::sync::Arc;

use axum::Json;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::brand::ACTIVE_BRAND;
use legion_protocol::mcp::{McpGateOutcome, McpHttpRequest, evaluate_mcp_http_request};
use serde_json::{Value, json};
use uuid::Uuid;

use super::AppState;
use super::mcp_milestones::{
    authorize_caller, handle_events_list, handle_events_poll, handle_events_stream,
    handle_events_subscribe, handle_server_discover,
};

const MAX_MCP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Handles `ANY /mcp`, running pre-auth HTTP boundary checks via [`evaluate_mcp_http_request`].
pub(crate) async fn handle_mcp(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let origin = header_presence_or_str(req.headers(), "origin");
    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok());
    let accept = req.headers().get("accept").and_then(|v| v.to_str().ok());

    let gate = evaluate_mcp_http_request(&McpHttpRequest::new(
        req.method().as_str(),
        origin,
        content_type,
        accept,
    ));

    match gate {
        Err(rejection) => {
            let status =
                StatusCode::from_u16(rejection.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut resp = (
                status,
                Json(json!({
                    "error": rejection.code,
                    "message": rejection.message,
                })),
            )
                .into_response();
            if let Some(allow) = rejection.allow {
                resp.headers_mut()
                    .insert("allow", HeaderValue::from_static(allow));
            }
            resp
        }
        Ok(McpGateOutcome::TerminateSession) => StatusCode::NO_CONTENT.into_response(),
        Ok(McpGateOutcome::JsonRpcPost) => handle_mcp_post(&state, req).await,
    }
}

async fn handle_mcp_post(state: &Arc<AppState>, req: Request) -> Response {
    let headers = req.headers().clone();
    let session_header = headers
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let Ok(bytes) = to_bytes(req.into_body(), MAX_MCP_BODY_BYTES).await else {
        return jsonrpc_parse_error("Invalid request body");
    };

    let Ok(payload) = serde_json::from_slice::<Value>(&bytes) else {
        return jsonrpc_parse_error("Parse error");
    };

    let id = payload.get("id").cloned().unwrap_or(Value::Null);
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "ping" => (
            StatusCode::OK,
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            })),
        )
            .into_response(),
        "server/discover" => handle_server_discover(&id),
        "initialize" => {
            let mut resp = (
                StatusCode::OK,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {
                            "tools": {},
                            "extensions": { "io.modelcontextprotocol/events": {} },
                        },
                        "serverInfo": {
                            "name": ACTIVE_BRAND.product_name,
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                })),
            )
                .into_response();
            let sid = session_header.unwrap_or_else(|| Uuid::new_v4().to_string());
            if let Ok(hdr) = HeaderValue::from_str(&sid) {
                resp.headers_mut().insert(MCP_SESSION_ID_HEADER, hdr);
            }
            resp
        }
        "notifications/initialized" | "notifications/roots/list_changed" => {
            let Some(_caller) = authorize_caller(state, &headers) else {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "unauthorized", "message": "Missing or invalid Bearer token." })),
                )
                    .into_response();
            };
            if payload.get("id").is_some() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "invalid_request", "message": "Notification must not carry id." })),
                )
                    .into_response();
            }
            StatusCode::ACCEPTED.into_response()
        }
        "tools/list" | "tools/call" | "events/list" | "events/poll" | "events/stream"
        | "events/subscribe" => {
            dispatch_authenticated_mcp(state, &headers, method, id, &params).await
        }
        _ => (
            StatusCode::OK,
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            })),
        )
            .into_response(),
    }
}

async fn dispatch_authenticated_mcp(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    method: &str,
    id: Value,
    params: &Value,
) -> Response {
    let Some(caller) = authorize_caller(state, headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized", "message": "Missing or invalid Bearer token." })),
        )
            .into_response();
    };
    match method {
        "events/list" => handle_events_list(state, &caller, &id),
        "events/poll" => handle_events_poll(state, &caller, headers, &id, params),
        "events/stream" => handle_events_stream(Arc::clone(state), caller, headers, id, params),
        "events/subscribe" => handle_events_subscribe(&id, params),
        "tools/list" => (
            StatusCode::OK,
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": super::mcp_tools::list_tools_json(state, &caller),
            })),
        )
            .into_response(),
        _ => {
            let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = super::mcp_tools::call_tool(state, &caller, tool_name, &args).await;
            (
                StatusCode::OK,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                })),
            )
                .into_response()
        }
    }
}

fn jsonrpc_parse_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": -32700, "message": message }
        })),
    )
        .into_response()
}

fn header_presence_or_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).map(|v| v.to_str().unwrap_or(""))
}
