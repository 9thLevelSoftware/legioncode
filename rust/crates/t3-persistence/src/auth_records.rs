//! Persistence queries and CLI formatters for auth pairing links, sessions, and peer links.
//!
//! Mirrors `apps/server/src/persistence/AuthPairingLinks.ts`,
//! `apps/server/src/persistence/AuthSessions.ts`, `apps/server/src/peer/PeerLinks.ts`, and
//! `apps/server/src/cliAuthFormat.ts:75-198`.

mod format;
mod queries;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) use format::format_client_metadata;
pub use format::{format_pairing_credential_list, format_session_list};
pub use queries::{
    insert_peer_link, list_active_pairing_links, list_active_sessions, list_peer_links,
    unlink_peer_link,
};

/// Errors produced when querying, mutating, or formatting auth and peer-link records.
#[derive(Debug, Error)]
pub enum AuthRecordsError {
    /// SQLite query or statement failure.
    #[error("auth records SQLite error during '{operation}': {source}")]
    Sqlite {
        /// Operation description.
        operation: &'static str,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// JSON serialization or deserialization failure (`scopes`, `urls_json`, or CLI JSON output).
    #[error("auth records JSON error during '{operation}': {source}")]
    Json {
        /// Operation description.
        operation: &'static str,
        /// Underlying `serde_json` error.
        #[source]
        source: serde_json::Error,
    },
}

/// Client connection metadata attached to an active `auth_sessions` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientMetadataRow {
    /// Human-readable client label, if provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Client IP address, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    /// Raw User-Agent string, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Device classification (`desktop`, `mobile`, `tablet`, `bot`, or `unknown`).
    pub device_type: String,
    /// Client operating system, if detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Client browser or runtime, if detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    /// Client UI surface (`web`, `desktop`, `mobile`, etc.), if reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    /// Client application version, if reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

impl Default for ClientMetadataRow {
    fn default() -> Self {
        Self {
            label: None,
            ip_address: None,
            user_agent: None,
            device_type: "unknown".to_owned(),
            os: None,
            browser: None,
            surface: None,
            app_version: None,
        }
    }
}

/// Row stored in `auth_pairing_links`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingLinkRow {
    /// Unique pairing credential identifier.
    pub id: String,
    /// One-time credential token.
    pub credential: String,
    /// Issuance method (`desktop-bootstrap`, `one-time-token`, etc.).
    pub method: String,
    /// Granted environment authorization scopes.
    pub scopes: Vec<String>,
    /// Subject bound to the pairing link.
    pub subject: String,
    /// Optional human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Creation timestamp (UTC ISO-8601).
    pub created_at: String,
    /// Expiration timestamp (UTC ISO-8601).
    pub expires_at: String,
    /// Consumption timestamp, or `None` if unspent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<String>,
    /// Revocation timestamp, or `None` if active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    /// Optional DPoP/Connect JWK SHA-256 thumbprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_key_thumbprint: Option<String>,
}

/// Row stored in `auth_sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionRow {
    /// Unique session identifier.
    pub session_id: String,
    /// Authenticated subject.
    pub subject: String,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Session issuance method.
    pub method: String,
    /// Client device metadata.
    pub client: ClientMetadataRow,
    /// Issuance timestamp (UTC ISO-8601).
    pub issued_at: String,
    /// Expiration timestamp (UTC ISO-8601).
    pub expires_at: String,
    /// Last active connection timestamp (UTC ISO-8601), or `None`.
    pub last_connected_at: Option<String>,
    /// Revocation timestamp, or `None` if active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    /// Whether this session is currently connected (populated by caller; defaults to `false`).
    #[serde(default)]
    pub connected: bool,
}

/// Row stored in `peer_environment_links`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkRecord {
    /// Linked remote environment ID.
    pub environment_id: String,
    /// Secret-store key holding this link's session bearer token.
    pub secret_name: String,
    /// Display label for the peer environment.
    pub label: String,
    /// Candidate base URLs serialized as `urls_json`.
    pub urls: Vec<String>,
    /// Granted access level (`read-only`, `full`, etc.).
    pub access: String,
    /// Link establishment timestamp (UTC ISO-8601).
    pub linked_at: String,
    /// Link expiration timestamp (UTC ISO-8601).
    pub expires_at: String,
    /// Timestamp of last successful contact, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reached_at: Option<String>,
    /// Last error recorded while reaching the peer, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Whether the link uses a fenced peer session (`fenced = 1`).
    pub fenced: bool,
}

pub(super) fn sqlite_err(operation: &'static str, source: rusqlite::Error) -> AuthRecordsError {
    AuthRecordsError::Sqlite { operation, source }
}

pub(super) fn json_err(operation: &'static str, source: serde_json::Error) -> AuthRecordsError {
    AuthRecordsError::Json { operation, source }
}
