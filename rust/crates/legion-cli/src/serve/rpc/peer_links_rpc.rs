//! Peer-link read, name, client-pairing, and transitive link-pairing WS RPCs (`#39406`, `#40036`).

use legion_contracts::ids::EnvironmentId;
use legion_contracts::peer_links::{
    PeerLinkClientPairingRequest, PeerLinkClientPairingResult, PeerLinkErrorReason,
    PeerLinkLinkPairingInput, PeerLinkName, PeerLinkPairingUnsupportedReason,
    PeerLinkThreadHistoryInput, PeerLinkThreadItemInput, PeerLinkThreadReadError,
};
use legion_protocol::rpc_codec::RpcExit;
use legion_remote::live_network::LivePeerClient;
use legion_remote::peer::client_pairing::{PeerPairingClient, filter_standard_client_scopes};
use legion_remote::peer::links::{PeerLinkError as RemotePeerLinkError, PeerLinksService};
use legion_remote::secrets::FileSecretStore;
use legion_server_runtime::{relay_peer_thread_history, relay_peer_thread_item};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj, utc_now_iso, with_db};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairClientPayload {
    environment_id: EnvironmentId,
}

const STANDARD_DELEGABLE_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "terminal:read",
    "terminal:operate",
    "filesystem:read",
    "preview:operate",
];

pub(crate) async fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "peerLinks.names" => peer_links_names(state),
        "peerLinks.threadHistoryPage" => peer_links_thread_history_page(state, payload).await,
        "peerLinks.threadItem" => peer_links_thread_item(state, payload).await,
        "peerLinks.pairClient" => peer_links_pair_client(state, payload).await,
        "peerLinks.pairLink" => peer_links_pair_link(state, payload).await,
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn peer_links_names(state: &AppState) -> RpcExit {
    let Ok(secrets) = FileSecretStore::new(state.paths.secrets_dir.clone()) else {
        return peer_link_err(PeerLinkErrorReason::Storage, "failed to open secrets store");
    };
    let svc = PeerLinksService::new(secrets);
    let now = utc_now_iso();
    with_db(state, |conn| {
        let links = svc
            .list(conn)
            .map_err(|err| peer_link_err(PeerLinkErrorReason::Storage, &err.to_string()))?;
        let names: Vec<PeerLinkName> = links
            .into_iter()
            .filter(|link| link.expires_at.as_str() > now.as_str())
            .map(|link| PeerLinkName {
                environment_id: link.environment_id,
                label: link.label,
            })
            .collect();
        Ok(RpcExit::Success {
            value: json!({ "links": names }),
        })
    })
    .unwrap_or_else(|err| err)
}

async fn peer_links_thread_history_page(state: &AppState, payload: Value) -> RpcExit {
    let req: PeerLinkThreadHistoryInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match relay_peer_thread_history(&state.runtime_http, &req, &state.descriptor).await {
        Ok(val) => RpcExit::Success { value: val },
        Err(err) => map_thread_read_err(&err),
    }
}

async fn peer_links_thread_item(state: &AppState, payload: Value) -> RpcExit {
    let req: PeerLinkThreadItemInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match relay_peer_thread_item(
        &state.runtime_http,
        &req.environment_id,
        &req.thread_id,
        &req.item_id,
        &state.descriptor,
    )
    .await
    {
        Ok(val) => RpcExit::Success { value: val },
        Err(err) => map_thread_read_err(&err),
    }
}

async fn peer_links_pair_client(state: &AppState, payload: Value) -> RpcExit {
    let req: PairClientPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let scopes: Vec<String> = filter_standard_client_scopes(
        &STANDARD_DELEGABLE_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
        STANDARD_DELEGABLE_SCOPES,
    );
    let pairing_req = PeerLinkClientPairingRequest {
        scopes,
        client_label: "Local Desktop".to_owned(),
        link: None,
    };
    request_peer_pairing(state, &req.environment_id, pairing_req).await
}

