//! Batch peer-environment linking candidate derivation and bounded concurrency queue
//! (`8e8ff9b243` / #39426).

use std::collections::BTreeSet;

use legion_contracts::ids::EnvironmentId;
use legion_contracts::peer_links::{PeerLinkError, PeerLinkFailureKind, peer_link_failure_kind};
use serde::{Deserialize, Serialize};

/// Maximum concurrent peer pairing requests started by [`LinkBatchQueue`] (`#39426`).
pub const LINK_BATCH_CONCURRENCY: usize = 4;

/// State of the linking environment's known-links query (`linkListState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkListState {
    /// Links list has not finished loading yet.
    Loading,
    /// Querying links from the linking environment failed.
    Failed,
    /// Known links loaded cleanly.
    Loaded,
}

/// Derives [`LinkListState`] from query completion/error flags (`#39426`).
#[must_use]
pub const fn link_list_state(has_data: bool, has_error: bool) -> LinkListState {
    if has_data {
        LinkListState::Loaded
    } else if has_error {
        LinkListState::Failed
    } else {
        LinkListState::Loading
    }
}

/// Reason a candidate machine cannot currently be linked in a batch (`#39426`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkBlockReason {
    /// Known-links list is not loaded yet (`loading` or `failed`).
    LinksUnknown,
    /// Target environment is already linked to the linking host.
    Linked,
    /// Target machine has no active connection (or is `connectOnly`).
    NotConnected,
    /// User lacks mint/pairing access on the target environment.
    NoAccess,
    /// Target machine advertises zero reachable URLs (e.g. loopback-only).
    NoAddress,
}

/// Target environment metadata evaluated for batch linking (`#39426`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkableMachine {
    /// Target environment identifier.
    pub environment_id: EnvironmentId,
    /// User-facing machine label.
    pub label: String,
    /// Machine host classification (`server`, `desktop`, ...).
    pub machine: String,
    /// True when client currently holds a live connection to this environment.
    pub connected: bool,
    /// True when the environment only dials outbound relays and cannot mint directly.
    pub connect_only: bool,
    /// True when the linking environment already holds a link to this environment.
    pub linked: bool,
    /// Reachable HTTP(S) URLs advertised by this machine.
    pub urls: Vec<String>,
}

/// Evaluated candidate with optional block reason (`None` = linkable now).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkBatchCandidate {
    /// Candidate environment details.
    pub machine: LinkableMachine,
    /// Why the machine cannot be linked yet, or `None` when eligible.
    pub blocked: Option<LinkBlockReason>,
}

/// Evaluates which machines can be batch-linked from the linking host (`#39426`).
#[must_use]
pub fn link_batch_candidates(
    machines: &[LinkableMachine],
    writable_environments: &BTreeSet<EnvironmentId>,
    list_state: LinkListState,
) -> Vec<LinkBatchCandidate> {
    machines
        .iter()
        .map(|machine| {
            let blocked = if list_state != LinkListState::Loaded {
                Some(LinkBlockReason::LinksUnknown)
            } else if machine.linked {
                Some(LinkBlockReason::Linked)
            } else if !machine.connected || machine.connect_only {
                Some(LinkBlockReason::NotConnected)
            } else if !writable_environments.contains(&machine.environment_id) {
                Some(LinkBlockReason::NoAccess)
            } else if machine.urls.is_empty() {
                Some(LinkBlockReason::NoAddress)
            } else {
                None
            };
            LinkBatchCandidate {
                machine: machine.clone(),
                blocked,
            }
        })
        .collect()
}

/// Lifecycle state of one target inside a [`LinkBatchQueue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkBatchItemStatus {
    /// Waiting in queue for a worker slot.
    Pending,
    /// Actively pairing within the `LINK_BATCH_CONCURRENCY` window.
    InFlight,
    /// Successfully paired and linked.
    Linked,
    /// Failed with a classified peer-link failure reason.
    Failed {
        /// Classified error category (`unreachable`, `code_refused`, `wrong_environment`, `other`).
        kind: PeerLinkFailureKind,
        /// Safe failure summary message.
        message: String,
    },
}

