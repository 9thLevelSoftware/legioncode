//! Peer-environment linking, linked-thread reading, batch failure classification,
//! and transitive pairing contracts (`peerLink.ts`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{EnvironmentId, ThreadId, TurnItemId};

/// HTTP route prefix serving a linked environment's threads to a peer link session.
pub const PEER_LINK_THREAD_ROUTE_PREFIX: &str = "/api/links/thread";
/// Streaming thread snapshot/delta route for peer links.
pub const PEER_LINK_THREAD_STREAM_ROUTE: &str = "/api/links/thread/stream";
/// History pagination route for peer links.
pub const PEER_LINK_THREAD_HISTORY_ROUTE: &str = "/api/links/thread/history";
/// Turn-item detail route for peer links.
pub const PEER_LINK_THREAD_ITEM_ROUTE: &str = "/api/links/thread/item";

/// HTTP response header carrying the renewed ISO-8601 expiry timestamp of an active peer link.
pub const LINK_EXPIRES_AT_HEADER: &str = "t3-link-expires-at";

/// Minimal link summary (`PeerLinkName`) readable with thread-read scope (`peerLinks.names`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkName {
    /// Linked environment identifier.
    pub environment_id: EnvironmentId,
    /// Human-readable display label of the linked environment.
    pub label: String,
}

/// Access level granted to an MCP client or peer environment link (`AuthMcpClientAccess`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeerMcpClientAccess {
    /// Read-only inspection only.
    ReadOnly,
    /// Requires user approval before running tools/turns.
    ApprovalRequired,
    /// Automatic execution without interactive approval.
    Auto,
    /// Unbounded workspace/pairing privilege (`full-access`).
    FullAccess,
}

/// Input payload identifying a thread on a linked peer environment (`PeerLinkThreadInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkThreadInput {
    /// Linked environment hosting the thread.
    pub environment_id: EnvironmentId,
    /// Target thread identifier on the linked environment.
    pub thread_id: ThreadId,
}

/// Input payload for `peerLinks.threadHistoryPage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkThreadHistoryInput {
    /// Linked environment hosting the thread.
    pub environment_id: EnvironmentId,
    /// Target thread identifier on the linked environment.
    pub thread_id: ThreadId,
    /// Pagination cursor from the snapshot or previous page.
    pub cursor: String,
}

/// Input payload for `peerLinks.threadItem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkThreadItemInput {
    /// Linked environment hosting the thread.
    pub environment_id: EnvironmentId,
    /// Target thread identifier on the linked environment.
    pub thread_id: ThreadId,
    /// Target turn-item identifier.
    pub item_id: TurnItemId,
    /// Optional client cache-busting revision tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// Structured failure reason on [`PeerLinkThreadReadError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkThreadReadReason {
    /// Linked environments are disabled on this server.
    Disabled,
    /// No peer link exists for the requested environment ID.
    UnknownLink,
    /// Stored link credential has expired.
    Expired,
    /// Peer refused the link or read request.
    Rejected,
    /// Peer host could not be reached.
    Unreachable,
    /// Requested thread does not exist on the peer.
    ThreadNotFound,
    /// Peer runs an older version that does not support linked thread reading.
    Incompatible,
}

/// Error returned when reading a linked environment's thread fails (`PeerLinkThreadReadError`).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct PeerLinkThreadReadError {
    /// Structured reason code.
    pub reason: PeerLinkThreadReadReason,
    /// Link label known locally, if any.
    pub label: Option<String>,
    /// User-facing failure summary.
    pub message: String,
}

/// Batch failure classification returned by `peer_link_failure_kind` (#39426).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkFailureKind {
    /// Peer could not be reached or was not a T3 server.
    Unreachable,
    /// Pairing code was rejected by the peer.
    CodeRefused,
    /// A different environment answered at the configured address.
    WrongEnvironment,
    /// Any other link error reason.
    Other,
}

/// Reason tag for [`PeerLinkError`] (`peerLink.ts:60-74`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkErrorReason {
    /// Base URL is invalid or rejected by security rules.
    InvalidUrl,
    /// Server refuses linking to itself.
    SelfLink,
    /// Peer could not be contacted.
    Unreachable,
    /// Host responded but is not a T3 environment.
    NotAT3Environment,
    /// Pairing code was invalid, expired, or consumed.
    PairingRejected,
    /// Peer lacks required link capabilities.
    Incompatible,
    /// Target environment ID is not linked here.
    UnknownLink,
    /// Link has expired.
    Expired,
    /// Storage persistence error.
    Storage,
    /// Linked environments feature is disabled.
    Disabled,
}

/// Structured peer-link error payload (`PeerLinkError`).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct PeerLinkError {
    /// Structured reason code.
    pub reason: PeerLinkErrorReason,
    /// Human-readable description.
    pub message: String,
    /// Name/ID of the environment that unexpectedly answered instead of the chosen peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_as: Option<String>,
}

/// Classifies a [`PeerLinkError`] into a [`PeerLinkFailureKind`] for batch peer linking (`peerLink.ts`).
pub const fn peer_link_failure_kind(error: &PeerLinkError) -> PeerLinkFailureKind {
    if error.answered_as.is_some() {
        PeerLinkFailureKind::WrongEnvironment
    } else {
        match error.reason {
            PeerLinkErrorReason::Unreachable | PeerLinkErrorReason::NotAT3Environment => {
                PeerLinkFailureKind::Unreachable
            }
            PeerLinkErrorReason::PairingRejected => PeerLinkFailureKind::CodeRefused,
            _ => PeerLinkFailureKind::Other,
        }
    }
}

/// Access payload attached to a transitive link-pairing request (`PeerLinkClientPairingRequest.link`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkPairingLinkAccess {
    /// Access level the minted link pairing code grants.
    pub access: PeerMcpClientAccess,
}

/// Transitive link-pairing request sent via another environment (`peerLinks.pairLink`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkLinkPairingInput {
    /// Target environment reachable through the intermediate linked host.
    pub environment_id: EnvironmentId,
    /// Access level the minted pairing code grants.
    pub access: PeerMcpClientAccess,
}

/// Request sent to a linked peer to mint either a client pairing code or a link pairing code (`#40036`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkClientPairingRequest {
    /// Requested scopes when pairing a client directly.
    pub scopes: Vec<String>,
    /// Requesting client display label.
    pub client_label: String,
    /// Present when minting a transitive peer-link pairing code instead of a client code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<PeerLinkPairingLinkAccess>,
}

/// Reason a peer refused transitive client/link pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkPairingUnsupportedReason {
    /// Linked peer does not hold full access.
    LinkAccess,
    /// Linked peer is unreachable.
    Unreachable,
    /// Linked peer version predates pairing support.
    Version,
    /// Peer policy or capability refused pairing.
    Refused,
}

/// Response union returned by `peerLinks.pairClient` and `peerLinks.pairLink`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerLinkClientPairingResult {
    /// Successfully minted pairing credential.
    #[serde(rename_all = "camelCase")]
    Credential {
        environment_id: EnvironmentId,
        label: String,
        urls: Vec<String>,
        credential: String,
        expires_at: String,
    },
    /// Peer could not or would not mint a pairing credential.
    #[serde(rename_all = "camelCase")]
    Unsupported {
        environment_id: EnvironmentId,
        label: String,
        urls: Vec<String>,
        reason: PeerLinkPairingUnsupportedReason,
    },
}
