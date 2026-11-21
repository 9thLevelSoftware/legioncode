//! Extended Axum authentication & access-control admin routes (`/api/auth/*`).

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use t3_contracts::permissions::EnvironmentScope;
use t3_persistence::auth_connect_devices::{
    ConnectApproveOutcome, ConnectRevokeOutcome, approve_connect_device_by_code,
    list_connect_devices, revoke_connect_device_by_code,
};
use t3_persistence::auth_mutations::{
    IssueSessionInput, issue_scoped_session, parse_cli_duration_ms, revoke_pairing_link,
    revoke_session,
};
use t3_persistence::auth_records::{list_active_pairing_links, list_active_sessions};
use t3_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::now_unix_ms;
use crate::http_auth_guard::{authorize_request, parse_authorized_json};
use crate::outbox_state::utc_now_iso;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePairingRequest {
    #[serde(default)]
    scopes: Vec<String>,
    ttl_ms: Option<i64>,
    ttl: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSessionRequest {
    #[serde(default)]
    scopes: Vec<String>,
    ttl_ms: Option<u64>,
    ttl: Option<String>,
    label: Option<String>,
    subject: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectDeviceCodeRequest {
    code: String,
}

/// `GET /api/auth/pairing` — list active unconsumed pairing links (`AccessRead`).
pub(crate) async fn handle_list_pairing(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::AccessRead], false)?;
    let now_iso = utc_now_iso();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let links = list_active_pairing_links(&conn, &now_iso)?;
    Ok((StatusCode::OK, Json(json!({ "links": links }))).into_response())
}

/// `POST /api/auth/pairing` — create a one-time pairing credential (`AccessWrite`).
pub(crate) async fn handle_create_pairing(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, CreatePairingRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;
    let mut input = IssuePairingTokenInput::default();
    if !req.scopes.is_empty() {
        input = input.with_scopes(req.scopes);
    }
    if let Some(ttl_ms) = resolve_pairing_ttl_ms(req.ttl_ms, req.ttl.as_deref())? {
        input = input.with_ttl_ms(ttl_ms);
    }
    if let Some(label) = req.label {
        input = input.with_label(label);
    }

    let now_ms = now_unix_ms();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let issued = issue_pairing_token_with_conn(&conn, &input, now_ms)?;
    Ok((StatusCode::CREATED, Json(issued)).into_response())
}

/// `DELETE /api/auth/pairing/{id}` — revoke an unconsumed pairing link (`AccessWrite`).
pub(crate) async fn handle_revoke_pairing(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::AccessWrite], true)?;
    let revoked = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        revoke_pairing_link(&conn, &id)?
    };
    if !revoked {
        return Err(ServerRuntimeError::NotFound(format!(
            "active pairing link `{id}` not found"
        )));
    }
    Ok((StatusCode::OK, Json(json!({ "revoked": true, "id": id }))).into_response())
}

/// `GET /api/auth/sessions` — list active sessions (`AccessRead`).
pub(crate) async fn handle_list_sessions(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::AccessRead], false)?;
    let now_iso = utc_now_iso();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let sessions = list_active_sessions(&conn, &now_iso)?;
    Ok((StatusCode::OK, Json(json!({ "sessions": sessions }))).into_response())
}

/// `POST /api/auth/sessions` — issue a scoped bearer session (`AccessWrite`).
pub(crate) async fn handle_issue_session(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, CreateSessionRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;
    let mut input = IssueSessionInput::default();
    if !req.scopes.is_empty() {
        input = input.with_scopes(req.scopes);
    }
    if let Some(ttl_ms) = resolve_session_ttl_ms(req.ttl_ms, req.ttl.as_deref())? {
        input = input.with_ttl_ms(ttl_ms);
    }
    if let Some(label) = req.label {
        input = input.with_label(label);
    }
    if let Some(subject) = req.subject {
        input = input.with_subject(subject);
    }

    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let issued = issue_scoped_session(&conn, input)?;
    Ok((StatusCode::CREATED, Json(issued)).into_response())
}

