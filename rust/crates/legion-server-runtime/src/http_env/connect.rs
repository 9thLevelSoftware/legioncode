//! `EnvironmentConnectHttpApi` `/api/connect/*` link state, preferences, and reachability handlers.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http_api::RuntimeHttpState;
use crate::http_auth::now_unix_ms;
use crate::http_env::connect_relay::{build_connect_descriptor, environment_id_for_state};
use crate::http_env::{
    EnvErr, EnvResult, authorize_env, env_internal, parse_json_body, sign_b64url_payload,
};
use crate::outbox_state::utc_now_iso;

/// In-memory state for Legion Connect environment linking and preferences.
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors EnvironmentCloudLinkStateResult wire booleans"
)]
pub(crate) struct ConnectRuntimeState {
    pub linked: bool,
    pub cloud_user_id: Option<String>,
    pub relay_url: Option<String>,
    pub relay_issuer: Option<String>,
    pub managed_tunnel_active: bool,
    pub publish_agent_activity: bool,
    pub hold_webhooks_while_offline: bool,
    pub live_activities_enabled: bool,
}

impl ConnectRuntimeState {
    fn link_state_json(&self) -> Value {
        json!({
            "linked": self.linked,
            "cloudUserId": self.cloud_user_id,
            "relayUrl": self.relay_url,
            "relayIssuer": self.relay_issuer,
            "managedTunnelActive": self.managed_tunnel_active,
            "publishAgentActivity": self.publish_agent_activity,
            "holdWebhooksWhileOffline": self.hold_webhooks_while_offline,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkProofBody {
    challenge: Option<String>,
    endpoint: Option<Value>,
    origin: Option<String>,
}

/// `POST /api/connect/link-proof`
pub(crate) async fn handle_connect_link_proof(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayConfigure))?;
    let req: LinkProofBody = if body.trim_ascii().is_empty() {
        LinkProofBody::default()
    } else {
        parse_json_body(&body)?
    };
    let env_id = environment_id_for_state(&state);
    let descriptor = build_connect_descriptor(env_id.clone());
    let payload = json!({
        "iat": now_unix_ms() / 1_000,
        "exp": (now_unix_ms() / 1_000) + 300,
        "challenge": req.challenge.unwrap_or_else(|| "link-challenge".to_owned()),
        "environmentId": env_id.as_str(),
        "descriptor": descriptor,
        "endpoint": req.endpoint,
        "origin": req.origin,
    });
    let proof = sign_b64url_payload(
        &state.signed_asset_secret,
        &serde_json::to_vec(&payload).unwrap_or_default(),
    );
    Ok(Json(Value::String(proof)).into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelayConfigBody {
    cloud_user_id: Option<String>,
    relay_url: Option<String>,
    relay_issuer: Option<String>,
    publish_agent_activity: Option<bool>,
    hold_webhooks_while_offline: Option<bool>,
}

/// `POST /api/connect/relay-config`
pub(crate) async fn handle_connect_relay_config(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayConfigure))?;
    let req: RelayConfigBody = parse_json_body(&body)?;
    let mut guard = state
        .connect_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("connect state lock poisoned"))))?;
    guard.linked = true;
    if let Some(uid) = req.cloud_user_id {
        guard.cloud_user_id = Some(uid);
    }
    if let Some(url) = req.relay_url {
        guard.relay_url = Some(url);
    }
    if let Some(issuer) = req.relay_issuer {
        guard.relay_issuer = Some(issuer);
    }
    if let Some(pub_act) = req.publish_agent_activity {
        guard.publish_agent_activity = pub_act;
    }
    if let Some(hold) = req.hold_webhooks_while_offline {
        guard.hold_webhooks_while_offline = hold;
    }
    Ok(Json(json!({
        "ok": true,
        "endpointRuntimeStatus": {
            "status": if guard.managed_tunnel_active { "connected" } else { "idle" }
        },
    }))
    .into_response())
}

/// `GET /api/connect/link-state`
pub(crate) async fn handle_connect_link_state(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayRead))?;
    let guard = state
        .connect_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("connect state lock poisoned"))))?;
    Ok(Json(guard.link_state_json()).into_response())
}

/// `POST /api/connect/reachability`
pub(crate) async fn handle_connect_reachability(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayRead))?;
    let checked_at = utc_now_iso();
    let guard = state
        .connect_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("connect state lock poisoned"))))?;
    if !guard.linked {
        return Ok(Json(json!({
            "status": "unreachable",
            "checkedAt": checked_at,
            "cause": "not_linked",
            "reason": "This environment is not linked to Legion Connect.",
            "hint": null,
            "fix": "Link this environment from Settings -> Connections.",
        }))
        .into_response());
    }
    if guard.managed_tunnel_active {
        return Ok(Json(json!({
            "status": "reachable",
            "checkedAt": checked_at,
            "readyConnections": 1,
            "protocol": "quic",
        }))
        .into_response());
    }
    Ok(Json(json!({
        "status": "unreachable",
        "checkedAt": checked_at,
        "cause": "connector_not_running",
        "reason": "Managed tunnel connector is not active.",
        "hint": null,
        "fix": "Start or reconnect the Legion Connect connector.",
    }))
    .into_response())
}

/// `POST /api/connect/unlink`
pub(crate) async fn handle_connect_unlink(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayConfigure))?;
    let mut guard = state
        .connect_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("connect state lock poisoned"))))?;
    *guard = ConnectRuntimeState::default();
    Ok(Json(json!({ "ok": true, "endpointRuntimeStatus": Value::Null })).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectPreferencesBody {
    publish_agent_activity: bool,
    hold_webhooks_while_offline: Option<bool>,
}

/// `POST /api/connect/preferences`
pub(crate) async fn handle_connect_preferences(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayWrite))?;
    let req: ConnectPreferencesBody = parse_json_body(&body)?;
    let mut guard = state
        .connect_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("connect state lock poisoned"))))?;
    guard.publish_agent_activity = req.publish_agent_activity;
    if let Some(hold) = req.hold_webhooks_while_offline {
        guard.hold_webhooks_while_offline = hold;
    }
    Ok(Json(guard.link_state_json()).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveActivityPreferenceBody {
    live_activities_enabled: bool,
}

/// `POST /api/connect/live-activity-preference`
pub(crate) async fn handle_connect_live_activity_preference(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> EnvResult<Response> {
    authorize_env(&state, &headers, Some(EnvironmentScope::RelayWrite))?;
    let req: LiveActivityPreferenceBody = parse_json_body(&body)?;
    let mut guard = state
        .connect_state
        .lock()
        .map_err(|_| EnvErr(Box::new(env_internal("connect state lock poisoned"))))?;
    guard.live_activities_enabled = req.live_activities_enabled;
    Ok(Json(json!({ "ok": true })).into_response())
}
