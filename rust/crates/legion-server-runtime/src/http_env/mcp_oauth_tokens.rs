//! `POST /oauth/mcp/decision` and `POST /oauth/mcp/token` (PKCE S256) handlers.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, STANDARD_CLIENT_SCOPES, exchange_pairing_token_with_conn,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::{authenticate_headers, now_unix_ms, percent_decode_form};
use crate::http_env::mcp_oauth::{
    McpAuthorizeParams, mcp_approval_error, percent_encode_component, validate_authorize_params,
};
use crate::http_env::{
    EnvErr, EnvResult, constant_time_eq_bytes, env_error_json, env_internal, parse_json_body,
    resolve_request_origin, sign_b64url_payload, verify_b64url_payload,
};

const MCP_CODE_TTL_MS: i64 = 5 * 60 * 1_000;

fn scopes_for_access(access: &str) -> Vec<String> {
    match access {
        "read-only" => vec![
            "orchestration:read".to_owned(),
            "diagnostics:read".to_owned(),
            "terminal:read".to_owned(),
            "filesystem:read".to_owned(),
        ],
        "full-access" => ADMINISTRATIVE_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        _ => STANDARD_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    }
}

fn oauth_token_error(error: &str, description: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::BAD_REQUEST,
        json!({
            "error": error,
            "error_description": description.into(),
        }),
    )
}

#[derive(Debug, Serialize, Deserialize)]
struct SignedMcpAuthCode {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    scopes: Vec<String>,
    exp_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "_tag", rename_all = "kebab-case")]
enum McpDecision {
    Deny,
    PairingCode {
        access: String,
        code: String,
    },
    BrowserSession {
        access: String,
        #[serde(rename = "csrfToken")]
        csrf_token: String,
    },
}

#[derive(Debug, Deserialize)]
struct McpDecisionBody {
    authorization: McpAuthorizeParams,
    decision: McpDecision,
}

fn append_redirect_query(base: &str, params: &[(&str, &str)]) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    let encoded: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode_component(v)))
        .collect();
    format!("{base}{sep}{}", encoded.join("&"))
}

/// `POST /oauth/mcp/decision` — approves or denies an MCP OAuth authorization request.
pub(crate) async fn handle_mcp_decision(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    let req: McpDecisionBody = parse_json_body(&body)
        .map_err(|_| EnvErr(Box::new(mcp_approval_error("Invalid decision payload"))))?;
    let (_, redirect_uri, challenge) = validate_authorize_params(&state, &req.authorization)?;
    let client_id = req
        .authorization
        .client_id
        .as_deref()
        .unwrap_or_default()
        .to_owned();
    let origin = resolve_request_origin(&headers);
    let state_param = req.authorization.state.as_deref();

    let access_level = match req.decision {
        McpDecision::Deny => {
            let mut qp = vec![("error", "access_denied"), ("iss", origin.as_str())];
            if let Some(st) = state_param {
                qp.push(("state", st));
            }
            return Ok(
                Json(json!({ "redirectTo": append_redirect_query(&redirect_uri, &qp) }))
                    .into_response(),
            );
        }
        McpDecision::PairingCode { access, code } => {
            let conn = state
                .db()
                .lock()
                .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
            if exchange_pairing_token_with_conn(
                &conn,
                code.trim(),
                Some("mcp-oauth"),
                now_unix_ms(),
            )
            .is_err()
            {
                return Err(EnvErr(Box::new(mcp_approval_error(
                    "Invalid or expired pairing code",
                ))));
            }
            access
        }
        McpDecision::BrowserSession { access, csrf_token } => {
            let principal = authenticate_headers(&state, &headers).map_err(|_| {
                EnvErr(Box::new(mcp_approval_error(
                    "Active authenticated browser session required",
                )))
            })?;
            let verified = verify_b64url_payload(&state.signed_asset_secret, &csrf_token)
                .ok_or_else(|| EnvErr(Box::new(mcp_approval_error("Invalid CSRF token"))))?;
            let expected = format!("{}|{}", principal.session_id, challenge);
            if !constant_time_eq_bytes(&verified, expected.as_bytes()) {
                return Err(EnvErr(Box::new(mcp_approval_error("CSRF token mismatch"))));
            }
            access
        }
    };

    let auth_code = SignedMcpAuthCode {
        client_id,
        redirect_uri: redirect_uri.clone(),
        code_challenge: challenge,
        scopes: scopes_for_access(&access_level),
        exp_ms: now_unix_ms().saturating_add(MCP_CODE_TTL_MS),
    };
    let code_token = sign_b64url_payload(
        &state.signed_asset_secret,
        &serde_json::to_vec(&auth_code).unwrap_or_default(),
    );
    let mut qp = vec![("code", code_token.as_str()), ("iss", origin.as_str())];
    if let Some(st) = state_param {
        qp.push(("state", st));
    }
    Ok(Json(json!({ "redirectTo": append_redirect_query(&redirect_uri, &qp) })).into_response())
}

#[derive(Debug, Default, Deserialize)]
struct McpTokenBody {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
}

