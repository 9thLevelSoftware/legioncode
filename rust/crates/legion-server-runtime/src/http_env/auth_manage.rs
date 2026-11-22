//! `EnvironmentAuthHttpApi` credential/client/device management handlers.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_persistence::auth_connect_devices::{
    ConnectApproveOutcome, ConnectRevokeOutcome, approve_connect_device_by_code,
    connect_device_verification_code, revoke_connect_device_by_code,
};
use legion_persistence::auth_mutations::{
    revoke_other_sessions, revoke_pairing_link, revoke_session,
};
use legion_persistence::ownership::format_epoch_millis_utc;
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, DEFAULT_PAIRING_TOKEN_TTL_MS, IssuePairingTokenInput,
    STANDARD_CLIENT_SCOPES, issue_pairing_token_with_conn, revoke_unspent_pairing_links,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::now_unix_ms;
use crate::http_env::auth_support::split_scopes_and_permissions;
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_bad_request, env_conflict, env_internal, env_not_found,
    parse_json_body,
};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairingTokenCreateBody {
    #[serde(default)]
    scopes: Vec<String>,
    role: Option<String>,
    label: Option<String>,
    ttl_ms: Option<i64>,
    ttl_minutes: Option<i64>,
    purpose: Option<String>,
    access: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevokePairingLinkBody {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevokeClientBody {
    session_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevokeOtherClientsBody {
    keep_session_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApproveConnectDeviceBody {
    code: Option<String>,
    pairing_link_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevokeConnectDeviceBody {
    code: Option<String>,
    proof_key_thumbprint: Option<String>,
}

/// `POST /api/auth/pairing-token` — creates a one-time pairing credential (`AccessWrite`).
pub(crate) async fn handle_post_pairing_token(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite))?;
    let req: PairingTokenCreateBody = if body.trim_ascii().is_empty() {
        PairingTokenCreateBody::default()
    } else {
        parse_json_body(&body)?
    };

    let ttl_ms = req
        .ttl_ms
        .or_else(|| req.ttl_minutes.map(|m| m.saturating_mul(60_000)))
        .unwrap_or(DEFAULT_PAIRING_TOKEN_TTL_MS);
    if ttl_ms <= 0 {
        return Err(EnvErr(Box::new(env_bad_request(
            "ttlMs must be greater than 0",
        ))));
    }

    let scopes: Vec<String> = if !req.scopes.is_empty() {
        req.scopes
    } else if req.role.as_deref() == Some("owner") {
        ADMINISTRATIVE_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else {
        STANDARD_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    };

    let mut input = IssuePairingTokenInput::default()
        .with_ttl_ms(ttl_ms)
        .with_scopes(scopes.clone());
    if let Some(ref label) = req.label
        && !label.trim().is_empty()
    {
        input = input.with_label(label.trim());
    }

    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let issued = issue_pairing_token_with_conn(&conn, &input, now_unix_ms())
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;

    let (legacy_scopes, permissions) = split_scopes_and_permissions(&scopes);
    let mut obj = Map::new();
    obj.insert("id".to_owned(), Value::String(issued.id));
    obj.insert("credential".to_owned(), Value::String(issued.credential));
    obj.insert("expiresAt".to_owned(), Value::String(issued.expires_at_iso));
    obj.insert("scopes".to_owned(), json!(legacy_scopes));
    obj.insert("permissions".to_owned(), json!(permissions));
    if let Some(label) = req
        .label
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
    {
        obj.insert("label".to_owned(), Value::String(label));
    }
    if let Some(purpose) = req.purpose {
        obj.insert("purpose".to_owned(), Value::String(purpose));
    }
    if let Some(access) = req.access {
        obj.insert("access".to_owned(), Value::String(access));
    }
    Ok(Json(Value::Object(obj)).into_response())
}

/// `POST /api/auth/pairing-links/revoke` — revokes an unconsumed pairing link (`AccessWrite`).
pub(crate) async fn handle_post_revoke_pairing_link(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite))?;
    let req: RevokePairingLinkBody = parse_json_body(&body)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let revoked = revoke_pairing_link(&conn, req.id.trim())
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
    Ok(Json(json!({ "revoked": revoked })).into_response())
}

/// `POST /api/auth/clients/revoke` — revokes a single client session (`AccessWrite`).
pub(crate) async fn handle_post_revoke_client(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite))?;
    let req: RevokeClientBody = parse_json_body(&body)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let revoked = revoke_session(&conn, req.session_id.trim())
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
    Ok(Json(json!({ "revoked": revoked })).into_response())
}

/// `POST /api/auth/clients/revoke-others` — revokes all other client sessions (`AccessWrite`).
pub(crate) async fn handle_post_revoke_other_clients(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    let principal = authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite))?;
    let req: RevokeOtherClientsBody = if body.trim_ascii().is_empty() {
        RevokeOtherClientsBody::default()
    } else {
        parse_json_body(&body)?
    };
    let keep = req
        .keep_session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&principal.session_id);
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let outcome = revoke_other_sessions(&conn, keep)
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
    Ok(Json(json!({
        "revokedCount": outcome.revoked_sessions,
        "revokedSessionCount": outcome.revoked_sessions,
        "revokedPairingLinkCount": outcome.revoked_pairing_links,
        "removedConnectDeviceCount": outcome.removed_connect_devices,
    }))
    .into_response())
}

