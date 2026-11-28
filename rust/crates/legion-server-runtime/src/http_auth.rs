//! Authentication endpoints (`/api/auth/pair`, `/oauth/token`) and `BUG-03` credential guard.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, COOKIE, PRAGMA, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_persistence::pairing_tokens::{
    ExchangedSession, exchange_pairing_token_with_conn, verify_bearer_session_with_conn,
};
use legion_protocol::auth::{CredentialPrecedence, RequestCredentials, extract_cookie_value};
use serde::{Deserialize, Serialize};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;

/// Authenticated request principal resolved via `BUG-03` credential precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedPrincipal {
    /// Session ID (`"local"` or `"dev"` when unauthenticated local or dev-token authenticated).
    pub session_id: String,
    /// Subject identifier (`"local"`, `"owner"`, `"one-time-token"`, `"thread:<id>"`, etc.).
    pub subject: String,
    /// Role label (`"admin"` or `"client"`).
    pub role: String,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Authentication method (`"bearer"`, `"cookie"`, `"dev_token"`, or `"local"`).
    pub method: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairRequest {
    token: Option<String>,
    credential: Option<String>,
    client_label: Option<String>,
    #[serde(rename = "client_label")]
    client_label_snake: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairSuccessResponse {
    access_token: String,
    #[serde(rename = "access_token")]
    access_token_snake: String,
    token_type: &'static str,
    #[serde(rename = "token_type")]
    token_type_snake: &'static str,
    session_id: String,
    scopes: Vec<String>,
    role: String,
    expires_at: String,
    expires_in: i64,
}

pub(crate) fn now_unix_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}

/// Authenticate an HTTP request using `BUG-03` strict precedence (`Authorization` > cookies).
///
/// Crucially, if an `Authorization` header is present and invalid, this NEVER falls back
/// to a valid session cookie.
pub fn authenticate_headers(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
) -> Result<AuthenticatedPrincipal, ServerRuntimeError> {
    let auth_hdr = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let cookie_hdr = headers
        .get(COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let primary_cookie = cookie_hdr.as_deref().and_then(|raw| {
        extract_cookie_value(raw, &state.cookie_name)
            .or_else(|| extract_cookie_by_prefix(raw, "legioncode_session_"))
    });
    let legacy_cookie = cookie_hdr.as_deref().and_then(|raw| {
        (state.cookie_name != "legioncode_session")
            .then(|| extract_cookie_value(raw, "legioncode_session"))
            .flatten()
    });
    let dev_cookie = cookie_hdr.as_deref().and_then(|raw| {
        extract_cookie_value(raw, "legioncode_dev_auth")
            .or_else(|| extract_cookie_by_prefix(raw, "legioncode_dev_session_"))
    });

    let selected = match CredentialPrecedence::select(&RequestCredentials {
        authorization: auth_hdr.as_deref(),
        primary_cookie: primary_cookie.as_deref(),
        legacy_cookie: legacy_cookie.as_deref(),
        dev_cookie: dev_cookie.as_deref(),
    }) {
        Ok(sel) => sel,
        Err(err) => {
            if auth_hdr.is_some()
                || primary_cookie.is_some()
                || legacy_cookie.is_some()
                || dev_cookie.is_some()
            {
                return Err(ServerRuntimeError::Unauthorized(err.to_string()));
            }
            if !state.require_auth {
                return Ok(AuthenticatedPrincipal {
                    session_id: "local".to_owned(),
                    subject: "local".to_owned(),
                    role: "admin".to_owned(),
                    scopes: vec!["*".to_owned()],
                    method: "local".to_owned(),
                });
            }
            return Err(ServerRuntimeError::Unauthorized(err.to_string()));
        }
    };

    if let Some(ref dev_tok) = state.dev_auth_token
        && constant_time_eq(selected.token.as_bytes(), dev_tok.as_bytes())
    {
        return Ok(AuthenticatedPrincipal {
            session_id: "dev".to_owned(),
            subject: "owner".to_owned(),
            role: "admin".to_owned(),
            scopes: vec!["*".to_owned()],
            method: "dev_token".to_owned(),
        });
    }

    let now_ms = now_unix_ms();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let verified = verify_bearer_session_with_conn(&conn, &selected.token, now_ms)?;
    Ok(AuthenticatedPrincipal {
        session_id: verified.session_id,
        subject: verified.subject,
        role: verified.role,
        scopes: verified.scopes,
        method: verified.method,
    })
}

/// `POST /api/auth/pair` — exchange one-time pairing credential for bearer session + `Set-Cookie`.
pub(crate) async fn handle_pair(
    State(state): State<std::sync::Arc<RuntimeHttpState>>,
    Json(body): Json<PairRequest>,
) -> Result<Response, ServerRuntimeError> {
    let presented = body
        .token
        .or(body.credential)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServerRuntimeError::BadRequest("missing pairing token".to_owned()))?;
    let label = body.client_label.or(body.client_label_snake);
    let exchanged = exchange_token(&state, &presented, label.as_deref())?;
    let expires_in = 3600_i64;
    let payload = PairSuccessResponse {
        access_token: exchanged.token.clone(),
        access_token_snake: exchanged.token.clone(),
        token_type: "Bearer",
        token_type_snake: "Bearer",
        session_id: exchanged.session_id,
        scopes: exchanged.scopes,
        role: exchanged.role,
        expires_at: exchanged.expires_at_iso,
        expires_in,
    };
    Ok(build_session_response(
        StatusCode::OK,
        &payload,
        &state.cookie_name,
        &exchanged.token,
    ))
}

fn exchange_token(
    state: &RuntimeHttpState,
    presented: &str,
    client_label: Option<&str>,
) -> Result<ExchangedSession, ServerRuntimeError> {
    let now_ms = now_unix_ms();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    exchange_pairing_token_with_conn(&conn, presented, client_label, now_ms).map_err(Into::into)
}

fn build_session_response<T: Serialize>(
    status: StatusCode,
    payload: &T,
    cookie_name: &str,
    session_token: &str,
) -> Response {
    let mut resp = (status, Json(payload)).into_response();
    let cookie_val = format!("{cookie_name}={session_token}; HttpOnly; Path=/; SameSite=Lax");
    if let Ok(hdr) = HeaderValue::from_str(&cookie_val) {
        resp.headers_mut().insert(SET_COOKIE, hdr);
    }
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    resp
}

fn extract_cookie_by_prefix(cookie_header: &str, prefix: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let trimmed = part.trim();
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        let val = value.trim();
        if name.trim().starts_with(prefix) && !val.is_empty() {
            return Some(val.to_owned());
        }
    }
    None
}

pub(crate) fn percent_decode_form(raw: &str) -> Result<String, ServerRuntimeError> {
    let plus_replaced = raw.replace('+', " ");
    let bytes = plus_replaced.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char)
                .to_digit(16)
                .and_then(|d| u8::try_from(d).ok())
                .ok_or_else(|| ServerRuntimeError::BadRequest("invalid form escape".to_owned()))?;
            let lo = (bytes[i + 2] as char)
                .to_digit(16)
                .and_then(|d| u8::try_from(d).ok())
                .ok_or_else(|| ServerRuntimeError::BadRequest("invalid form escape".to_owned()))?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|_| ServerRuntimeError::BadRequest("invalid UTF-8 in form value".to_owned()))
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
