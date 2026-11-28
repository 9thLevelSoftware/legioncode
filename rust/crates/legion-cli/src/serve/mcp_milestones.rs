//! SEP-3415 milestone events & server discovery handlers for `/mcp` (`#40774`).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::brand::ACTIVE_BRAND;
use legion_persistence::pairing_tokens::verify_bearer_session_with_conn;
use legion_protocol::mcp::matches_mcp_header;
use legion_protocol::milestones::{
    MilestoneEventPollRequest, MilestoneEventType, MilestoneEvents, MilestoneThreadFilterArgs,
    MilestoneWorkflowFilterArgs, hidden_from_threads,
};
use serde_json::{Value, json};

#[path = "mcp_milestones_sse.rs"]
mod sse;
use sse::build_sse_stream;

use super::AppState;

const PROTOCOL_2026: &str = "2026-07-28";
const PROTOCOL_2025: &str = "2025-06-18";

/// Authenticated caller context on `/mcp`.
#[derive(Debug, Clone)]
pub(super) struct McpCallerContext {
    pub(super) session_id: Option<String>,
    pub(super) subject: Option<String>,
    pub(super) thread_id: Option<String>,
    pub(super) has_thread_scope: bool,
}

impl McpCallerContext {
    pub(super) fn shows_outside_only(&self) -> bool {
        hidden_from_threads(self.has_thread_scope)
    }
}

pub(super) fn authorize_caller(state: &AppState, headers: &HeaderMap) -> Option<McpCallerContext> {
    let thread_id = headers
        .iter()
        .find(|(name, _)| matches_mcp_header(name.as_str(), "thread-id"))
        .and_then(|(_, val)| val.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if !state.runtime_http.require_auth {
        return Some(McpCallerContext {
            session_id: None,
            subject: None,
            has_thread_scope: thread_id.is_some(),
            thread_id,
        });
    }

    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            let mut parts = raw.splitn(2, char::is_whitespace);
            let scheme = parts.next()?.trim();
            let rest = parts.next()?.trim();
            (scheme.eq_ignore_ascii_case("Bearer") && !rest.is_empty()).then_some(rest)
        })?;

    if let Some(ref dev_tok) = state.runtime_http.dev_auth_token
        && dev_tok == token
    {
        return Some(McpCallerContext {
            session_id: None,
            subject: None,
            has_thread_scope: thread_id.is_some(),
            thread_id,
        });
    }

    let now_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let session = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .ok()
        .and_then(|conn| verify_bearer_session_with_conn(&conn, token, now_ms).ok())?;
    let subject_thread_id = session
        .subject
        .strip_prefix("thread:")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let (Some(sub_tid), Some(hdr_tid)) = (&subject_thread_id, &thread_id)
        && sub_tid != hdr_tid
    {
        return None;
    }
    let effective_thread_id = subject_thread_id.or(thread_id);
    let has_thread_scope = effective_thread_id.is_some();
    Some(McpCallerContext {
        session_id: Some(session.session_id),
        subject: Some(session.subject),
        thread_id: effective_thread_id,
        has_thread_scope,
    })
}

pub(super) fn handle_server_discover(id: &Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "supportedVersions": [PROTOCOL_2026, PROTOCOL_2025],
                "capabilities": {
                    "tools": {},
                    "extensions": {
                        "io.modelcontextprotocol/events": {},
                    },
                },
                "serverInfo": {
                    "name": ACTIVE_BRAND.product_name,
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        })),
    )
        .into_response()
}

pub(super) fn handle_events_list(
    state: &AppState,
    caller: &McpCallerContext,
    id: &Value,
) -> Response {
    if !caller.shows_outside_only() {
        return (
            StatusCode::OK,
            Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "events": [] } })),
        )
            .into_response();
    }
    let events_svc = MilestoneEvents::new((*state.runtime_http.milestone_hub).clone());
    let list: Vec<Value> = events_svc
        .all()
        .into_iter()
        .map(|desc| {
            let schema = desc.parameters_schema();
            json!({
                "name": desc.name,
                "description": desc.description,
                "delivery": ["poll", "push"],
                "inputSchema": schema,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": { "events": list } })),
    )
        .into_response()
}

pub(super) fn handle_events_subscribe(id: &Value, params: &Value) -> Response {
    if params.pointer("/delivery/mode").and_then(Value::as_str) == Some("webhook") {
        return (
            StatusCode::OK,
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32026,
                    "message": "Webhook event delivery is not supported.",
                    "data": { "feature": "deliveryMode", "value": "webhook" },
                }
            })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "Use events/poll or events/stream." }
        })),
    )
        .into_response()
}

