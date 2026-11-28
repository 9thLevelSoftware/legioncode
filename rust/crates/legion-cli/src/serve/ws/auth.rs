//! Upgrade protocol validation, credential verification, and loopback/origin guards for `/ws`.

#[path = "auth_helpers.rs"]
mod helpers;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::header::{AUTHORIZATION, COOKIE, ORIGIN};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::{RpcCallerKind, caller_kind_from_subject};
use legion_persistence::pairing_tokens::verify_bearer_session_with_conn;
use legion_protocol::auth::extract_cookie_value;
use legion_protocol::rpc_codec::validate_orchestration_protocol;
use legion_server_runtime::{AuthenticatedPrincipal, authenticate_headers};
use serde_json::json;

use self::helpers::{
    constant_time_eq, find_prefixed_cookie, has_query_flag, is_allowed_loopback_origin,
    is_direct_loopback_request, percent_decode, query_param,
};
use crate::serve::AppState;

const DEV_AUTH_COOKIE_NAME: &str = "legioncode_dev_auth";
const SESSION_COOKIE_PREFIX: &str = "legioncode_session_";

/// Authenticated WebSocket client principal resolved during upgrade.
#[derive(Debug, Clone)]
pub(super) struct WsPrincipal {
    pub session_id: String,
    #[expect(dead_code, reason = "retained on WsPrincipal for session diagnostics")]
    pub role: String,
    #[expect(dead_code, reason = "retained on WsPrincipal for session diagnostics")]
    pub subject: String,
    pub scopes: Vec<String>,
    #[expect(dead_code, reason = "retained on WsPrincipal for session diagnostics")]
    pub method: String,
    pub caller_kind: RpcCallerKind,
}

fn is_mcp_client_subject(subject: &str) -> bool {
    let lower = subject.trim().to_ascii_lowercase();
    lower == "mcp-client" || lower.starts_with("mcp-client:")
}

impl WsPrincipal {
    fn local() -> Self {
        Self {
            session_id: "local".to_owned(),
            role: "admin".to_owned(),
            subject: "local".to_owned(),
            scopes: vec!["*".to_owned()],
            method: "local".to_owned(),
            caller_kind: RpcCallerKind::Client,
        }
    }

    fn dev_token() -> Self {
        Self {
            session_id: "dev".to_owned(),
            role: "admin".to_owned(),
            subject: "owner".to_owned(),
            scopes: vec!["*".to_owned()],
            method: "dev_token".to_owned(),
            caller_kind: RpcCallerKind::Client,
        }
    }

    fn from_authenticated(p: AuthenticatedPrincipal) -> Result<Self, WsUpgradeRejection> {
        if is_mcp_client_subject(&p.subject) {
            return Err(unauthorized(
                "MCP client sessions are only accepted by the MCP endpoint.",
            ));
        }
        let caller_kind = caller_kind_from_subject(&p.subject);
        Ok(Self {
            session_id: p.session_id,
            role: p.role,
            subject: p.subject,
            scopes: p.scopes,
            method: p.method,
            caller_kind,
        })
    }
}

/// Parsed query options carried on `/ws`.
#[derive(Debug, Clone)]
pub(super) struct WsQueryParams {
    pub frame_parts: bool,
    pub ticket: Option<String>,
}

/// HTTP rejection returned before completing a `/ws` upgrade.
pub(super) struct WsUpgradeRejection {
    status: StatusCode,
    body: serde_json::Value,
}

impl IntoResponse for WsUpgradeRejection {
    fn into_response(self) -> Response {
        (self.status, axum::Json(self.body)).into_response()
    }
}

/// Validates `?orchestrationProtocol=2` and parses upgrade query parameters.
pub(super) fn parse_ws_query(uri: &Uri) -> Result<WsQueryParams, WsUpgradeRejection> {
    let query = uri.query();
    let protocol = query_param(query, "orchestrationProtocol");
    if let Err(err) = validate_orchestration_protocol(protocol.as_deref()) {
        return Err(WsUpgradeRejection {
            status: StatusCode::UPGRADE_REQUIRED,
            body: json!({
                "error": err.to_string(),
                "receivedProtocol": protocol,
            }),
        });
    }

    let ticket = query_param(query, "wsTicket")
        .or_else(|| query_param(query, "ticket"))
        .map(|raw| percent_decode(&raw));

    Ok(WsQueryParams {
        frame_parts: has_query_flag(query, "frameParts"),
        ticket,
    })
}

