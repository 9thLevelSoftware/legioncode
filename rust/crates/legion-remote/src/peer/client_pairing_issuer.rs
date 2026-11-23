//! Linked-side issuer policy validation for transitive client and peer-link pairing (`#40036`).

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::{PeerLinkClientPairingRequest, PeerMcpClientAccess};
use legion_contracts::policy::RuntimeMode;
use serde::{Deserialize, Serialize};

/// Issued credential lifetime (`3` minutes) matching `CLIENT_PAIRING_TTL` in `PeerClientPairing.ts`.
pub const CLIENT_PAIRING_TTL_MS: u64 = 180_000;
/// Maximum caller client-label characters retained on issued pairing labels (`PeerClientPairing.ts`).
pub const ISSUER_CLIENT_LABEL_MAX_LENGTH: usize = 80;

/// Machine-readable refusal reason returned by [`evaluate_pairing_issue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerClientPairingRefusalReason {
    /// Linked environments are disabled in settings.
    Disabled,
    /// Caller does not hold an unrestricted full-access link or managed policy caps mode below full access.
    NotAFullAccessLink,
    /// Client holds no scopes eligible for standard pairing.
    NoScopes,
    /// Enterprise/managed policy rejected the pairing request.
    Policy,
}

/// Error returned when an intermediate peer refuses to mint a transitive pairing credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerClientPairingRefused {
    /// Refusal category.
    pub reason: PeerClientPairingRefusalReason,
    /// Human-readable message.
    pub message: String,
}

/// Authenticated caller context evaluated on the linked target environment (`PeerClientPairingIssuer.issue`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingIssuerCaller {
    /// Whether the caller authenticated over a linked-environment bearer token.
    pub is_peer_link: bool,
    /// Effective granted access tier of the peer link.
    pub access: McpClientAccess,
    /// Caller's link label on this machine (e.g. `"LegionCode · Laptop"`).
    pub label: String,
}

/// Validated issuance plan produced by [`evaluate_pairing_issue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovedPairingIssue {
    /// Mint a peer-link pairing code (`purpose: "peer-link"`).
    PeerLink {
        /// Access tier granted to the new transitive peer link.
        access: PeerMcpClientAccess,
        /// Formatted client label (`"Link · via <via>"`).
        label: String,
    },
    /// Mint a standard OAuth/client pairing link (`ttl: 3m`).
    Client {
        /// Standard scopes permitted for the delegated client.
        scopes: Vec<String>,
        /// Formatted label (`"<bounded client label> · via <via>"`).
        label: String,
        /// Expiry lifetime in milliseconds (`180_000`).
        ttl_ms: u64,
    },
}

/// Validates transitive pairing issuance constraints matching `PeerClientPairingIssuer.issue` (`#40036`).
pub fn evaluate_pairing_issue(
    linked_environments_enabled: bool,
    max_runtime_mode: Option<RuntimeMode>,
    caller: &PairingIssuerCaller,
    request: &PeerLinkClientPairingRequest,
    standard_client_scopes: &[&str],
) -> Result<ApprovedPairingIssue, PeerClientPairingRefused> {
    evaluate_pairing_issue_for_brand(
        linked_environments_enabled,
        max_runtime_mode,
        caller,
        request,
        standard_client_scopes,
        &ACTIVE_BRAND,
    )
}

/// Brand-parameterized variant of [`evaluate_pairing_issue`].
pub fn evaluate_pairing_issue_for_brand(
    linked_environments_enabled: bool,
    max_runtime_mode: Option<RuntimeMode>,
    caller: &PairingIssuerCaller,
    request: &PeerLinkClientPairingRequest,
    standard_client_scopes: &[&str],
    brand: &BrandConfig,
) -> Result<ApprovedPairingIssue, PeerClientPairingRefused> {
    if !linked_environments_enabled {
        return Err(PeerClientPairingRefused {
            reason: PeerClientPairingRefusalReason::Disabled,
            message: format!(
                "Linked environments is off. Turn it on in Settings -> Labs, or set \"linkedEnvironmentsEnabled\": true in your {} settings.",
                brand.product_name
            ),
        });
    }
    if !caller.is_peer_link
        || caller.access != McpClientAccess::FullAccess
        || max_runtime_mode.is_some_and(|mode| mode != RuntimeMode::FullAccess)
    {
        return Err(PeerClientPairingRefused {
            reason: PeerClientPairingRefusalReason::NotAFullAccessLink,
            message: "Only a full-access link may pair a client here.".to_owned(),
        });
    }

    let prefix = format!("{} · ", brand.product_name);
    let via = caller.label.strip_prefix(&prefix).unwrap_or(&caller.label);

    if let Some(link) = request.link {
        return Ok(ApprovedPairingIssue::PeerLink {
            access: link.access,
            label: format!("Link · via {via}"),
        });
    }

    let scopes: Vec<String> = standard_client_scopes
        .iter()
        .filter(|scope| request.scopes.iter().any(|candidate| candidate == *scope))
        .map(|&scope| scope.to_owned())
        .collect();
    if scopes.is_empty() {
        return Err(PeerClientPairingRefused {
            reason: PeerClientPairingRefusalReason::NoScopes,
            message: "The client holds no access a pairing could carry.".to_owned(),
        });
    }

    let bounded_client_label: String = request
        .client_label
        .chars()
        .take(ISSUER_CLIENT_LABEL_MAX_LENGTH)
        .collect();
    Ok(ApprovedPairingIssue::Client {
        scopes,
        label: format!("{bounded_client_label} · via {via}"),
        ttl_ms: CLIENT_PAIRING_TTL_MS,
    })
}