async fn peer_links_pair_link(state: &AppState, payload: Value) -> RpcExit {
    let req: PeerLinkLinkPairingInput = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Ok(secrets) = FileSecretStore::new(state.paths.secrets_dir.clone()) else {
        return peer_link_err(PeerLinkErrorReason::Storage, "failed to open secrets store");
    };
    let creds = match with_db(state, |conn| {
        PeerLinksService::new(secrets)
            .resolve_credentials(conn, req.environment_id.as_str(), &utc_now_iso())
            .map_err(|err| map_peer_link_error(&err))
    }) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(peer_url) = creds.link.urls.first().cloned() else {
        return RpcExit::Success {
            value: json!(PeerLinkClientPairingResult::Unsupported {
                environment_id: creds.link.environment_id,
                label: creds.link.label,
                urls: creds.link.urls,
                reason: PeerLinkPairingUnsupportedReason::Unreachable,
            }),
        };
    };
    let live = match LivePeerClient::new() {
        Ok(c) => c,
        Err(err) => return map_peer_link_error(&err),
    };
    let caps = match live.probe_environment(&peer_url).await {
        Ok((_, desc)) => desc.capabilities,
        Err(_) => {
            return RpcExit::Success {
                value: json!(PeerLinkClientPairingResult::Unsupported {
                    environment_id: creds.link.environment_id,
                    label: creds.link.label,
                    urls: creds.link.urls,
                    reason: PeerLinkPairingUnsupportedReason::Unreachable,
                }),
            };
        }
    };
    let client = match PeerPairingClient::new() {
        Ok(c) => c,
        Err(err) => return map_peer_link_error(&err),
    };
    match client
        .request_link_pairing(
            &creds.link,
            &caps,
            &peer_url,
            &creds.token,
            "Local Server",
            &req,
        )
        .await
    {
        Ok(result) => RpcExit::Success {
            value: json!(result),
        },
        Err(err) => map_peer_link_error(&err),
    }
}

async fn request_peer_pairing(
    state: &AppState,
    environment_id: &EnvironmentId,
    pairing_req: PeerLinkClientPairingRequest,
) -> RpcExit {
    let Ok(secrets) = FileSecretStore::new(state.paths.secrets_dir.clone()) else {
        return peer_link_err(PeerLinkErrorReason::Storage, "failed to open secrets store");
    };
    let creds = match with_db(state, |conn| {
        PeerLinksService::new(secrets)
            .resolve_credentials(conn, environment_id.as_str(), &utc_now_iso())
            .map_err(|err| map_peer_link_error(&err))
    }) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(peer_url) = creds.link.urls.first().cloned() else {
        return RpcExit::Success {
            value: json!(PeerLinkClientPairingResult::Unsupported {
                environment_id: creds.link.environment_id,
                label: creds.link.label,
                urls: creds.link.urls,
                reason: PeerLinkPairingUnsupportedReason::Unreachable,
            }),
        };
    };
    let live = match LivePeerClient::new() {
        Ok(c) => c,
        Err(err) => return map_peer_link_error(&err),
    };
    let caps = match live.probe_environment(&peer_url).await {
        Ok((_, desc)) => desc.capabilities,
        Err(_) => {
            return RpcExit::Success {
                value: json!(PeerLinkClientPairingResult::Unsupported {
                    environment_id: creds.link.environment_id,
                    label: creds.link.label,
                    urls: creds.link.urls,
                    reason: PeerLinkPairingUnsupportedReason::Unreachable,
                }),
            };
        }
    };
    let client = match PeerPairingClient::new() {
        Ok(c) => c,
        Err(err) => return map_peer_link_error(&err),
    };
    match client
        .request_pairing(&creds.link, &caps, &peer_url, &creds.token, pairing_req)
        .await
    {
        Ok(result) => RpcExit::Success {
            value: json!(result),
        },
        Err(err) => map_peer_link_error(&err),
    }
}

pub(crate) fn map_thread_read_err(err: &PeerLinkThreadReadError) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "PeerLinkThreadReadError",
        "reason": err.reason,
        "label": err.label,
        "message": err.message,
    }))
}

fn map_peer_link_error(err: &RemotePeerLinkError) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "PeerLinkError",
        "reason": err.reason,
        "message": err.message,
        "answeredBy": err.answered_as,
    }))
}

fn peer_link_err(reason: PeerLinkErrorReason, message: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "PeerLinkError",
        "reason": reason,
        "message": message,
    }))
}
