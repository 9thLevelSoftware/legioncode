//! Peer-link CLI list, compact JSON, and unlink formatters matching `apps/server/src/cli/environment.ts`.

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use serde::{Deserialize, Serialize};

use super::types::PeerUnlinkOutcome;
use super::{PeerLinkError, PeerLinkErrorReason};

/// Reachability classification for a linked peer environment (`packages/contracts/src/peerLink.ts:26`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerLinkStatus {
    /// At least one peer URL answered with the matching environment descriptor.
    Reachable,
    /// No peer URL answered before timeout or probe failed.
    Unreachable,
    /// Link expiration timestamp has elapsed (or legacy unfenced link).
    Expired,
}

/// Listed peer link summary including live reachability (`PeerLinkSummary` in `peerLink.ts:29-33`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerLinkSummary {
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
    /// Current status (`"reachable"`, `"unreachable"`, or `"expired"`).
    pub status: PeerLinkStatus,
}

/// Formats peer links matching `cli/environment.ts:75-84` byte-for-byte (with trailing `\n`).
#[must_use]
pub fn format_peer_links(links: &[PeerLinkSummary]) -> String {
    if links.is_empty() {
        return "No linked environments.\n".to_owned();
    }
    let blocks: Vec<String> = links
        .iter()
        .map(|link| {
            let status = status_str(link.status);
            let access = access_str(link.access);
            let urls = link.urls.join(", ");
            let err_line = match &link.last_error {
                None => String::new(),
                Some(err) => format!("\n  last error: {err}"),
            };
            format!(
                "{} ({})\n  {status}, access {access}, expires {}\n  {urls}{err_line}",
                link.label, link.environment_id, link.expires_at
            )
        })
        .collect();
    let mut out = blocks.join("\n\n");
    out.push('\n');
    out
}

/// Formats peer links as compact JSON with trailing `\n` matching `cli/environment.ts:71-73,134`.
///
/// # Errors
///
/// Returns [`PeerLinkError`] with [`PeerLinkErrorReason::Storage`] if serialization fails.
pub fn format_peer_links_json(links: &[PeerLinkSummary]) -> Result<String, PeerLinkError> {
    let mut out = serde_json::to_string(links).map_err(|err| {
        PeerLinkError::new(
            PeerLinkErrorReason::Storage,
            format!("Failed to serialize peer links JSON: {err}"),
        )
    })?;
    out.push('\n');
    Ok(out)
}

/// Formats the `legioncode environment unlink` CLI confirmation matching `cli/environment.ts:155-158`.
#[must_use]
pub fn format_peer_unlink_result(environment_id: &str, removed: bool) -> String {
    if removed {
        format!(
            "Forgot {environment_id}. Revoke it in that environment's Sharing & access to end its session there too.\n"
        )
    } else {
        format!("No linked environment {environment_id}.\n")
    }
}

/// Formats the `#41005` `legioncode environment unlink` outcome reflecting whether the peer ended
/// its remote session (`ended_there`).
#[must_use]
pub fn format_peer_unlink_outcome(environment_id: &str, outcome: PeerUnlinkOutcome) -> String {
    if !outcome.removed {
        format!("No linked environment {environment_id}.\n")
    } else if outcome.ended_there {
        format!("Forgot {environment_id} and ended its session there.\n")
    } else {
        format!(
            "Forgot {environment_id}. It did not answer, so revoke it in that environment's Sharing & access to end its session there too.\n"
        )
    }
}

const fn status_str(status: PeerLinkStatus) -> &'static str {
    match status {
        PeerLinkStatus::Reachable => "reachable",
        PeerLinkStatus::Unreachable => "unreachable",
        PeerLinkStatus::Expired => "expired",
    }
}

const fn access_str(access: McpClientAccess) -> &'static str {
    match access {
        McpClientAccess::ReadOnly => "read-only",
        McpClientAccess::ApprovalRequired => "approval-required",
        McpClientAccess::AutoAcceptEdits => "auto-accept-edits",
        McpClientAccess::Auto => "auto",
        McpClientAccess::FullAccess => "full-access",
    }
}
