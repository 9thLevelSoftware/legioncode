//! Batch peer environment linking helpers (`#39426`).

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::PeerLinkFailureKind;
use rusqlite::Connection;

use crate::live_network::{LivePeerClient, LivePeerLinkInput};
use crate::peer::links::{
    InsertPeerLinkInput, PeerLink, PeerLinkError, PeerLinkErrorReason, PeerLinksService,
};
use crate::secrets::FileSecretStore;

/// Candidate environment included in a batch peer linking run (`#39426`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchLinkTarget {
    /// Target peer environment identifier.
    pub environment_id: EnvironmentId,
    /// Human-readable display label of the target environment.
    pub label: String,
    /// Primary HTTP/HTTPS endpoint URL for the target peer.
    pub url: String,
    /// Optional fallback origin URLs.
    pub alternate_urls: Vec<String>,
    /// One-time pairing token or code minted for linking.
    pub pairing_code: String,
    /// Granted remote MCP client access level.
    pub access: McpClientAccess,
}

/// Outcome for a single machine in a batch peer linking run (`#39426`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchLinkOutcome {
    /// Target environment is linked (or already held an unexpired link whose pairing code stayed unused).
    Linked {
        /// Stored or retained [`PeerLink`].
        link: PeerLink,
        /// `true` when an active link already existed and no new pairing exchange occurred.
        already_linked: bool,
    },
    /// Linking target failed with a classified failure category.
    Failed {
        /// Target environment identifier.
        environment_id: EnvironmentId,
        /// Target display label.
        label: String,
        /// Classified batch failure reason (`unreachable`, `code_refused`, `wrong_environment`, `other`).
        kind: PeerLinkFailureKind,
        /// Safe diagnostic summary.
        message: String,
    },
}

/// Aggregated counters summarizing a [`Vec<BatchLinkOutcome>`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchLinkSummary {
    /// Total targets evaluated.
    pub total: usize,
    /// Newly linked environments.
    pub newly_linked: usize,
    /// Environments kept on their existing unexpired link.
    pub already_linked: usize,
    /// Environments that failed to link.
    pub failed: usize,
}

/// Summarizes outcomes from a batch peer linking run (`#39426`).
#[must_use]
pub fn summarize_batch_outcomes(outcomes: &[BatchLinkOutcome]) -> BatchLinkSummary {
    let mut summary = BatchLinkSummary {
        total: outcomes.len(),
        ..BatchLinkSummary::default()
    };
    for outcome in outcomes {
        match outcome {
            BatchLinkOutcome::Linked {
                already_linked: true,
                ..
            } => summary.already_linked += 1,
            BatchLinkOutcome::Linked {
                already_linked: false,
                ..
            } => summary.newly_linked += 1,
            BatchLinkOutcome::Failed { .. } => summary.failed += 1,
        }
    }
    summary
}

/// Maps a [`PeerLinkError`] into a [`PeerLinkFailureKind`] (`#39426`).
#[must_use]
pub const fn classify_batch_failure(error: &PeerLinkError) -> PeerLinkFailureKind {
    if error.answered_as.is_some() {
        PeerLinkFailureKind::WrongEnvironment
    } else {
        match error.reason {
            PeerLinkErrorReason::Unreachable | PeerLinkErrorReason::NotALegionCodeEnvironment => {
                PeerLinkFailureKind::Unreachable
            }
            PeerLinkErrorReason::PairingRejected => PeerLinkFailureKind::CodeRefused,
            _ => PeerLinkFailureKind::Other,
        }
    }
}

/// Links each machine in `targets`, skipping already-linked unexpired targets without spending
/// their pairing codes (`#39426`).
pub async fn run_batch_linking(
    client: &LivePeerClient,
    conn: &Connection,
    secrets: &FileSecretStore,
    targets: &[BatchLinkTarget],
    client_label: &str,
    self_environment_id: Option<&EnvironmentId>,
    now_iso: &str,
) -> Vec<BatchLinkOutcome> {
    let service = PeerLinksService::new(secrets.clone());
    let mut outcomes = Vec::with_capacity(targets.len());
    for target in targets {
        if let Ok(Some(existing)) = service.get(conn, target.environment_id.as_str())
            && existing.expires_at.as_str() > now_iso
        {
            outcomes.push(BatchLinkOutcome::Linked {
                link: existing,
                already_linked: true,
            });
            continue;
        }
        let mut input = LivePeerLinkInput::new(
            &target.url,
            &target.pairing_code,
            target.access,
            client_label,
        );
        input.alternate_urls.clone_from(&target.alternate_urls);
        input.self_environment_id = self_environment_id.cloned();
        input.expected_environment_id = Some(target.environment_id.clone());
        input.linked_at = Some(now_iso.to_owned());
        match client.link_peer_over_http(conn, secrets, input).await {
            Ok(link) => outcomes.push(BatchLinkOutcome::Linked {
                link,
                already_linked: false,
            }),
            Err(err) => outcomes.push(BatchLinkOutcome::Failed {
                environment_id: target.environment_id.clone(),
                label: target.label.clone(),
                kind: classify_batch_failure(&err),
                message: err.message,
            }),
        }
    }
    outcomes
}

/// Links `input` via [`PeerLinksService::link_unless_linked`], keeping existing unexpired links intact.
pub fn link_unless_linked_local(
    service: &PeerLinksService,
    conn: &Connection,
    now_iso: &str,
    input: InsertPeerLinkInput,
) -> Result<(PeerLink, bool), PeerLinkError> {
    let outcome = service.link_unless_linked(conn, now_iso, input)?;
    Ok((outcome.link, outcome.already_linked))
}