fn parse_mcp_token_body(headers: &HeaderMap, body: &[u8]) -> EnvResult<McpTokenBody> {
    let is_form = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("application/x-www-form-urlencoded"));
    if !is_form && body.trim_ascii_start().starts_with(b"{") {
        return serde_json::from_slice(body).map_err(|_| {
            EnvErr(Box::new(oauth_token_error(
                "invalid_request",
                "Malformed JSON body",
            )))
        });
    }
    let raw = std::str::from_utf8(body).map_err(|_| {
        EnvErr(Box::new(oauth_token_error(
            "invalid_request",
            "Form body is not valid UTF-8",
        )))
    })?;
    let mut parsed = McpTokenBody::default();
    for pair in raw.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode_form(k).map_err(|e| {
            EnvErr(Box::new(oauth_token_error(
                "invalid_request",
                e.to_string(),
            )))
        })?;
        let val = percent_decode_form(v).map_err(|e| {
            EnvErr(Box::new(oauth_token_error(
                "invalid_request",
                e.to_string(),
            )))
        })?;
        match key.as_str() {
            "grant_type" => parsed.grant_type = Some(val),
            "code" => parsed.code = Some(val),
            "redirect_uri" => parsed.redirect_uri = Some(val),
            "client_id" => parsed.client_id = Some(val),
            "code_verifier" => parsed.code_verifier = Some(val),
            _ => {}
        }
    }
    Ok(parsed)
}

/// `POST /oauth/mcp/token` — exchanges PKCE S256 authorization code for a scoped bearer token.
pub(crate) async fn handle_mcp_token(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    let req = parse_mcp_token_body(&headers, &body)?;
    if req.grant_type.as_deref() != Some("authorization_code") {
        return Err(EnvErr(Box::new(oauth_token_error(
            "unsupported_grant_type",
            "Only grant_type=authorization_code is supported",
        ))));
    }
    let (Some(code), Some(verifier), Some(client_id), Some(redirect_uri)) = (
        req.code.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        req.code_verifier
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        req.client_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        req.redirect_uri
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
    ) else {
        return Err(EnvErr(Box::new(oauth_token_error(
            "invalid_request",
            "code, code_verifier, client_id, and redirect_uri are required",
        ))));
    };

    let raw_claims = verify_b64url_payload(&state.signed_asset_secret, code).ok_or_else(|| {
        EnvErr(Box::new(oauth_token_error(
            "invalid_grant",
            "Authorization code signature is invalid",
        )))
    })?;
    let claims: SignedMcpAuthCode = serde_json::from_slice(&raw_claims).map_err(|_| {
        EnvErr(Box::new(oauth_token_error(
            "invalid_grant",
            "Malformed authorization code claims",
        )))
    })?;
    if claims.exp_ms <= now_unix_ms()
        || claims.client_id != client_id
        || claims.redirect_uri != redirect_uri
    {
        return Err(EnvErr(Box::new(oauth_token_error(
            "invalid_grant",
            "Authorization code expired or client mismatch",
        ))));
    }

    let computed_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if !constant_time_eq_bytes(
        computed_challenge.as_bytes(),
        claims.code_challenge.as_bytes(),
    ) {
        return Err(EnvErr(Box::new(oauth_token_error(
            "invalid_grant",
            "PKCE code_verifier verification failed",
        ))));
    }

    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let issued = issue_scoped_session(
        &conn,
        IssueSessionInput {
            scopes: claims.scopes,
            ttl_ms: Some(3600 * 1_000),
            label: Some("MCP OAuth client".to_owned()),
            subject: Some("mcp-oauth".to_owned()),
        },
    )
    .map_err(|_| {
        EnvErr(Box::new(env_internal(
            "failed to persist MCP OAuth bearer session",
        )))
    })?;

    Ok(Json(json!({
        "access_token": issued.token,
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": issued.scopes.join(" "),
    }))
    .into_response())
}

/// `POST /oauth/mcp/revoke` — RFC 7009 token revocation scoped strictly to MCP OAuth sessions (`#41005`).
pub(crate) async fn handle_mcp_revoke(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = parse_revocation_token(&headers, &body);
    if let Some(raw_token) = token.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let token_hash = hex::encode(Sha256::digest(raw_token.as_bytes()));
        let now_iso = crate::outbox_state::utc_now_iso();
        if let Ok(conn) = state.db().lock() {
            let _ = conn.execute(
                "UPDATE auth_sessions
                 SET revoked_at = ?1
                 WHERE bearer_token_hash = ?2
                   AND subject = 'mcp-oauth'
                   AND method = 'bearer-access-token'
                   AND revoked_at IS NULL",
                rusqlite::params![now_iso, token_hash],
            );
        }
    }
    let mut resp = StatusCode::OK.into_response();
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    resp
}

fn parse_revocation_token(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    let is_form = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("application/x-www-form-urlencoded"));
    if !is_form
        && body.trim_ascii_start().starts_with(b"{")
        && let Ok(val) = serde_json::from_slice::<serde_json::Value>(body)
    {
        return val.get("token").and_then(|v| v.as_str()).map(str::to_owned);
    }
    let raw = std::str::from_utf8(body).ok()?;
    for pair in raw.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode_form(k).ok().as_deref() == Some("token") {
            return percent_decode_form(v).ok();
        }
    }
    None
}
