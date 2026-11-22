//! Upgrade protocol validation, credential verification, and loopback/origin guards for `/ws`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::header::{AUTHORIZATION, COOKIE, HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use legion_persistence::pairing_tokens::verify_bearer_session_with_conn;
use legion_protocol::auth::extract_cookie_value;
use legion_protocol::rpc_codec::validate_orchestration_protocol;
use legion_server_runtime::authenticate_headers;
use serde_json::json;

use crate::serve::AppState;

const DEV_AUTH_COOKIE_NAME: &str = "legioncode_dev_auth";
const SESSION_COOKIE_PREFIX: &str = "legioncode_session_";

/// Authenticated WebSocket client principal resolved during upgrade.
#[derive(Debug, Clone)]
pub(super) struct WsPrincipal {
    pub session_id: String,
}

impl WsPrincipal {
    fn local() -> Self {
        Self {
            session_id: "local".to_owned(),
        }
    }

    fn dev_token() -> Self {
        Self {
            session_id: "dev".to_owned(),
        }
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
        return verify_token_credential(state, ticket)
            .ok_or_else(|| unauthorized("invalid or expired WebSocket ticket"));
    }

    let cookie_raw = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let has_auth_hdr = headers.contains_key(AUTHORIZATION);
    let has_named_cookie = extract_cookie_value(cookie_raw, &state.runtime_http.cookie_name)
        .is_some()
        || extract_cookie_value(cookie_raw, DEV_AUTH_COOKIE_NAME).is_some();

    if has_auth_hdr || has_named_cookie {
        return authenticate_headers(&state.runtime_http, headers)
            .map(|p| WsPrincipal {
                session_id: p.session_id,
            })
            .map_err(|_| unauthorized("invalid WebSocket credentials"));
    }

    if let Some(prefixed_token) = find_prefixed_session_cookie(cookie_raw) {
        return verify_token_credential(state, &prefixed_token)
            .ok_or_else(|| unauthorized("invalid session cookie"));
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

fn verify_token_credential(state: &AppState, raw: &str) -> Option<WsPrincipal> {
    let token = raw.trim();
    if token.is_empty() {
        return None;
    }

    if let Some(ref dev_tok) = state.runtime_http.dev_auth_token
        && constant_time_eq(token.as_bytes(), dev_tok.as_bytes())
    {
        return Some(WsPrincipal::dev_token());
    }

    let now_ms = now_unix_ms();
    let conn = state.runtime_http.db().lock().ok()?;
    let verified = verify_bearer_session_with_conn(&conn, token, now_ms).ok()?;
    Some(WsPrincipal {
        session_id: verified.session_id,
    })
}

fn unauthorized(msg: &str) -> WsUpgradeRejection {
    WsUpgradeRejection {
        status: StatusCode::UNAUTHORIZED,
        body: json!({ "error": msg }),
    }
}

fn find_prefixed_session_cookie(cookie_header: &str) -> Option<String> {
    for segment in cookie_header.split(';') {
        let trimmed = segment.trim();
        let Some((name, val)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim().starts_with(SESSION_COOKIE_PREFIX) {
            let value = val.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn is_direct_loopback_request(headers: &HeaderMap) -> bool {
    const PROXY_HEADERS: [&str; 5] = [
        "x-forwarded-for",
        "x-forwarded-host",
        "forwarded",
        "cf-connecting-ip",
        "true-client-ip",
    ];
    if PROXY_HEADERS.iter().any(|name| headers.contains_key(*name)) {
        return false;
    }

    let Some(host_val) = headers.get(HOST).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    is_loopback_authority(host_val.trim())
}

fn is_allowed_loopback_origin(origin: &str) -> bool {
    let trimmed = origin.trim();
    let lower = trimmed.to_ascii_lowercase();
    if [
        "app://",
        "legioncode://",
        "capacitor://",
        "ionic://",
        "file://",
        "tauri://",
    ]
    .iter()
    .any(|scheme| lower.starts_with(scheme))
    {
        return true;
    }
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"));
    let Some(authority) = rest else {
        return false;
    };
    let host_part = authority.split('/').next().unwrap_or("");
    is_loopback_authority(host_part)
}

fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(after_bracket) = authority.strip_prefix('[') {
        after_bracket.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn constant_time_eq(lhs: &[u8], rhs: &[u8]) -> bool {
    if lhs.len() != rhs.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (a, b) in lhs.iter().zip(rhs.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub(super) fn now_unix_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
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

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
