//! Peer link data models, error types, and SQLite row conversion helpers.

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::{
    PeerLinkError as ContractPeerLinkError, PeerLinkErrorReason as ContractPeerLinkErrorReason,
    PeerLinkFailureKind, peer_link_failure_kind as contract_failure_kind,
};
use legion_persistence::auth_records::PeerLinkRecord;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Reason classification for [`PeerLinkError`] (`packages/contracts/src/peerLink.ts:61-71`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkErrorReason {
    /// Peer address is unreachable or uses disallowed plain HTTP.
    Unreachable,
    /// Host answered with a non-`LegionCode` payload.
    NotALegionCodeEnvironment,
    /// Peer lacks fencing/mode-limit capabilities.
    Incompatible,
    /// Target resolved to this same environment.
    SelfLink,
    /// Pairing code or OAuth exchange was refused.
    PairingRejected,
    /// Stored peer link token has expired.
    Expired,
    /// No stored link or lost credential for `environment_id`.
    UnknownLink,
    /// SQLite or secret store persistence error.
    Storage,
    /// Linked environments feature is disabled on this machine.
    Disabled,
}

/// Structured peer-link error (`PeerLinkError` in `peerLink.ts:60-74`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct PeerLinkError {
    /// Machine-readable reason tag.
    pub reason: PeerLinkErrorReason,
    /// Actionable user-facing message.
    pub message: String,
    /// Display label/ID of the environment that unexpectedly answered instead (`#39426`).
    pub answered_as: Option<String>,
}

impl PeerLinkError {
    /// Constructs a [`PeerLinkError`].
    pub fn new(reason: PeerLinkErrorReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            answered_as: None,
        }
    }

    /// Attaches the `answered_as` identity returned when another peer answered at this address.
    #[must_use]
    pub fn with_answered_as(mut self, answered_as: impl Into<String>) -> Self {
        self.answered_as = Some(answered_as.into());
        self
    }

    /// Maps into [`ContractPeerLinkError`] and classifies via `legion_contracts::peer_links`.
    #[must_use]
    pub fn failure_kind(&self) -> PeerLinkFailureKind {
        let reason = match self.reason {
            PeerLinkErrorReason::Unreachable => ContractPeerLinkErrorReason::Unreachable,
            PeerLinkErrorReason::NotALegionCodeEnvironment => {
                ContractPeerLinkErrorReason::NotALegionCodeEnvironment
            }
            PeerLinkErrorReason::Incompatible => ContractPeerLinkErrorReason::Incompatible,
            PeerLinkErrorReason::SelfLink => ContractPeerLinkErrorReason::SelfLink,
            PeerLinkErrorReason::PairingRejected => ContractPeerLinkErrorReason::PairingRejected,
            PeerLinkErrorReason::Expired => ContractPeerLinkErrorReason::Expired,
            PeerLinkErrorReason::UnknownLink => ContractPeerLinkErrorReason::UnknownLink,
            PeerLinkErrorReason::Storage => ContractPeerLinkErrorReason::Storage,
            PeerLinkErrorReason::Disabled => ContractPeerLinkErrorReason::Disabled,
        };
        let contract = ContractPeerLinkError {
            reason,
            message: self.message.clone(),
            answered_as: self.answered_as.clone(),
        };
        contract_failure_kind(&contract)
    }
}

/// Stored link metadata matching `PeerLink` (`packages/contracts/src/peerLink.ts:12-22`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLink {
    /// Linked remote environment ID.
    pub environment_id: EnvironmentId,
    /// Display label of the peer environment.
    pub label: String,
    /// Normalized peer origin URLs, tried in order.
    pub urls: Vec<String>,
    /// Granted MCP client access tier.
    pub access: McpClientAccess,
    /// Link creation timestamp (UTC ISO-8601).
    pub linked_at: String,
    /// Link expiration timestamp (UTC ISO-8601).
    pub expires_at: String,
    /// Last successful contact timestamp, or `null`.
    pub last_reached_at: Option<String>,
    /// Last call error message, or `null`.
    pub last_error: Option<String>,
}

/// Input to [`super::PeerLinksService::insert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertPeerLinkInput {
    /// Linked peer environment ID.
    pub environment_id: EnvironmentId,
    /// Display label.
    pub label: String,
    /// Candidate base URLs (validated and normalized to origins).
    pub urls: Vec<String>,
    /// Granted access tier.
    pub access: McpClientAccess,
    /// OAuth bearer access token stored as a `0o600` `.bin` secret.
    pub access_token: String,
    /// Link creation timestamp (UTC ISO-8601).
    pub linked_at: String,
    /// Link expiration timestamp (UTC ISO-8601).
    pub expires_at: String,
}

/// Resolved peer link along with its bearer token read from [`crate::secrets::FileSecretStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPeerCredentials {
    /// Stored link record.
    pub link: PeerLink,
    /// Bearer access token.
    pub token: String,
}

pub(super) fn record_to_link(row: PeerLinkRecord) -> Result<PeerLink, PeerLinkError> {
    let environment_id = EnvironmentId::new(row.environment_id).map_err(|err| {
        PeerLinkError::new(
            PeerLinkErrorReason::Storage,
            format!("Invalid peer environmentId: {err}"),
        )
    })?;
    let access: McpClientAccess = serde_json::from_value(serde_json::Value::String(row.access))
        .map_err(|err| {
            PeerLinkError::new(
                PeerLinkErrorReason::Storage,
                format!("Invalid peer access tier: {err}"),
            )
        })?;
    // PeerLinks.ts:288: unfenced legacy links expire immediately so they must re-pair.
    let expires_at = if row.fenced {
        row.expires_at
    } else {
        row.linked_at.clone()
    };
    Ok(PeerLink {
        environment_id,
        label: row.label,
        urls: row.urls,
        access,
        linked_at: row.linked_at,
        expires_at,
        last_reached_at: row.last_reached_at,
        last_error: row.last_error,
    })
}

pub(super) fn serialize_access(access: McpClientAccess) -> Result<String, PeerLinkError> {
    match serde_json::to_value(access) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        _ => Err(PeerLinkError::new(
            PeerLinkErrorReason::Storage,
            "Failed to encode peer access tier.",
        )),
    }
}

pub(super) fn storage_err<E>(_err: E) -> PeerLinkError {
    PeerLinkError::new(
        PeerLinkErrorReason::Storage,
        "Linked environments could not be read or saved here. The server log has details.",
    )
}
