//! Outbound HTTP/SSE relay client helpers wired to [`PeerThreadRelayClient`] (`#39406`).

use serde_json::Value;
use t3_contracts::environment::ExecutionEnvironmentDescriptor;
use t3_contracts::ids::{EnvironmentId, ThreadId, TurnItemId};
use t3_contracts::peer_links::{
    PeerLinkThreadHistoryInput, PeerLinkThreadInput, PeerLinkThreadItemInput,
    PeerLinkThreadReadError, PeerLinkThreadReadReason,
};
use t3_remote::peer::links::{PeerLinksService, StoredPeerCredentials};
use t3_remote::peer::thread_relay::PeerThreadRelayClient;
use t3_remote::peer::thread_relay_helpers::from_link_error;

use crate::http_api::RuntimeHttpState;
use crate::outbox_state::utc_now_iso;

fn poisoned_db_error() -> PeerLinkThreadReadError {
    PeerLinkThreadReadError {
        reason: PeerLinkThreadReadReason::Unreachable,
        label: None,
        message: "db mutex poisoned".to_owned(),
    }
}

fn resolve_peer_origin_and_creds(
    state: &RuntimeHttpState,
    environment_id: &EnvironmentId,
) -> Result<(String, StoredPeerCredentials), PeerLinkThreadReadError> {
    let secrets = state
        .secrets_store()
        .map_err(|err| PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::Unreachable,
            label: None,
            message: err.to_string(),
        })?;
    let creds = {
        let conn = state.db().lock().map_err(|_| poisoned_db_error())?;
        PeerLinksService::new(secrets)
            .resolve_credentials(&conn, environment_id.as_str(), &utc_now_iso())
            .map_err(|err| from_link_error(&err, environment_id.as_str()))?
    };
    let Some(origin) = creds.link.urls.first().cloned() else {
        return Err(PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::Unreachable,
            label: Some(creds.link.label),
            message: "Linked peer has no configured URL.".to_owned(),
        });
    };
    Ok((origin, creds))
}

/// Outbound relay helper forwarding read-only thread stream requests to a linked peer
/// via [`PeerThreadRelayClient`].
pub async fn relay_peer_thread_stream(
    state: &RuntimeHttpState,
    environment_id: &EnvironmentId,
    thread_id: &ThreadId,
    capabilities: &ExecutionEnvironmentDescriptor,
) -> Result<Vec<Value>, PeerLinkThreadReadError> {
    let (origin, creds) = resolve_peer_origin_and_creds(state, environment_id)?;
    let client = PeerThreadRelayClient::new()?;
    let payload = client
        .stream_snapshot(
            &origin,
            &creds.link.label,
            &creds.token,
            &capabilities.capabilities,
            &PeerLinkThreadInput {
                environment_id: environment_id.clone(),
                thread_id: thread_id.clone(),
            },
        )
        .await?;
    Ok(payload.data)
}

/// Outbound relay helper reading a single turn item from a linked peer via [`PeerThreadRelayClient`].
pub async fn relay_peer_thread_item(
    state: &RuntimeHttpState,
    environment_id: &EnvironmentId,
    thread_id: &ThreadId,
    item_id: &TurnItemId,
    capabilities: &ExecutionEnvironmentDescriptor,
) -> Result<Value, PeerLinkThreadReadError> {
    let (origin, creds) = resolve_peer_origin_and_creds(state, environment_id)?;
    let client = PeerThreadRelayClient::new()?;
    let payload = client
        .item(
            &origin,
            &creds.link.label,
            &creds.token,
            &capabilities.capabilities,
            &PeerLinkThreadItemInput {
                environment_id: environment_id.clone(),
                thread_id: thread_id.clone(),
                item_id: item_id.clone(),
                revision: None,
            },
        )
        .await?;
    Ok(payload.data)
}

/// Outbound relay helper reading an older history page (`history_page`) from a linked peer.
pub async fn relay_peer_thread_history(
    state: &RuntimeHttpState,
    input: &PeerLinkThreadHistoryInput,
    capabilities: &ExecutionEnvironmentDescriptor,
) -> Result<Value, PeerLinkThreadReadError> {
    let (origin, creds) = resolve_peer_origin_and_creds(state, &input.environment_id)?;
    let client = PeerThreadRelayClient::new()?;
    let payload = client
        .history_page(
            &origin,
            &creds.link.label,
            &creds.token,
            &capabilities.capabilities,
            input,
        )
        .await?;
    Ok(payload.data)
}
