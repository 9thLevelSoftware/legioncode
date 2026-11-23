//! Extended Axum integration routes: `/api/peers`, `/api/providers/probe`, `/api/preview/action`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::join_all;
use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::permissions::EnvironmentScope;
use legion_persistence::ownership::format_epoch_millis_utc;
use legion_remote::live_network::LivePeerClient;
use legion_remote::peer::links::{
    InsertPeerLinkInput, PeerLinkStatus, PeerLinkSummary, PeerLinksService, validate_peer_origin,
};
use reqwest::{Client, StatusCode as ReqStatus, header::ACCEPT};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::{authorize_request, parse_authorized_json};
use crate::outbox_state::utc_now_iso;
use crate::preview_browser::PreviewAction;
use crate::provider_probe::ProviderProbeTarget;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkPeerRequest {
    url: String,
    #[serde(default)]
    alternate_urls: Vec<String>,
    pairing_code: String,
    access: Option<McpClientAccess>,
    client_label: Option<String>,
    self_environment_id: Option<EnvironmentId>,
    expected_environment_id: Option<EnvironmentId>,
    require_link_limits: Option<bool>,
}

struct PairTokenPayload {
    access_token: String,
    expires_at: Option<String>,
    expires_in: Option<i64>,
}

/// `GET /api/peers` — list linked peer environments with live reachability (`RelayRead`).
pub(crate) async fn handle_list_peers(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::RelayRead], false)?;
    let secrets = state.secrets_store()?;
    let links = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        PeerLinksService::new(secrets).list(&conn)?
    };
    let client = LivePeerClient::new()?;
    let now_iso = utc_now_iso();
    let summaries = join_all(links.into_iter().map(|link| {
        let client = client.clone();
        let now = now_iso.clone();
        let mut label = link.label;
        async move {
            let status = if link.expires_at.as_str() <= now.as_str() {
                PeerLinkStatus::Expired
            } else {
                let mut found = None;
                for url in &link.urls {
                    if let Ok((_, desc)) = client.probe_environment(url).await
                        && desc.environment_id == link.environment_id
                    {
                        found = Some(desc.label);
                        break;
                    }
                }
                if let Some(live_label) = found {
                    label = live_label;
                    PeerLinkStatus::Reachable
                } else {
                    PeerLinkStatus::Unreachable
                }
            };
            PeerLinkSummary {
                environment_id: link.environment_id,
                label,
                urls: link.urls,
                access: link.access,
                linked_at: link.linked_at,
                expires_at: link.expires_at,
                last_reached_at: link.last_reached_at,
                last_error: link.last_error,
                status,
            }
        }
    }))
    .await;
    Ok((StatusCode::OK, Json(json!({ "peers": summaries }))).into_response())
}

/// `POST /api/peers` — probe target, exchange pairing code, and persist peer link (`AccessWrite`).
pub(crate) async fn handle_link_peer(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, LinkPeerRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;
    let client = LivePeerClient::new()?;
    let (primary_origin, descriptor) = client.probe_environment(&req.url).await?;

    let mut origins = vec![primary_origin.clone()];
    let mut seen = HashSet::from([primary_origin.clone()]);
    for alt in &req.alternate_urls {
        let alt_origin = validate_peer_origin(alt)?;
        if seen.insert(alt_origin.clone()) {
            origins.push(alt_origin);
        }
    }
    if let Some(expected) = &req.expected_environment_id
        && descriptor.environment_id != *expected
    {
        return Err(ServerRuntimeError::Conflict(format!(
            "{primary_origin} answered as another environment `{}`",
            descriptor.environment_id
        )));
    }
    if req.self_environment_id.as_ref() == Some(&descriptor.environment_id) {
        return Err(ServerRuntimeError::BadRequest(
            "cannot link an environment to itself".to_owned(),
        ));
    }
    if req.require_link_limits.unwrap_or(true)
        && (descriptor.capabilities.mcp_mode_limit_header != Some(true)
            || descriptor.capabilities.link_fence != Some(true))
    {
        return Err(ServerRuntimeError::BadRequest(format!(
            "{} does not advertise link-fence/mode-limit capabilities",
            descriptor.label
        )));
    }

    let secrets = state.secrets_store()?;
    let linked_at = utc_now_iso();
    {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        if req.expected_environment_id.is_none()
            && let Some(existing) = PeerLinksService::new(secrets.clone())
                .get(&conn, descriptor.environment_id.as_str())?
            && existing.expires_at.as_str() > linked_at.as_str()
            && !existing.urls.contains(&primary_origin)
        {
            return Err(ServerRuntimeError::Conflict(format!(
                "{} is already linked at another address",
                existing.label
            )));
        }
    }

    let client_label = req
        .client_label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("legion-server-runtime");
    let pair = exchange_peer_pairing_code(&primary_origin, &req.pairing_code, client_label).await?;
    let expires_at = pair
        .expires_at
        .unwrap_or_else(|| expires_utc_iso(pair.expires_in.unwrap_or(30 * 24 * 3600)));

    let linked = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        PeerLinksService::new(secrets).insert(
            &conn,
            InsertPeerLinkInput {
                environment_id: descriptor.environment_id,
                label: descriptor.label,
                urls: origins,
                access: req.access.unwrap_or(McpClientAccess::Auto),
                access_token: pair.access_token,
                linked_at,
                expires_at,
            },
        )?
    };
    Ok((StatusCode::CREATED, Json(json!({ "peer": linked }))).into_response())
}

