//! Request/response types, errors, and time helpers for [`super::LivePeerClient`] and
//! [`super::LivePeerMcpSession`].

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use t3_contracts::brand::ACTIVE_BRAND;
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_contracts::orchestration::McpClientAccess;
use t3_persistence::ownership::format_epoch_millis_utc;
use t3_protocol::mcp::ModeCeiling;
use thiserror::Error;

use crate::peer::links::{PeerLinkError, PeerLinkErrorReason};

/// Inputs required by [`super::LivePeerClient::link_peer_over_http`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePeerLinkInput {
    /// Primary HTTP/HTTPS URL of the remote peer environment.
    pub url: String,
    /// Optional fallback URLs validated and saved with the link.
    pub alternate_urls: Vec<String>,
    /// One-time pairing token or code exchanged at `/api/auth/pair`.
    pub pairing_code: String,
    /// Granted remote MCP access tier.
    pub access: McpClientAccess,
    /// Label describing this caller shown in the peer's connection list.
    pub client_label: String,
    /// Current environment ID, rejected with `SelfLink` when matching the target.
    pub self_environment_id: Option<EnvironmentId>,
    /// Expected target environment ID when refreshing an existing machine pick.
    pub expected_environment_id: Option<EnvironmentId>,
    /// Enforces `mcpModeLimitHeader` + `linkFence` capability checks (`PeerLinks.ts:219`).
    pub require_link_limits: bool,
    /// Optional UTC ISO-8601 link timestamp override (defaults to current UTC clock).
    pub linked_at: Option<String>,
}

impl LivePeerLinkInput {
    /// Constructs a [`LivePeerLinkInput`] with `require_link_limits = true`.
    pub fn new(
        url: impl Into<String>,
        pairing_code: impl Into<String>,
        access: McpClientAccess,
        client_label: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            alternate_urls: Vec::new(),
            pairing_code: pairing_code.into(),
            access,
            client_label: client_label.into(),
            self_environment_id: None,
            expected_environment_id: None,
            require_link_limits: true,
            linked_at: None,
        }
    }
}

/// Session options configuring [`super::LivePeerMcpSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePeerMcpOptions {
    /// Client name reported during MCP `initialize`.
    pub client_name: String,
    /// Client version reported during MCP `initialize`.
    pub client_version: String,
    /// Optional monotonic narrowing ceiling sent as `T3-Mode-Limit`.
    pub mode_limit: Option<ModeCeiling>,
    /// Optional caller `(environment_id, thread_id, title)` sent as `T3-Sender-Thread`.
    pub sender_thread: Option<(EnvironmentId, ThreadId, String)>,
    /// Per-request HTTP timeout (handshake and short RPCs; waits add extra headroom).
    pub timeout: Duration,
}

impl Default for LivePeerMcpOptions {
    fn default() -> Self {
        Self {
            client_name: format!("{}-peer", ACTIVE_BRAND.url_scheme),
            client_version: "1".to_owned(),
            mode_limit: None,
            sender_thread: None,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Errors returned by [`super::LivePeerMcpSession`].
#[derive(Debug, Error)]
pub enum PeerMcpError {
    /// Underlying peer-link resolution or origin validation error.
    #[error(transparent)]
    Link(#[from] PeerLinkError),
    /// Peer `/mcp` answered with an unexpected HTTP error status.
    #[error("peer /mcp responded with HTTP {status}: {detail}")]
    HttpStatus {
        /// Returned HTTP status code.
        status: u16,
        /// Short diagnostic detail.
        detail: String,
    },
    /// Transport, connection, or timeout failure talking to peer `/mcp`.
    #[error("peer /mcp transport error: {detail}")]
    Transport {
        /// Underlying HTTP transport message.
        detail: String,
    },
    /// Server omitted or rejected required MCP session handshake metadata.
    #[error("invalid peer /mcp session: {detail}")]
    InvalidSession {
        /// Session handshake error summary.
        detail: String,
    },
    /// Malformed JSON or SSE payload returned by peer `/mcp` (`BUG-06`).
    #[error("failed to parse peer /mcp response: {detail}")]
    Parse {
        /// Parse failure detail.
        detail: String,
    },
    /// JSON-RPC level error object returned from `/mcp`.
    #[error("peer /mcp JSON-RPC error ({code}): {message}")]
    JsonRpc {
        /// JSON-RPC numeric error code.
        code: i64,
        /// JSON-RPC error message.
        message: String,
    },
    /// Tool returned `isError: true` with a refusal code and message.
    #[error("peer tool refused ({code}): {message}")]
    ToolRefused {
        /// Parsed refusal code (e.g. `"thread_not_found"`, `"capability_denied"`).
        code: String,
        /// Refusal explanation.
        message: String,
    },
}

#[derive(Debug, Deserialize)]
pub(super) struct PairTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default, rename = "accessToken")]
    access_token_camel: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default, rename = "expiresAt")]
    expires_at_camel: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default, rename = "expiresIn")]
    expires_in_camel: Option<i64>,
}

pub(super) struct NormalizedPairToken {
    pub(super) access_token: String,
    pub(super) expires_at: Option<String>,
    pub(super) expires_in: Option<i64>,
}

impl PairTokenResponse {
    pub(super) fn into_normalized(self) -> Option<NormalizedPairToken> {
        let access_token = non_empty_string(self.access_token)
            .or_else(|| non_empty_string(self.access_token_camel))?;
        let expires_at =
            non_empty_string(self.expires_at).or_else(|| non_empty_string(self.expires_at_camel));
        let expires_in = self.expires_in.or(self.expires_in_camel);
        Some(NormalizedPairToken {
            access_token,
            expires_at,
            expires_in,
        })
    }
}

fn non_empty_string(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

pub(super) fn now_epoch_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}

pub(super) fn current_utc_iso() -> String {
    format_epoch_millis_utc(now_epoch_ms())
}

pub(super) fn expires_utc_iso(expires_in_secs: Option<i64>) -> String {
    let secs = expires_in_secs.unwrap_or(3_600).max(1);
    let delta_ms = secs.saturating_mul(1_000);
    format_epoch_millis_utc(now_epoch_ms().saturating_add(delta_ms))
}

pub(super) fn map_link_storage_err(reason: PeerLinkErrorReason, message: String) -> PeerLinkError {
    PeerLinkError::new(reason, message)
}