/// Aggregate progress counts for UI status reporting (`linkBatchSummary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkBatchSummary {
    /// Successfully linked environments.
    pub linked: usize,
    /// Failed environments (eligible for retry).
    pub failed: usize,
    /// Targets waiting for a concurrency slot.
    pub pending: usize,
    /// Targets currently running in flight (`<= LINK_BATCH_CONCURRENCY`).
    pub in_flight: usize,
}

/// Bounded-concurrency queue (`<= LINK_BATCH_CONCURRENCY`) coordinating peer batch linking and
/// retrying only failed entries (`#39426`).
#[derive(Debug, Clone, Default)]
pub struct LinkBatchQueue {
    order: Vec<EnvironmentId>,
    statuses: std::collections::BTreeMap<EnvironmentId, LinkBatchItemStatus>,
}

impl LinkBatchQueue {
    /// Initializes a batch queue from eligible (unblocked) candidate environments.
    #[must_use]
    pub fn from_candidates(candidates: &[LinkBatchCandidate]) -> Self {
        let mut order = Vec::new();
        let mut statuses = std::collections::BTreeMap::new();
        for c in candidates {
            if c.blocked.is_none() {
                let id = c.machine.environment_id.clone();
                if statuses
                    .insert(id.clone(), LinkBatchItemStatus::Pending)
                    .is_none()
                {
                    order.push(id);
                }
            }
        }
        Self { order, statuses }
    }

    /// Claims up to remaining concurrency slots (`LINK_BATCH_CONCURRENCY - in_flight`).
    pub fn take_next_batch(&mut self) -> Vec<EnvironmentId> {
        let in_flight = self
            .statuses
            .values()
            .filter(|s| matches!(s, LinkBatchItemStatus::InFlight))
            .count();
        let available = LINK_BATCH_CONCURRENCY.saturating_sub(in_flight);
        if available == 0 {
            return Vec::new();
        }
        let mut claimed = Vec::with_capacity(available);
        for id in &self.order {
            if claimed.len() >= available {
                break;
            }
            if matches!(self.statuses.get(id), Some(LinkBatchItemStatus::Pending)) {
                self.statuses
                    .insert(id.clone(), LinkBatchItemStatus::InFlight);
                claimed.push(id.clone());
            }
        }
        claimed
    }

    /// Records the result for one in-flight environment pairing attempt.
    pub fn record_outcome(
        &mut self,
        environment_id: &EnvironmentId,
        outcome: Result<(), &PeerLinkError>,
    ) {
        let next = match outcome {
            Ok(()) => LinkBatchItemStatus::Linked,
            Err(err) => LinkBatchItemStatus::Failed {
                kind: peer_link_failure_kind(err),
                message: err.message.clone(),
            },
        };
        if self.statuses.contains_key(environment_id) {
            self.statuses.insert(environment_id.clone(), next);
        }
    }

    /// Re-queues only failed targets (leaving already-linked ones untouched) and returns them.
    pub fn retry_failed(&mut self) -> Vec<EnvironmentId> {
        let mut retried = Vec::new();
        for id in &self.order {
            if matches!(
                self.statuses.get(id),
                Some(LinkBatchItemStatus::Failed { .. })
            ) {
                self.statuses
                    .insert(id.clone(), LinkBatchItemStatus::Pending);
                retried.push(id.clone());
            }
        }
        retried
    }

    /// Returns aggregate counts across all queued environments.
    #[must_use]
    pub fn summary(&self) -> LinkBatchSummary {
        let mut summary = LinkBatchSummary {
            linked: 0,
            failed: 0,
            pending: 0,
            in_flight: 0,
        };
        for status in self.statuses.values() {
            match status {
                LinkBatchItemStatus::Linked => summary.linked += 1,
                LinkBatchItemStatus::Failed { .. } => summary.failed += 1,
                LinkBatchItemStatus::Pending => summary.pending += 1,
                LinkBatchItemStatus::InFlight => summary.in_flight += 1,
            }
        }
        summary
    }

    /// Returns the recorded status for `environment_id`, if queued.
    #[must_use]
    pub fn status(&self, environment_id: &EnvironmentId) -> Option<&LinkBatchItemStatus> {
        self.statuses.get(environment_id)
    }
}