/// `DELETE /api/peers/{id}` — unlink peer environment, scrub secret, and revoke remote MCP token (`#41005`).
pub(crate) async fn handle_unlink_peer(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(&state, &headers, &[EnvironmentScope::AccessWrite], true)?;
    let secrets = state.secrets_store()?;
    let target = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        PeerLinksService::new(secrets).unlink_with_revocation_target(&conn, &id)?
    };
    let Some(target) = target else {
        return Err(ServerRuntimeError::NotFound(format!(
            "linked peer `{id}` not found"
        )));
    };
    let ended_there = if let Some(ref token) = target.token {
        LivePeerClient::new()?
            .revoke_peer_token(&target.environment_id, &target.urls, token)
            .await
    } else {
        false
    };
    Ok((
        StatusCode::OK,
        Json(json!({
            "unlinked": true,
            "removed": true,
            "endedThere": ended_there,
            "environmentId": id,
        })),
    )
        .into_response())
}

/// `POST /api/providers/probe` — execute live provider/speech health probe (`ProvidersManage`).
pub(crate) async fn handle_provider_probe(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, target): (_, ProviderProbeTarget) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::ProvidersManage],
        true,
        &body,
    )?;
    let result = state.probe_service().run_probe(target).await?;
    Ok((StatusCode::OK, Json(result)).into_response())
}

/// `POST /api/preview/action` — execute live headless CDP browser action (`PreviewOperate`).
pub(crate) async fn handle_preview_action(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, action): (_, PreviewAction) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::PreviewOperate],
        true,
        &body,
    )?;
    let result = state.preview_browser().execute_action(action).await?;
    Ok((StatusCode::OK, Json(result)).into_response())
}

async fn exchange_peer_pairing_code(
    origin: &str,
    pairing_code: &str,
    client_label: &str,
) -> Result<PairTokenPayload, ServerRuntimeError> {
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| {
            ServerRuntimeError::Upstream(format!("failed to build HTTP client: {err}"))
        })?;
    let resp = http
        .post(format!("{origin}/api/auth/pair"))
        .header(ACCEPT, "application/json")
        .json(&json!({ "token": pairing_code, "clientLabel": client_label }))
        .send()
        .await
        .map_err(|err| ServerRuntimeError::Upstream(format!("{origin} pairing exchange: {err}")))?;
    let status = resp.status();
    if matches!(
        status,
        ReqStatus::BAD_REQUEST | ReqStatus::UNAUTHORIZED | ReqStatus::FORBIDDEN
    ) {
        let body = resp.text().await.unwrap_or_default();
        return Err(ServerRuntimeError::BadRequest(format!(
            "peer `{origin}` rejected pairing code ({}): {body}",
            status.as_u16()
        )));
    }
    if !status.is_success() {
        return Err(ServerRuntimeError::Upstream(format!(
            "peer `{origin}` returned HTTP {} from /api/auth/pair",
            status.as_u16()
        )));
    }
    let val: Value = resp.json().await.map_err(|err| {
        ServerRuntimeError::Upstream(format!(
            "invalid /api/auth/pair JSON from `{origin}`: {err}"
        ))
    })?;
    let access_token = val
        .get("accessToken")
        .or_else(|| val.get("access_token"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ServerRuntimeError::Upstream(format!(
                "missing accessToken in `{origin}` /api/auth/pair"
            ))
        })?
        .to_owned();
    let expires_at = val
        .get("expiresAt")
        .or_else(|| val.get("expires_at"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let expires_in = val
        .get("expiresIn")
        .or_else(|| val.get("expires_in"))
        .and_then(Value::as_i64);
    Ok(PairTokenPayload {
        access_token,
        expires_at,
        expires_in,
    })
}

fn expires_utc_iso(expires_in_secs: i64) -> String {
    let now_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(i64::MAX);
    format_epoch_millis_utc(now_ms.saturating_add(expires_in_secs.saturating_mul(1_000)))
}