/// `DELETE /api/auth/sessions/{id}` — revoke an active session (`AccessWrite`).
pub(crate) async fn handle_revoke_session(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::AccessWrite], true)?;
    let revoked = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        revoke_session(&conn, &id)?
    };
    if !revoked {
        return Err(ServerRuntimeError::NotFound(format!(
            "active session `{id}` not found"
        )));
    }
    Ok((
        StatusCode::OK,
        Json(json!({ "revoked": true, "sessionId": id })),
    )
        .into_response())
}

/// `GET /api/auth/connect-devices` — list waiting & trusted T3 Connect devices (`AccessRead`).
pub(crate) async fn handle_list_connect_devices(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::AccessRead], false)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let (waiting, trusted) = list_connect_devices(&conn)?;
    Ok((
        StatusCode::OK,
        Json(json!({ "waiting": waiting, "trusted": trusted })),
    )
        .into_response())
}

/// `POST /api/auth/connect-devices/approve` — approve a waiting Connect device (`AccessWrite`).
pub(crate) async fn handle_approve_connect_device(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, ConnectDeviceCodeRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;
    let outcome = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        approve_connect_device_by_code(&conn, &req.code)?
    };
    match outcome {
        ConnectApproveOutcome::Approved {
            pairing_link_id,
            proof_key_thumbprint,
            verification_code,
        } => Ok((
            StatusCode::OK,
            Json(json!({
                "approved": true,
                "pairingLinkId": pairing_link_id,
                "proofKeyThumbprint": proof_key_thumbprint,
                "verificationCode": verification_code,
            })),
        )
            .into_response()),
        ConnectApproveOutcome::NotFound => Err(ServerRuntimeError::NotFound(format!(
            "no waiting Connect device matching `{}`",
            req.code
        ))),
        ConnectApproveOutcome::Ambiguous { match_count } => {
            Err(ServerRuntimeError::Conflict(format!(
                "verification code `{}` matched {match_count} waiting devices",
                req.code
            )))
        }
    }
}

/// `POST /api/auth/connect-devices/revoke` — revoke a trusted Connect device (`AccessWrite`).
pub(crate) async fn handle_revoke_connect_device(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, ConnectDeviceCodeRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;
    let outcome = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        revoke_connect_device_by_code(&conn, &req.code)?
    };
    match outcome {
        ConnectRevokeOutcome::Revoked {
            proof_key_thumbprint,
        } => Ok((
            StatusCode::OK,
            Json(json!({
                "revoked": true,
                "proofKeyThumbprint": proof_key_thumbprint,
            })),
        )
            .into_response()),
        ConnectRevokeOutcome::NotFound => Err(ServerRuntimeError::NotFound(format!(
            "no trusted Connect device matching `{}`",
            req.code
        ))),
        ConnectRevokeOutcome::Ambiguous { match_count } => {
            Err(ServerRuntimeError::Conflict(format!(
                "verification code `{}` matched {match_count} trusted devices",
                req.code
            )))
        }
    }
}

fn resolve_pairing_ttl_ms(
    ttl_ms: Option<i64>,
    ttl: Option<&str>,
) -> Result<Option<i64>, ServerRuntimeError> {
    if let Some(ms) = ttl_ms {
        return Ok(Some(ms));
    }
    let Some(raw) = ttl.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let parsed = parse_cli_duration_ms(raw).map_err(ServerRuntimeError::BadRequest)?;
    Ok(Some(i64::try_from(parsed).unwrap_or(i64::MAX)))
}

fn resolve_session_ttl_ms(
    ttl_ms: Option<u64>,
    ttl: Option<&str>,
) -> Result<Option<u64>, ServerRuntimeError> {
    if let Some(ms) = ttl_ms {
        return Ok(Some(ms));
    }
    let Some(raw) = ttl.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let parsed = parse_cli_duration_ms(raw).map_err(ServerRuntimeError::BadRequest)?;
    Ok(Some(parsed))
}
