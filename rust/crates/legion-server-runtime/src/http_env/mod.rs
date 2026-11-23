//! `EnvironmentHttpApi` endpoint handlers, typed error payloads, HMAC signing, and shared helpers.

pub(crate) mod assets;
pub(crate) mod auth;
pub(crate) mod auth_manage;
pub(crate) mod auth_support;
pub(crate) mod connect;
pub(crate) mod connect_relay;
pub(crate) mod cors;
pub(crate) mod mcp_oauth;
pub(crate) mod mcp_oauth_tokens;
pub(crate) mod oauth_token;
pub(crate) mod orchestration;
pub(crate) mod pull_requests;
pub(crate) mod voice;
pub(crate) mod voice_models;
pub(crate) mod voice_state;
pub(crate) mod webhooks;

use axum::Json;
use axum::http::header::HOST;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use legion_contracts::permissions::EnvironmentScope;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::{AuthenticatedPrincipal, authenticate_headers};
use crate::http_auth_guard::authorize_scopes;

/// Boxed error `Response` wrapper keeping `Result<T, EnvErr>` pointer-sized for Clippy.
pub(crate) struct EnvErr(pub Box<Response>);

pub(crate) type EnvResult<T> = Result<T, EnvErr>;

impl IntoResponse for EnvErr {
    fn into_response(self) -> Response {
        *self.0
    }
}

/// Builds a typed `EnvironmentHttpApi` JSON error response.
pub(crate) fn env_error_json(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

pub(crate) fn env_invalid_request(reason: &str, message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::BAD_REQUEST,
        json!({
            "_tag": "EnvironmentRequestInvalidError",
            "code": "invalid_request",
            "reason": reason,
            "traceId": Uuid::new_v4().to_string(),
            "message": message.into(),
        }),
    )
}

pub(crate) fn env_bad_request(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::BAD_REQUEST,
        json!({
            "_tag": "EnvironmentHttpBadRequestError",
            "message": message.into(),
        }),
    )
}

pub(crate) fn env_auth_invalid(reason: &str, message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::UNAUTHORIZED,
        json!({
            "_tag": "EnvironmentAuthInvalidError",
            "code": "auth_invalid",
            "reason": reason,
            "traceId": Uuid::new_v4().to_string(),
            "message": message.into(),
        }),
    )
}

pub(crate) fn env_scope_required(required: &[EnvironmentScope], message: &str) -> Response {
    let required_strs: Vec<&str> = required.iter().map(|s| s.as_str()).collect();
    env_error_json(
        StatusCode::FORBIDDEN,
        json!({
            "_tag": "EnvironmentScopeRequiredError",
            "code": "scope_required",
            "requiredScopes": required_strs,
            "message": message,
        }),
    )
}

pub(crate) fn env_forbidden(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::FORBIDDEN,
        json!({
            "_tag": "EnvironmentOperationForbiddenError",
            "code": "operation_forbidden",
            "message": message.into(),
        }),
    )
}

pub(crate) fn env_not_found(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::NOT_FOUND,
        json!({
            "_tag": "EnvironmentResourceNotFoundError",
            "message": message.into(),
        }),
    )
}

pub(crate) fn env_conflict(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::CONFLICT,
        json!({
            "_tag": "EnvironmentHttpConflictError",
            "message": message.into(),
        }),
    )
}

pub(crate) fn env_internal(message: impl Into<String>) -> Response {
    env_error_json(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({
            "_tag": "EnvironmentInternalError",
            "message": message.into(),
        }),
    )
}

pub(crate) fn map_runtime_error(err: ServerRuntimeError) -> Response {
    match err {
        ServerRuntimeError::BadRequest(msg) => env_bad_request(msg),
        ServerRuntimeError::Unauthorized(msg) => env_auth_invalid("invalid_credential", msg),
        ServerRuntimeError::Forbidden(msg) => env_forbidden(msg),
        ServerRuntimeError::NotFound(msg) => env_not_found(msg),
        ServerRuntimeError::Conflict(msg) => env_conflict(msg),
        other => env_internal(other.to_string()),
    }
}

/// Authenticates request headers and checks an optional [`EnvironmentScope`].
pub(crate) fn authorize_env(
    state: &RuntimeHttpState,
    headers: &HeaderMap,
    required_scope: Option<EnvironmentScope>,
) -> EnvResult<AuthenticatedPrincipal> {
    let principal = match authenticate_headers(state, headers) {
        Ok(p) => p,
        Err(ServerRuntimeError::Unauthorized(msg)) => {
            let has_cred = headers.contains_key("authorization") || headers.contains_key("cookie");
            let reason = if has_cred {
                "invalid_credential"
            } else {
                "missing_credential"
            };
            return Err(EnvErr(Box::new(env_auth_invalid(reason, msg))));
        }
        Err(other) => return Err(EnvErr(Box::new(map_runtime_error(other)))),
    };
    if let Some(scope) = required_scope {
        let is_mutation = !matches!(
            scope,
            EnvironmentScope::OrchestrationRead
                | EnvironmentScope::DiagnosticsRead
                | EnvironmentScope::TerminalRead
                | EnvironmentScope::FilesystemRead
                | EnvironmentScope::AccessRead
                | EnvironmentScope::RelayRead
        );
        if let Err(err) = authorize_scopes(&principal, &[scope], is_mutation) {
            return Err(EnvErr(Box::new(env_scope_required(
                &[scope],
                &err.to_string(),
            ))));
        }
    }
    Ok(principal)
}

/// Parses a JSON body or returns boxed `400 EnvironmentHttpBadRequestError`.
pub(crate) fn parse_json_body<T: DeserializeOwned>(body: &[u8]) -> EnvResult<T> {
    serde_json::from_slice(body).map_err(|err| {
        EnvErr(Box::new(env_bad_request(format!(
            "Invalid JSON payload: {err}"
        ))))
    })
}

/// Computes RFC 2104 HMAC-SHA256 using `sha2::Sha256`.
pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut padded = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        padded[..32].copy_from_slice(&hashed);
    } else {
        padded[..key.len()].copy_from_slice(key);
    }

    let mut inner = [0_u8; BLOCK_SIZE];
    let mut outer = [0_u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        inner[i] = padded[i] ^ 0x36;
        outer[i] = padded[i] ^ 0x5c;
    }

    let mut inner_hasher = Sha256::new();
    inner_hasher.update(inner);
    inner_hasher.update(data);
    let inner_digest = inner_hasher.finalize();

    let mut outer_hasher = Sha256::new();
    outer_hasher.update(outer);
    outer_hasher.update(inner_digest);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&outer_hasher.finalize());
    out
}

pub(crate) fn sign_b64url_payload(secret: &[u8], payload: &[u8]) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    let mac = hmac_sha256(secret, encoded.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac);
    format!("{encoded}.{sig}")
}

pub(crate) fn verify_b64url_payload(secret: &[u8], token: &str) -> Option<Vec<u8>> {
    let (encoded, sig_b64) = token.split_once('.')?;
    let expected = hmac_sha256(secret, encoded.as_bytes());
    let provided = URL_SAFE_NO_PAD.decode(sig_b64).ok()?;
    if provided.len() != expected.len() || !constant_time_eq_bytes(&expected, &provided) {
        return None;
    }
    URL_SAFE_NO_PAD.decode(encoded).ok()
}

pub(crate) fn constant_time_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(crate) fn resolve_request_origin(headers: &HeaderMap) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| *s == "http" || *s == "https")
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(HOST))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("127.0.0.1:3773");
    format!("{proto}://{host}")
}