/// Authenticates an incoming `/ws` HTTP upgrade request.
///
/// Enforces query ticket -> `Authorization` header (`BUG-03`) -> session cookies ->
/// direct unproxied loopback fallback (with `Origin` validation against CSWSH).
pub(super) fn authenticate_ws_upgrade(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    query: &WsQueryParams,
) -> Result<WsPrincipal, WsUpgradeRejection> {
    if let Some(ref ticket) = query.ticket {
        let trimmed = ticket.trim();
        if let Some(principal) = state
            .runtime_http
            .consume_ws_ticket(trimmed, Some("rpc"))
            .or_else(|| state.runtime_http.consume_ws_ticket(trimmed, None))
        {
            return WsPrincipal::from_authenticated(principal);
        }
        return verify_token_credential(state, trimmed);
    }

    let cookie_raw = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let has_auth_hdr = headers.contains_key(AUTHORIZATION);
    let has_named_cookie = extract_cookie_value(cookie_raw, &state.runtime_http.cookie_name)
        .is_some()
        || extract_cookie_value(cookie_raw, "legioncode_session").is_some()
        || extract_cookie_value(cookie_raw, DEV_AUTH_COOKIE_NAME).is_some()
        || find_prefixed_cookie(cookie_raw, "legioncode_dev_session_").is_some();

    if has_auth_hdr || has_named_cookie {
        let principal = authenticate_headers(&state.runtime_http, headers)
            .map_err(|_| unauthorized("invalid WebSocket credentials"))?;
        return WsPrincipal::from_authenticated(principal);
    }

    if let Some(prefixed_token) = find_prefixed_cookie(cookie_raw, SESSION_COOKIE_PREFIX) {
        return verify_token_credential(state, &prefixed_token);
    }

    if !state.runtime_http.require_auth {
        return Ok(WsPrincipal::local());
    }

    if !is_direct_loopback_request(headers) {
        return Err(unauthorized(
            "authentication required for non-loopback WebSocket upgrade",
        ));
    }

    if let Some(origin) = headers.get(ORIGIN).and_then(|v| v.to_str().ok())
        && !is_allowed_loopback_origin(origin)
    {
        return Err(WsUpgradeRejection {
            status: StatusCode::FORBIDDEN,
            body: json!({ "error": "untrusted WebSocket origin" }),
        });
    }

    Ok(WsPrincipal::local())
}

fn verify_token_credential(state: &AppState, raw: &str) -> Result<WsPrincipal, WsUpgradeRejection> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(unauthorized("invalid or expired WebSocket credentials"));
    }

    if let Some(ref dev_tok) = state.runtime_http.dev_auth_token
        && constant_time_eq(token.as_bytes(), dev_tok.as_bytes())
    {
        return Ok(WsPrincipal::dev_token());
    }

    let now_ms = now_unix_ms();
    let conn = state
        .runtime_http
        .db()
        .lock()
        .map_err(|_| unauthorized("session lookup unavailable"))?;
    let verified = verify_bearer_session_with_conn(&conn, token, now_ms)
        .map_err(|_| unauthorized("invalid or expired WebSocket credentials"))?;
    if is_mcp_client_subject(&verified.subject) {
        return Err(unauthorized(
            "MCP client sessions are only accepted by the MCP endpoint.",
        ));
    }
    let caller_kind = caller_kind_from_subject(&verified.subject);
    Ok(WsPrincipal {
        session_id: verified.session_id,
        role: verified.role,
        subject: verified.subject,
        scopes: verified.scopes,
        method: verified.method,
        caller_kind,
    })
}

fn unauthorized(msg: &str) -> WsUpgradeRejection {
    WsUpgradeRejection {
        status: StatusCode::UNAUTHORIZED,
        body: json!({ "error": msg }),
    }
}

pub(super) fn now_unix_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}
