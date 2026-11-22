//! Authentication endpoints (`/api/auth/pair`, `/oauth/token`) and `BUG-03` credential guard.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, PRAGMA, SET_COOKIE};
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
    /// Subject or role label.
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

#[derive(Debug, Deserialize)]
struct OAuthTokenRequest {
    grant_type: Option<String>,
    subject_token: Option<String>,
    token: Option<String>,
    client_label: Option<String>,
}

#[derive(Debug, Serialize)]
struct OAuthTokenResponse {
    access_token: String,
    issued_token_type: &'static str,
    token_type: &'static str,
    expires_in: i64,
    scope: String,
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
    let primary_cookie = cookie_hdr
        .as_deref()
        .and_then(|raw| extract_cookie_value(raw, &state.cookie_name));
    let dev_cookie = cookie_hdr
        .as_deref()
        .and_then(|raw| extract_cookie_value(raw, "legioncode_dev_auth"));

    let selected = match CredentialPrecedence::select(&RequestCredentials {
        authorization: auth_hdr.as_deref(),
        primary_cookie: primary_cookie.as_deref(),
        legacy_cookie: None,
        dev_cookie: dev_cookie.as_deref(),
    }) {
        Ok(sel) => sel,
        Err(err) => {
            if auth_hdr.is_some() || primary_cookie.is_some() || dev_cookie.is_some() {
                return Err(ServerRuntimeError::Unauthorized(err.to_string()));
            }
            if !state.require_auth {
                return Ok(AuthenticatedPrincipal {
                    session_id: "local".to_owned(),
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

/// `POST /oauth/token` — OAuth 2.0 RFC 8693 token exchange accepting JSON or form-urlencoded.
pub(crate) async fn handle_oauth_token(
    State(state): State<std::sync::Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let req = parse_oauth_body(&headers, &body)?;
    if let Some(ref grant) = req.grant_type
        && grant != "urn:ietf:params:oauth:grant-type:token-exchange"
        && grant != "pairing_token"
    {
        return Err(ServerRuntimeError::BadRequest(format!(
            "unsupported grant_type `{grant}`"
        )));
    }
    let presented = req
        .subject_token
        .or(req.token)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServerRuntimeError::Unauthorized("missing subject_token".to_owned()))?;
    let exchanged = exchange_token(&state, &presented, req.client_label.as_deref())?;
    let payload = OAuthTokenResponse {
        access_token: exchanged.token.clone(),
        issued_token_type: "urn:ietf:params:oauth:token-type:access_token",
        token_type: "Bearer",
        expires_in: 3600,
        scope: exchanged.scopes.join(" "),
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

fn parse_oauth_body(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<OAuthTokenRequest, ServerRuntimeError> {
    let is_json = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("application/json"));
    if is_json || body.first().copied() == Some(b'{') {
        return serde_json::from_slice(body).map_err(Into::into);
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| ServerRuntimeError::BadRequest("invalid UTF-8 form body".to_owned()))?;
    let mut map: HashMap<String, String> = HashMap::new();
    for pair in text.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode_form(k)?, percent_decode_form(v)?);
    }
    Ok(OAuthTokenRequest {
        grant_type: map.remove("grant_type"),
        subject_token: map.remove("subject_token"),
        token: map.remove("token"),
        client_label: map.remove("client_label"),
    })
}

fn percent_decode_form(raw: &str) -> Result<String, ServerRuntimeError> {
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
