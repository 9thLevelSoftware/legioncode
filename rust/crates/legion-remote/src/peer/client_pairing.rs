//! Batch peer environment linking (`#39426`) and transitive pairing through an
//! already-linked peer environment (`#40036`).

use std::time::Duration;

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use legion_contracts::environment::EnvironmentCapabilities;
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::{
    PeerLinkClientPairingRequest, PeerLinkClientPairingResult, PeerLinkLinkPairingInput,
    PeerLinkPairingLinkAccess, PeerLinkPairingUnsupportedReason, PeerMcpClientAccess,
};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

pub use crate::peer::batch_linking::{
    BatchLinkOutcome, BatchLinkSummary, BatchLinkTarget, classify_batch_failure,
    link_unless_linked_local, run_batch_linking, summarize_batch_outcomes,
};
pub use crate::peer::client_pairing_issuer::{
    ApprovedPairingIssue, CLIENT_PAIRING_TTL_MS, ISSUER_CLIENT_LABEL_MAX_LENGTH,
    PairingIssuerCaller, PeerClientPairingRefusalReason, PeerClientPairingRefused,
    evaluate_pairing_issue, evaluate_pairing_issue_for_brand,
};
use crate::peer::links::{PeerLink, PeerLinkError, PeerLinkErrorReason, validate_peer_origin};

/// Default peer client pairing HTTP endpoint relative path.
pub const PEER_LINK_CLIENT_PAIRING_ROUTE: &str = "/api/links/client-pairing";
/// Default timeout for transitive pairing HTTP calls.
const DEFAULT_PAIRING_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum number of characters retained from a transitive caller's client label.
pub const CLIENT_LABEL_MAX_LENGTH: usize = 128;

/// Returns the client-label prefix prepended to linked peer client labels using [`ACTIVE_BRAND`].
#[must_use]
pub fn link_client_name_prefix() -> String {
    link_client_name_prefix_for_brand(&ACTIVE_BRAND)
}

/// Parameterized variant of [`link_client_name_prefix`] for an explicit [`BrandConfig`].
#[must_use]
pub fn link_client_name_prefix_for_brand(brand: &BrandConfig) -> String {
    format!("{} · ", brand.product_name)
}

/// Strips the brand link prefix from `caller_label` and formats either a client or transitive link label (`#40036`).
#[must_use]
pub fn format_transitive_pairing_label(
    caller_label: &str,
    client_label: Option<&str>,
    is_link_pairing: bool,
) -> String {
    format_transitive_pairing_label_for_brand(
        caller_label,
        client_label,
        is_link_pairing,
        &ACTIVE_BRAND,
    )
}

/// Brand-parameterized variant of [`format_transitive_pairing_label`] (`#40036`).
#[must_use]
pub fn format_transitive_pairing_label_for_brand(
    caller_label: &str,
    client_label: Option<&str>,
    is_link_pairing: bool,
    brand: &BrandConfig,
) -> String {
    let prefix = link_client_name_prefix_for_brand(brand);
    let via = caller_label.strip_prefix(&prefix).unwrap_or(caller_label);
    if is_link_pairing {
        format!("Link · via {via}")
    } else {
        let bounded: String = client_label
            .unwrap_or("Client")
            .chars()
            .take(CLIENT_LABEL_MAX_LENGTH)
            .collect();
        format!("{bounded} · via {via}")
    }
}

/// Intersects caller-requested OAuth/MCP scopes with the permitted standard client scopes.
#[must_use]
pub fn filter_standard_client_scopes(
    requested: &[String],
    standard_scopes: &[&str],
) -> Vec<String> {
    standard_scopes
        .iter()
        .filter(|scope| requested.iter().any(|candidate| candidate == *scope))
        .map(|&scope| scope.to_owned())
        .collect()
}

