//! `POST /oauth/token` RFC 8693 Token Exchange handler.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, PairingTokenError, exchange_pairing_token_with_conn,
    verify_bearer_session_with_conn,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::{now_unix_ms, percent_decode_form};
use crate::http_env::{
    EnvErr, EnvResult, constant_time_eq_bytes, env_auth_invalid, env_internal, env_invalid_request,
    parse_json_body,
};

const ISSUED_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";
const GRANT_TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const SUBJECT_TYPE_ACCESS: &str = "urn:ietf:params:oauth:token-type:access_token";

#[derive(Debug, Default, Deserialize)]
struct OAuthTokenExchangeInput {
    grant_type: Option<String>,
    subject_token: Option<String>,
    subject_token_type: Option<String>,
    requested_token_type: Option<String>,
    token: Option<String>,
    scope: Option<String>,
    client_label: Option<String>,
}

fn parse_oauth_exchange_body(
    headers: &HeaderMap,
    body: &[u8],
) -> EnvResult<OAuthTokenExchangeInput> {
    let is_form = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("application/x-www-form-urlencoded"));
    if !is_form && body.trim_ascii_start().starts_with(b"{") {
        return parse_json_body(body);
    }
    let raw = std::str::from_utf8(body).map_err(|_| {
        EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            "Form body is not valid UTF-8",
        )))
    })?;
    let mut input = OAuthTokenExchangeInput::default();
    for pair in raw.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode_form(k).map_err(|e| {
            EnvErr(Box::new(env_invalid_request(
                "invalid_command",
                e.to_string(),
            )))
        })?;
        let val = percent_decode_form(v).map_err(|e| {
            EnvErr(Box::new(env_invalid_request(
                "invalid_command",
                e.to_string(),
            )))
        })?;
        match key.as_str() {
            "grant_type" => input.grant_type = Some(val),
            "subject_token" => input.subject_token = Some(val),
            "subject_token_type" => input.subject_token_type = Some(val),
            "requested_token_type" => input.requested_token_type = Some(val),
            "token" => input.token = Some(val),
            "scope" => input.scope = Some(val),
            "client_label" => input.client_label = Some(val),
            _ => {}
        }
    }
    Ok(input)
}

fn narrow_requested_scopes(granted: &[String], requested: Option<&str>) -> EnvResult<Vec<String>> {
    let Some(raw) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(granted.to_vec());
    };
    let mut selected = Vec::new();
    for tok in raw.split_whitespace() {
        let valid_scope: Option<EnvironmentScope> =
            serde_json::from_value(Value::String(tok.to_owned())).ok();
        if valid_scope.is_none() {
            return Err(EnvErr(Box::new(env_invalid_request(
                "invalid_scope",
                format!("Unknown OAuth scope `{tok}`"),
            ))));
        }
        if !granted.iter().any(|g| g == tok || g == "*") {
            return Err(EnvErr(Box::new(env_invalid_request(
                "scope_not_granted",
                format!("Scope `{tok}` was not granted to the credential"),
            ))));
        }
        if !selected.iter().any(|s| s == tok) {
            selected.push(tok.to_owned());
        }
    }
    Ok(selected)
}

/// `POST /oauth/token` — OAuth 2.0 Token Exchange (RFC 8693) supporting form-urlencoded and JSON.
pub(crate) async fn handle_oauth_token_exchange(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    let req = parse_oauth_exchange_body(&headers, &body)?;
    if let Some(ref grant) = req.grant_type
        && grant != GRANT_TOKEN_EXCHANGE
        && grant != "pairing_token"
    {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            format!("Unsupported grant_type `{grant}`"),
        ))));
    }
    if let Some(ref rtt) = req.requested_token_type
        && rtt != ISSUED_TOKEN_TYPE
    {
        return Err(EnvErr(Box::new(env_invalid_request(
            "invalid_command",
            format!("Unsupported requested_token_type `{rtt}`"),
        ))));
    }

    let Some(credential) = req
        .subject_token
        .as_deref()
        .or(req.token.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(EnvErr(Box::new(env_auth_invalid(
            "missing_credential",
            "subject_token is required",
        ))));
    };

    let now_ms = now_unix_ms();
    let label = req.client_label.as_deref();
    let has_dpop = headers
        .get("dpop")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.trim().is_empty());

    let (access_token, scopes) = exchange_credential(
        &state,
        credential,
        req.subject_token_type.as_deref(),
        req.scope.as_deref(),
        label,
        now_ms,
    )?;
    Ok(Json(json!({
        "access_token": access_token,
        "issued_token_type": ISSUED_TOKEN_TYPE,
        "token_type": if has_dpop { "DPoP" } else { "Bearer" },
        "expires_in": 3600,
        "scope": scopes.join(" "),
    }))
    .into_response())
}

fn exchange_credential(
    state: &RuntimeHttpState,
    credential: &str,
    subject_token_type: Option<&str>,
    requested_scope: Option<&str>,
    label: Option<&str>,
    now_ms: i64,
) -> EnvResult<(String, Vec<String>)> {
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    if state
        .dev_auth_token
        .as_deref()
        .is_some_and(|expected| constant_time_eq_bytes(expected.as_bytes(), credential.as_bytes()))
    {
        let granted: Vec<String> = ADMINISTRATIVE_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let narrowed = narrow_requested_scopes(&granted, requested_scope)?;
        let sess = issue_scoped_session(
            &conn,
            IssueSessionInput {
                scopes: narrowed,
                ttl_ms: Some(3600 * 1_000),
                label: label.map(str::to_owned),
                subject: Some("owner".to_owned()),
            },
        )
        .map_err(|e| EnvErr(Box::new(env_internal(e.to_string()))))?;
        return Ok((sess.token, sess.scopes));
    }

    if subject_token_type == Some(SUBJECT_TYPE_ACCESS) {
        let existing = verify_bearer_session_with_conn(&conn, credential, now_ms).map_err(|e| {
            EnvErr(Box::new(env_auth_invalid(
                "invalid_credential",
                e.to_string(),
            )))
        })?;
        let narrowed = narrow_requested_scopes(&existing.scopes, requested_scope)?;
        let sess = issue_scoped_session(
            &conn,
            IssueSessionInput {
                scopes: narrowed,
                ttl_ms: Some(3600 * 1_000),
                label: label.map(str::to_owned),
                subject: Some(existing.subject),
            },
        )
        .map_err(|e| EnvErr(Box::new(env_internal(e.to_string()))))?;
        return Ok((sess.token, sess.scopes));
    }

    match exchange_pairing_token_with_conn(&conn, credential, label, now_ms) {
        Ok(sess) => {
            let narrowed = narrow_requested_scopes(&sess.scopes, requested_scope)?;
            Ok((sess.token, narrowed))
        }
        Err(PairingTokenError::Sqlite { .. } | PairingTokenError::OpenDatabase { .. }) => Err(
            EnvErr(Box::new(env_internal("pairing token database failure"))),
        ),
        Err(err) => Err(EnvErr(Box::new(env_auth_invalid(
            "invalid_credential",
            err.to_string(),
        )))),
    }
}
