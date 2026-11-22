//! Transitive peer & client pairing HTTP handlers (`#40036`) wired to `t3_remote::peer::client_pairing`.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use t3_contracts::environment::ExecutionEnvironmentDescriptor;
use t3_contracts::ids::EnvironmentId;
use t3_contracts::peer_links::{
    PeerLinkClientPairingRequest, PeerLinkClientPairingResult, PeerLinkLinkPairingInput,
    PeerMcpClientAccess,
};
use t3_contracts::permissions::EnvironmentScope;
use t3_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use t3_remote::peer::client_pairing::{
    PeerPairingClient, filter_standard_client_scopes, format_transitive_pairing_label,
};
use t3_remote::peer::links::PeerLinksService;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::now_unix_ms;
use crate::http_auth_guard::parse_authorized_json;
use crate::outbox_state::utc_now_iso;

const DEFAULT_TRANSITIVE_PAIRING_TTL_MS: i64 = 10 * 60 * 1_000;

const STANDARD_DELEGABLE_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "terminal:read",
    "terminal:operate",
    "filesystem:read",
    "preview:operate",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemotePeerPairRequest {
    environment_id: EnvironmentId,
    #[serde(default)]
    access: Option<PeerMcpClientAccess>,
    #[serde(default)]
    client_label: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    capabilities: ExecutionEnvironmentDescriptor,
    peer_url: Option<String>,
}

/// `POST /api/links/client-pairing` — issues a local credential for a transitive client or link (`#40036`).
pub(crate) async fn handle_issue_client_pairing(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, PeerLinkClientPairingRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;

    if !state.linked_environments_enabled {
        return Err(ServerRuntimeError::Forbidden(
            "Linked environments are disabled on this server.".to_owned(),
        ));
    }

    let caller = req.client_label.trim();
    let is_link_pairing = req.link.is_some();
    let label = format_transitive_pairing_label(caller, Some(caller), is_link_pairing);

    let scopes = if let Some(link_access) = req.link {
        match link_access.access {
            PeerMcpClientAccess::FullAccess
            | PeerMcpClientAccess::Auto
            | PeerMcpClientAccess::ApprovalRequired => vec![
                EnvironmentScope::OrchestrationRead.as_str().to_owned(),
                EnvironmentScope::OrchestrationOperate.as_str().to_owned(),
                EnvironmentScope::AccessRead.as_str().to_owned(),
                EnvironmentScope::AccessWrite.as_str().to_owned(),
            ],
            PeerMcpClientAccess::ReadOnly => {
                vec![EnvironmentScope::OrchestrationRead.as_str().to_owned()]
            }
        }
    } else {
        let filtered = filter_standard_client_scopes(&req.scopes, STANDARD_DELEGABLE_SCOPES);
        if filtered.is_empty() {
            return Err(ServerRuntimeError::Forbidden(
                "No delegable client scopes requested for transitive pairing.".to_owned(),
            ));
        }
        filtered
    };

    let now_ms = now_unix_ms();
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    let issued = issue_pairing_token_with_conn(
        &conn,
        &IssuePairingTokenInput {
            scopes,
            ttl_ms: DEFAULT_TRANSITIVE_PAIRING_TTL_MS,
            label: Some(label),
        },
        now_ms,
    )?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "credential": issued.credential,
            "expiresAt": format!("{}ms", issued.expires_at_ms),
            "expiresAtMs": issued.expires_at_ms,
        })),
    )
        .into_response())
}

/// `POST /api/peers/pair-link` — asks an already-linked peer for a transitive pairing code (`#40036`).
pub(crate) async fn handle_pair_link_through_peer(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (principal, req): (_, RemotePeerPairRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::AccessWrite],
        true,
        &body,
    )?;

    let client_label = req
        .client_label
        .unwrap_or_else(|| format!("session:{}", principal.session_id));
    let secrets = state.secrets_store()?;
    let creds = {
        let conn = state
            .db()
            .lock()
            .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
        PeerLinksService::new(secrets)
            .resolve_credentials(&conn, req.environment_id.as_str(), &utc_now_iso())
            .map_err(|err| ServerRuntimeError::Conflict(err.message))?
    };

    let peer_url = req
        .peer_url
        .or_else(|| creds.link.urls.first().cloned())
        .ok_or_else(|| {
            ServerRuntimeError::BadRequest("Linked peer has no reachable URL".to_owned())
        })?;

    let pairing_client =
        PeerPairingClient::new().map_err(|err| ServerRuntimeError::Upstream(err.message))?;

    let result: PeerLinkClientPairingResult = if let Some(access) = req.access {
        pairing_client
            .request_link_pairing(
                &creds.link,
                &req.capabilities.capabilities,
                &peer_url,
                &creds.token,
                &client_label,
                &PeerLinkLinkPairingInput {
                    environment_id: req.environment_id,
                    access,
                },
            )
            .await
            .map_err(|err| ServerRuntimeError::Upstream(err.message))?
    } else {
        pairing_client
            .request_pairing(
                &creds.link,
                &req.capabilities.capabilities,
                &peer_url,
                &creds.token,
                PeerLinkClientPairingRequest {
                    scopes: req.scopes,
                    client_label,
                    link: None,
                },
            )
            .await
            .map_err(|err| ServerRuntimeError::Upstream(err.message))?
    };

    Ok((StatusCode::OK, Json(result)).into_response())
}