/// Runs pre-flight checks before asking a linked peer to mint a client or transitive link credential (`#40036`).
#[must_use]
pub fn check_transitive_pairing_capability(
    link: &PeerLink,
    capabilities: &EnvironmentCapabilities,
    is_link_pairing: bool,
) -> Option<PeerLinkClientPairingResult> {
    let unsupported = |reason| {
        Some(PeerLinkClientPairingResult::Unsupported {
            environment_id: link.environment_id.clone(),
            label: link.label.clone(),
            urls: link.urls.clone(),
            reason,
        })
    };
    if link.access != McpClientAccess::FullAccess {
        return unsupported(PeerLinkPairingUnsupportedReason::LinkAccess);
    }
    if capabilities.peer_link_pairing_codes != Some(true) {
        return unsupported(PeerLinkPairingUnsupportedReason::Version);
    }
    // Older peers without transitive link pairing interpret a client-pairing POST as pairing a client instead.
    if is_link_pairing && capabilities.transitive_peer_linking != Some(true) {
        return unsupported(PeerLinkPairingUnsupportedReason::Version);
    }
    None
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuedPairingPayload {
    credential: String,
    expires_at: String,
}

/// Non-redirecting HTTP client requesting client or transitive peer-link credentials (`#40036`).
#[derive(Debug, Clone)]
pub struct PeerPairingClient {
    http: Client,
    timeout: Duration,
}

impl PeerPairingClient {
    /// Builds a [`PeerPairingClient`] with redirects disabled so bearer tokens stay pinned.
    pub fn new() -> Result<Self, PeerLinkError> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| {
                PeerLinkError::new(
                    PeerLinkErrorReason::Unreachable,
                    format!("Failed to initialize pairing HTTP client: {err}"),
                )
            })?;
        Ok(Self {
            http,
            timeout: DEFAULT_PAIRING_TIMEOUT,
        })
    }

    /// Requests a transitive peer-link pairing credential through an intermediate full-access link (`#40036`).
    pub async fn request_link_pairing(
        &self,
        link: &PeerLink,
        capabilities: &EnvironmentCapabilities,
        peer_url: &str,
        bearer_token: &str,
        client_label: &str,
        input: &PeerLinkLinkPairingInput,
    ) -> Result<PeerLinkClientPairingResult, PeerLinkError> {
        self.request_pairing(
            link,
            capabilities,
            peer_url,
            bearer_token,
            PeerLinkClientPairingRequest {
                scopes: Vec::new(),
                client_label: client_label.to_owned(),
                link: Some(PeerLinkPairingLinkAccess {
                    access: input.access,
                }),
            },
        )
        .await
    }

    /// Requests a client pairing credential (`peerLinks.pairClient`) or link pairing credential (`peerLinks.pairLink`).
    pub async fn request_pairing(
        &self,
        link: &PeerLink,
        capabilities: &EnvironmentCapabilities,
        peer_url: &str,
        bearer_token: &str,
        payload: PeerLinkClientPairingRequest,
    ) -> Result<PeerLinkClientPairingResult, PeerLinkError> {
        let is_link_pairing = payload.link.is_some();
        if let Some(refused) =
            check_transitive_pairing_capability(link, capabilities, is_link_pairing)
        {
            return Ok(refused);
        }
        let origin = validate_peer_origin(peer_url)?;
        let unsupported = |reason| PeerLinkClientPairingResult::Unsupported {
            environment_id: link.environment_id.clone(),
            label: link.label.clone(),
            urls: link.urls.clone(),
            reason,
        };
        let Ok(resp) = self
            .http
            .post(format!("{origin}{PEER_LINK_CLIENT_PAIRING_ROUTE}"))
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, format!("Bearer {bearer_token}"))
            .json(&payload)
            .timeout(self.timeout)
            .send()
            .await
        else {
            return Ok(unsupported(PeerLinkPairingUnsupportedReason::Unreachable));
        };

        if !resp.status().is_success() {
            let reason = match resp.status() {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    PeerLinkPairingUnsupportedReason::Refused
                }
                _ => PeerLinkPairingUnsupportedReason::Unreachable,
            };
            return Ok(unsupported(reason));
        }

        // Do not include response body in errors because unexpected shapes may echo credentials.
        let Ok(issued) = resp.json::<IssuedPairingPayload>().await else {
            return Ok(unsupported(PeerLinkPairingUnsupportedReason::Refused));
        };
        Ok(PeerLinkClientPairingResult::Credential {
            environment_id: link.environment_id.clone(),
            label: link.label.clone(),
            urls: link.urls.clone(),
            credential: issued.credential,
            expires_at: issued.expires_at,
        })
    }
}

/// Maps transitive [`PeerMcpClientAccess`] cleanly to local [`McpClientAccess`].
#[must_use]
pub const fn to_mcp_client_access(access: PeerMcpClientAccess) -> McpClientAccess {
    match access {
        PeerMcpClientAccess::ReadOnly => McpClientAccess::ReadOnly,
        PeerMcpClientAccess::ApprovalRequired => McpClientAccess::ApprovalRequired,
        PeerMcpClientAccess::Auto => McpClientAccess::Auto,
        PeerMcpClientAccess::FullAccess => McpClientAccess::FullAccess,
    }
}