/// `POST /api/auth/connect-devices/approve` — approves via `{ pairingLinkId }` or `{ code }`.
pub(crate) async fn handle_post_approve_connect_device(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite))?;
    let req: ApproveConnectDeviceBody = parse_json_body(&body)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;

    if let Some(link_id) = req
        .pairing_link_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(approve_connect_device_by_link_id(&conn, link_id));
    }

    let Some(code) = req.code.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(EnvErr(Box::new(env_bad_request(
            "Either pairingLinkId or code is required",
        ))));
    };
    match approve_connect_device_by_code(&conn, code) {
        Ok(ConnectApproveOutcome::Approved {
            pairing_link_id,
            proof_key_thumbprint,
            verification_code,
        }) => Ok(Json(json!({
            "approved": true,
            "pairingLinkId": pairing_link_id,
            "proofKeyThumbprint": proof_key_thumbprint,
            "verificationCode": verification_code,
        }))
        .into_response()),
        Ok(ConnectApproveOutcome::NotFound) => Err(EnvErr(Box::new(env_not_found(
            "Waiting Connect device not found",
        )))),
        Ok(ConnectApproveOutcome::Ambiguous { match_count }) => {
            Err(EnvErr(Box::new(env_conflict(format!(
                "Verification code matches {match_count} waiting Connect devices"
            )))))
        }
        Err(err) => Err(EnvErr(Box::new(env_internal(err.to_string())))),
    }
}

fn approve_connect_device_by_link_id(conn: &rusqlite::Connection, link_id: &str) -> Response {
    let now_iso = format_epoch_millis_utc(now_unix_ms());
    let row = conn.query_row(
        "SELECT proof_key_thumbprint, requested_client_json
         FROM auth_pairing_links
         WHERE id = ?1
           AND revoked_at IS NULL
           AND consumed_at IS NULL
           AND expires_at > ?2
           AND proof_key_thumbprint IS NOT NULL
         LIMIT 1",
        params![link_id, now_iso],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    );
    let (thumbprint, client_json) = match row {
        Ok(pair) => pair,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return env_not_found(format!("Waiting pairing link `{link_id}` not found"));
        }
        Err(err) => return env_internal(err.to_string()),
    };
    if let Err(err) = conn.execute(
        "INSERT INTO auth_connect_devices (proof_key_thumbprint, client_json, approved_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (proof_key_thumbprint) DO UPDATE
           SET client_json = COALESCE(excluded.client_json, auth_connect_devices.client_json)",
        params![thumbprint, client_json, now_iso],
    ) {
        return env_internal(err.to_string());
    }
    let verification_code = connect_device_verification_code(&thumbprint);
    Json(json!({
        "approved": true,
        "pairingLinkId": link_id,
        "proofKeyThumbprint": thumbprint,
        "verificationCode": verification_code,
    }))
    .into_response()
}

/// `POST /api/auth/connect-devices/revoke` — revokes via `{ proofKeyThumbprint }` or `{ code }`.
pub(crate) async fn handle_post_revoke_connect_device(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite))?;
    let req: RevokeConnectDeviceBody = parse_json_body(&body)?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;

    if let Some(thumb) = req
        .proof_key_thumbprint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let now_iso = format_epoch_millis_utc(now_unix_ms());
        let deleted = conn
            .execute(
                "DELETE FROM auth_connect_devices WHERE proof_key_thumbprint = ?1",
                [thumb],
            )
            .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
        let _ = revoke_unspent_pairing_links(&conn, Some(thumb), None, false, &now_iso);
        let sessions_revoked = conn
            .execute(
                "UPDATE auth_sessions SET revoked_at = ?1 WHERE proof_key_thumbprint = ?2 AND revoked_at IS NULL",
                params![now_iso, thumb],
            )
            .unwrap_or(0);
        if deleted == 0 && sessions_revoked == 0 {
            return Err(EnvErr(Box::new(env_not_found(
                "Trusted Connect device not found",
            ))));
        }
        return Ok(Json(json!({
            "revoked": true,
            "proofKeyThumbprint": thumb,
        }))
        .into_response());
    }

    let Some(code) = req.code.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(EnvErr(Box::new(env_bad_request(
            "Either proofKeyThumbprint or code is required",
        ))));
    };
    match revoke_connect_device_by_code(&conn, code) {
        Ok(ConnectRevokeOutcome::Revoked {
            proof_key_thumbprint,
        }) => Ok(Json(json!({
            "revoked": true,
            "proofKeyThumbprint": proof_key_thumbprint,
        }))
        .into_response()),
        Ok(ConnectRevokeOutcome::NotFound) => Err(EnvErr(Box::new(env_not_found(
            "Trusted Connect device not found",
        )))),
        Ok(ConnectRevokeOutcome::Ambiguous { match_count }) => Err(EnvErr(Box::new(env_conflict(
            format!("Verification code matches {match_count} Connect devices"),
        )))),
        Err(err) => Err(EnvErr(Box::new(env_internal(err.to_string())))),
    }
}
