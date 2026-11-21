//! Peer link data models, error types, and SQLite row conversion helpers.

use serde::{Deserialize, Serialize};
use t3_contracts::ids::EnvironmentId;
use t3_contracts::orchestration::McpClientAccess;
use t3_persistence::auth_records::PeerLinkRecord;
use thiserror::Error;

/// Reason classification for [`PeerLinkError`] (`packages/contracts/src/peerLink.ts:61-71`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkErrorReason {
    /// Peer address is unreachable or uses disallowed plain HTTP.
    Unreachable,
    /// Host answered with a non-T3 payload.
    NotAT3Environment,
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
}

impl PeerLinkError {
    /// Constructs a [`PeerLinkError`].
    pub fn new(reason: PeerLinkErrorReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
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
