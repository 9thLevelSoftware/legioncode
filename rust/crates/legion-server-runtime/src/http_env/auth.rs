//! `EnvironmentAuthHttpApi` session check, browser-session cookie, and WebSocket ticket handlers.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use legion_persistence::ownership::format_epoch_millis_utc;
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, DEFAULT_SESSION_TTL_MS, PairingTokenError,
    exchange_pairing_token_with_conn,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http_api::RuntimeHttpState;
use crate::http_auth::{AuthenticatedPrincipal, authenticate_headers, now_unix_ms};
use crate::http_env::auth_support::{build_auth_descriptor, split_scopes_and_permissions};
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, constant_time_eq_bytes, env_auth_invalid, env_internal,
    env_invalid_request, parse_json_body,
};

fn wire_session_method(principal: &AuthenticatedPrincipal, headers: &HeaderMap) -> &'static str {
    if principal.method == "bearer" {
        let auth_hdr = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth_hdr.len() >= 5 && auth_hdr[..5].eq_ignore_ascii_case("dpop ") {
            return "dpop-access-token";
        }
        return "bearer-access-token";
    }
    "browser-session-cookie"
}

fn lookup_session_expires_at(state: &RuntimeHttpState, session_id: &str, now_ms: i64) -> String {
    if let Ok(conn) = state.db().lock()
        && let Ok(Some(exp)) = conn
            .query_row(
                "SELECT expires_at FROM auth_sessions WHERE session_id = ?1 LIMIT 1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
    {
        return exp;
    }
    format_epoch_millis_utc(now_ms.saturating_add(DEFAULT_SESSION_TTL_MS))
}

/// `GET /api/auth/session` — returns `AuthSessionState`.
pub(crate) async fn handle_get_session(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Response {
    let descriptor = build_auth_descriptor(&state);
    let now_ms = now_unix_ms();

    let has_explicit_cred = headers.contains_key("authorization") || headers.contains_key("cookie");
    let principal = if !state.require_auth && !has_explicit_cred {
        Ok(AuthenticatedPrincipal {
            session_id: "local".to_owned(),
            role: "owner".to_owned(),
            scopes: ADMINISTRATIVE_CLIENT_SCOPES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            method: "local".to_owned(),
        })
    } else {
        authenticate_headers(&state, &headers)
    };

    match principal {
        Ok(p) => {
            let (scopes, permissions) = split_scopes_and_permissions(&p.scopes);
            let session_method = wire_session_method(&p, &headers);
            let expires_at = lookup_session_expires_at(&state, &p.session_id, now_ms);
            Json(json!({
                "authenticated": true,
                "auth": descriptor,
                "scopes": scopes,
                "permissions": permissions,
                "sessionMethod": session_method,
                "expiresAt": expires_at,
            }))
            .into_response()
        }
        Err(_) => Json(json!({
            "authenticated": false,
            "auth": descriptor,
        }))
        .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct BrowserSessionRequest {
    credential: String,
}

fn detect_client_device_and_os(
    user_agent: Option<&str>,
) -> (&'static str, Option<&'static str>, Option<&'static str>) {
    let Some(ua) = user_agent else {
        return ("unknown", None, None);
    };
    let lower = ua.to_ascii_lowercase();
    let device_type = if lower.contains("iphone") || lower.contains("android") {
        "mobile"
    } else if lower.contains("ipad") || lower.contains("tablet") {
        "tablet"
    } else if lower.contains("bot") || lower.contains("curl") {
        "bot"
    } else if lower.contains("mozilla") || lower.contains("electron") {
        "desktop"
    } else {
        "unknown"
    };
    let os = if lower.contains("windows") {
        Some("Windows")
    } else if lower.contains("mac os") || lower.contains("darwin") {
        Some("macOS")
    } else if lower.contains("linux") {
        Some("Linux")
    } else {
        None
    };
    let browser = if lower.contains("edg/") {
        Some("Edge")
    } else if lower.contains("chrome/") {
        Some("Chrome")
    } else if lower.contains("firefox/") {
        Some("Firefox")
    } else if lower.contains("safari/") {
        Some("Safari")
    } else {
        None
    };
    (device_type, os, browser)
}

fn persist_dev_browser_session(
    conn: &Connection,
    now_ms: i64,
    user_agent: Option<&str>,
) -> EnvResult<(String, Vec<String>, String)> {
    legion_persistence::pairing_tokens::ensure_pairing_schema(conn)
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
    let session_id = Uuid::new_v4().to_string();
    let token = format!("lcs_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    let scopes: Vec<String> = ADMINISTRATIVE_CLIENT_SCOPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let scopes_json = serde_json::to_string(&scopes)
        .map_err(|e| EnvErr(Box::new(env_internal(e.to_string()))))?;
    let issued_at = format_epoch_millis_utc(now_ms);
    let expires_at = format_epoch_millis_utc(now_ms.saturating_add(DEFAULT_SESSION_TTL_MS));
    let (device_type, os, browser) = detect_client_device_and_os(user_agent);

    conn.execute(
        "INSERT INTO auth_sessions (
           session_id, subject, scopes, method, client_label, client_user_agent,
           client_device_type, client_os, client_browser, issued_at, expires_at,
           revoked_at, bearer_token_hash
         ) VALUES (?1, 'owner', ?2, 'browser-session-cookie', 'Browser', ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
        params![
            session_id,
            scopes_json,
            user_agent,
            device_type,
            os,
            browser,
            issued_at,
            expires_at,
            token_hash,
        ],
    )
    .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;

    Ok((token, scopes, expires_at))
}

/// `POST /api/auth/browser-session` — validates credential and sets `HttpOnly` browser session cookie.
pub(crate) async fn handle_post_browser_session(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    let req: BrowserSessionRequest = parse_json_body(&body).map_err(|_| {
        EnvErr(Box::new(env_auth_invalid(
            "invalid_credential",
            "Invalid browser-session body",
        )))
    })?;
    let cred = req.credential.trim();
    if cred.is_empty() {
        return Err(EnvErr(Box::new(env_auth_invalid(
            "missing_credential",
            "Pairing credential is required",
        ))));
    }

    let now_ms = now_unix_ms();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let (token, scopes, expires_at) = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
        if state
            .dev_auth_token
            .as_deref()
            .is_some_and(|expected| constant_time_eq_bytes(expected.as_bytes(), cred.as_bytes()))
        {
            persist_dev_browser_session(&conn, now_ms, ua)?
        } else {
            let label = ua.unwrap_or("browser");
            match exchange_pairing_token_with_conn(&conn, cred, Some(label), now_ms) {
                Ok(sess) => {
                    let (device_type, os, browser) = detect_client_device_and_os(ua);
                    let _ = conn.execute(
                        "UPDATE auth_sessions
                         SET method = 'browser-session-cookie',
                             client_user_agent = COALESCE(?1, client_user_agent),
                             client_device_type = ?2,
                             client_os = COALESCE(?3, client_os),
                             client_browser = COALESCE(?4, client_browser)
                         WHERE session_id = ?5",
                        params![ua, device_type, os, browser, sess.session_id],
                    );
                    (sess.token, sess.scopes, sess.expires_at_iso)
                }
                Err(PairingTokenError::Sqlite { .. } | PairingTokenError::OpenDatabase { .. }) => {
                    return Err(EnvErr(Box::new(env_internal(
                        "pairing token database failure",
                    ))));
                }
                Err(err) => {
                    return Err(EnvErr(Box::new(env_auth_invalid(
                        "invalid_credential",
                        err.to_string(),
                    ))));
                }
            }
        }
    };

    let (legacy_scopes, permissions) = split_scopes_and_permissions(&scopes);
    let max_age_secs = DEFAULT_SESSION_TTL_MS / 1_000;
    let is_https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"));
    let secure_attr = if is_https { "; Secure" } else { "" };
    let cookie_val = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure_attr}",
        state.cookie_name
    );

    let mut resp = Json(json!({
        "authenticated": true,
        "scopes": legacy_scopes,
        "permissions": permissions,
        "sessionMethod": "browser-session-cookie",
        "expiresAt": expires_at,
    }))
    .into_response();
    if let Ok(hv) = HeaderValue::from_str(&cookie_val) {
        resp.headers_mut().append(SET_COOKIE, hv);
    }
    Ok(resp)
}

/// `POST /api/auth/websocket-ticket` — issues a generic single-use WebSocket upgrade ticket.
pub(crate) async fn handle_ws_ticket(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    issue_ws_ticket_response(&state, &headers, None)
}

/// `POST /api/auth/websocket-ticket/{use}` — issues a use-scoped (`rpc` | `media`) ticket.
pub(crate) async fn handle_ws_ticket_for(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(ticket_use): Path<String>,
) -> EnvResult<Response> {
    if ticket_use != "rpc" && ticket_use != "media" {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            format!("Invalid websocket ticket use `{ticket_use}`; expected `rpc` or `media`"),
        ))));
    }
    issue_ws_ticket_response(&state, &headers, Some(ticket_use))
}

fn issue_ws_ticket_response(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
    ticket_use: Option<String>,
) -> EnvResult<Response> {
    let principal = authorize_env(state, headers, None)?;
    let now_ms = now_unix_ms();
    let (ticket, expires_at_ms) = state.ws_tickets.issue(principal, ticket_use, now_ms);
    Ok(Json(json!({
        "ticket": ticket,
        "expiresAt": format_epoch_millis_utc(expires_at_ms),
    }))
    .into_response())
}
