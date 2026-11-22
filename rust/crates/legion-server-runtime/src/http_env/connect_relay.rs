//! Public Legion Connect relay health and credential minting endpoints (`/api/legion-connect/*`).

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::EnvironmentId;
use legion_contracts::permissions::EnvironmentScope;
use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::paths::ensure_environment_id;
use serde::Deserialize;
use serde_json::json;

use crate::http_api::RuntimeHttpState;
use crate::http_auth::now_unix_ms;
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_auth_invalid, env_internal, parse_json_body,
    sign_b64url_payload,
};
use crate::outbox_state::utc_now_iso;

pub(crate) fn environment_id_for_state(state: &RuntimeHttpState) -> EnvironmentId {
    let base_dir = state.logs_dir.parent().unwrap_or(state.logs_dir.as_path());
    let env_path = base_dir.join("state").join("environment-id");
    let raw = ensure_environment_id(&env_path).unwrap_or_else(|_| "env-local".to_owned());
    EnvironmentId::new(raw)
        .unwrap_or_else(|_| EnvironmentId::new("env-local").expect("static id valid"))
}

pub(crate) fn build_connect_descriptor(env_id: EnvironmentId) -> ExecutionEnvironmentDescriptor {
    let os = if cfg!(target_os = "macos") {
        PlatformOs::Darwin
    } else if cfg!(target_os = "windows") {
        PlatformOs::Windows
    } else {
        PlatformOs::Linux
    };
    let arch = if cfg!(target_arch = "aarch64") {
        PlatformArch::Arm64
    } else {
        PlatformArch::X64
    };
    ExecutionEnvironmentDescriptor::builder(
        env_id,
        "LegionCode",
        PlatformInfo::new(os, arch),
        crate::SERVER_RUNTIME_VERSION,
    )
    .database_migration(105)
    .build()
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectHealthBody {
    request_nonce: Option<String>,
}

/// `POST /api/legion-connect/health`
pub(crate) async fn handle_legion_connect_health(
    State(state): State<Arc<RuntimeHttpState>>,
    body: Bytes,
) -> EnvResult<Response> {
    let req: ConnectHealthBody = if body.trim_ascii().is_empty() {
        ConnectHealthBody::default()
    } else {
        parse_json_body(&body)?
    };
    let env_id = environment_id_for_state(&state);
    let descriptor = build_connect_descriptor(env_id.clone());
    let checked_at = utc_now_iso();
    let proof_payload = json!({
        "environmentId": env_id.as_str(),
        "requestNonce": req.request_nonce.unwrap_or_default(),
        "status": "online",
        "checkedAt": checked_at,
    });
    let proof = sign_b64url_payload(
        &state.signed_asset_secret,
        &serde_json::to_vec(&proof_payload).unwrap_or_default(),
    );
    Ok(Json(json!({
        "environmentId": env_id.as_str(),
        "status": "online",
        "descriptor": descriptor,
        "checkedAt": checked_at,
        "proof": proof,
    }))
    .into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintCredentialBody {
    client_proof_key_thumbprint: Option<String>,
    request_nonce: Option<String>,
    scopes: Option<Vec<String>>,
    label: Option<String>,
}

/// `POST /api/legion-connect/mint-credential`
pub(crate) async fn handle_legion_connect_mint_credential(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    if state.require_auth
        && authorize_env(&state, &headers, Some(EnvironmentScope::AccessWrite)).is_err()
    {
        return Err(EnvErr(Box::new(env_auth_invalid(
            "invalid_credential",
            "Connect credential minting requires authorized relay/device proof",
        ))));
    }
    let req: MintCredentialBody = if body.trim_ascii().is_empty() {
        MintCredentialBody::default()
    } else {
        parse_json_body(&body)?
    };
    let mut input = IssuePairingTokenInput::default();
    if let Some(scopes) = req.scopes
        && !scopes.is_empty()
    {
        input = input.with_scopes(scopes);
    }
    if let Some(label) = req.label {
        input = input.with_label(label);
    }
    let conn = state
        .db()
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("database lock poisoned"))))?;
    let issued = issue_pairing_token_with_conn(&conn, &input, now_unix_ms())
        .map_err(|err| EnvErr(Box::new(env_internal(err.to_string()))))?;
    let env_id = environment_id_for_state(&state);
    let proof_payload = json!({
        "environmentId": env_id.as_str(),
        "clientProofKeyThumbprint": req.client_proof_key_thumbprint.unwrap_or_default(),
        "requestNonce": req.request_nonce.unwrap_or_default(),
        "credential": issued.credential,
    });
    let proof = sign_b64url_payload(
        &state.signed_asset_secret,
        &serde_json::to_vec(&proof_payload).unwrap_or_default(),
    );
    Ok(Json(json!({
        "credential": issued.credential,
        "expiresAt": issued.expires_at_iso,
        "proof": proof,
    }))
    .into_response())
}