pub(super) fn handle_events_poll(
    state: &AppState,
    caller: &McpCallerContext,
    headers: &HeaderMap,
    id: &Value,
    params: &Value,
) -> Response {
    let (event_type, req) = match validate_event_request(caller, headers, id, params) {
        Ok(pair) => pair,
        Err(resp) => return *resp,
    };
    let svc = MilestoneEvents::new((*state.runtime_http.milestone_hub).clone());
    let page = if event_type.is_workflow() {
        let filter: MilestoneWorkflowFilterArgs =
            serde_json::from_value(params.clone()).unwrap_or_default();
        svc.poll_workflow(event_type, filter, req)
    } else {
        let filter: MilestoneThreadFilterArgs =
            serde_json::from_value(params.clone()).unwrap_or_default();
        svc.poll_thread(event_type, filter, req)
    };
    (
        StatusCode::OK,
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": page })),
    )
        .into_response()
}

pub(super) fn handle_events_stream(
    state: Arc<AppState>,
    caller: McpCallerContext,
    headers: &HeaderMap,
    id: Value,
    params: &Value,
) -> Response {
    let (event_type, req) = match validate_event_request(&caller, headers, &id, params) {
        Ok(pair) => pair,
        Err(resp) => return *resp,
    };
    let heartbeat_ms = params
        .get("heartbeatMs")
        .and_then(Value::as_u64)
        .unwrap_or(30_000)
        .clamp(1_000, 300_000);
    let svc = MilestoneEvents::new((*state.runtime_http.milestone_hub).clone());
    let follow = if event_type.is_workflow() {
        let filter: MilestoneWorkflowFilterArgs =
            serde_json::from_value(params.clone()).unwrap_or_default();
        svc.stream_workflow(event_type, filter, &req)
    } else {
        let filter: MilestoneThreadFilterArgs =
            serde_json::from_value(params.clone()).unwrap_or_default();
        svc.stream_thread(event_type, filter, &req)
    };
    let sse_stream = build_sse_stream(state, caller.session_id, id, follow, heartbeat_ms);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream; charset=utf-8")
        .header("cache-control", "no-cache")
        .body(Body::from_stream(sse_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn validate_event_request(
    caller: &McpCallerContext,
    headers: &HeaderMap,
    id: &Value,
    params: &Value,
) -> Result<(MilestoneEventType, MilestoneEventPollRequest), Box<Response>> {
    let param_name = params.get("name").and_then(Value::as_str);
    let hdr_name = headers
        .get("mcp-name")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    let is_sep3415 = headers.contains_key("mcp-method")
        || headers
            .get("mcp-protocol-version")
            .and_then(|v| v.to_str().ok())
            == Some(PROTOCOL_2026);
    if (is_sep3415 && hdr_name.is_none()) || hdr_name.is_some_and(|h| Some(h) != param_name) {
        return Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32020,
                        "message": "Mcp-Name header is missing or does not match event name.",
                    }
                })),
            )
                .into_response(),
        ));
    }
    if !caller.shows_outside_only() {
        return Err(Box::new(
            (
                StatusCode::OK,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32023,
                        "message": "Milestone events are only exposed to outside clients.",
                        "data": { "kind": "event" },
                    }
                })),
            )
                .into_response(),
        ));
    }
    let Ok(event_type) = serde_json::from_value::<MilestoneEventType>(Value::String(
        param_name.unwrap_or_default().to_owned(),
    )) else {
        return Err(Box::new(
            (
                StatusCode::OK,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32023,
                        "message": "Unknown milestone event.",
                        "data": { "kind": "event" },
                    }
                })),
            )
                .into_response(),
        ));
    };
    let req: MilestoneEventPollRequest = serde_json::from_value(params.clone()).unwrap_or_default();
    Ok((event_type, req))
}
