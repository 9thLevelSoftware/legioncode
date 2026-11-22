//! Pure client state for read-only linked-environment thread streams (`1cb3a8c753` / #39406),
//! bounded batch peer linking (`8e8ff9b243` / #39426), and multi-repo thread pin selection
//! (`60863edc41` / #35925).

use serde::{Deserialize, Serialize};
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_contracts::orchestration::{Run, RunStatus};
use t3_contracts::peer_links::{PeerLinkThreadReadError, PeerLinkThreadReadReason};

pub use crate::linked_threads_batch::{
    LINK_BATCH_CONCURRENCY, LinkBatchCandidate, LinkBatchItemStatus, LinkBatchQueue,
    LinkBatchSummary, LinkBlockReason, LinkListState, LinkableMachine, link_batch_candidates,
    link_list_state,
};
pub use crate::linked_threads_repos::{
    PinnedThreadRepository, RepositorySectionOrigin, ThreadRepositoryRef, ThreadRepositorySection,
    ThreadRepositorySections, append_tracked_folders, pin_thread_repository,
    suggested_tracked_folders, thread_git_repositories, unpin_thread_repository,
};
pub use crate::linked_threads_worktree::{
    RewindFileRestoreOutcome, addable_task_folder_repositories, pinnable_thread_repositories,
    resolve_rewind_file_restore, resolve_worktree_repository_selection, thread_repository_sections,
};

/// Reconnect delay in milliseconds when following a linked-environment thread stream (`#39406`).
pub const LINKED_THREAD_RETRY_MS: u64 = 15_000;

/// Metadata identifying a thread hosted on a peer environment read via `via_environment_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedThreadTarget {
    /// Connected environment relaying read requests over its peer link.
    pub via_environment_id: EnvironmentId,
    /// Owning peer environment identifier.
    pub environment_id: EnvironmentId,
    /// Target thread identifier on the peer environment.
    pub thread_id: ThreadId,
    /// Best known human-readable peer environment label.
    pub label: String,
    /// Best known thread title before the first stream snapshot arrives.
    pub title: Option<String>,
}

/// Opening strategy resolved for a thread target (`resolveLinkedThreadOpen`, `#39406`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkedThreadOpen {
    /// Client opens directly against its own saved/live connection to `environment_id`.
    Direct,
    /// Client opens read-only proxied stream through `via_environment_id`'s link.
    Linked,
    /// Neither an own connection nor an active link exists.
    Unavailable,
}

/// Resolves how a client opens `environment_id` from `via_environment_id` (`#39406`).
#[must_use]
pub fn resolve_linked_thread_open(
    environment_id: &EnvironmentId,
    via_environment_id: &EnvironmentId,
    own_connected: Option<bool>,
    linked: bool,
) -> LinkedThreadOpen {
    if environment_id == via_environment_id {
        return LinkedThreadOpen::Direct;
    }
    if own_connected == Some(true) {
        return LinkedThreadOpen::Direct;
    }
    if linked {
        return LinkedThreadOpen::Linked;
    }
    match own_connected {
        None => LinkedThreadOpen::Unavailable,
        Some(_) => LinkedThreadOpen::Direct,
    }
}

/// User-facing failure presentation for linked-environment thread reads (`#39406`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedThreadFailure {
    /// Short error headline.
    pub title: String,
    /// Actionable explanation naming `via` and peer `label`.
    pub detail: String,
    /// True when retrying later without configuration changes may succeed (peer unreachable).
    pub retryable: bool,
}

/// Translates a [`PeerLinkThreadReadError`] (or legacy unknown-method failure when `None`) into
/// structured retryability and guidance (`#39406`).
#[must_use]
pub fn classify_linked_thread_failure(
    error: Option<&PeerLinkThreadReadError>,
    via_label: &str,
    peer_label: &str,
) -> LinkedThreadFailure {
    let Some(err) = error else {
        return LinkedThreadFailure {
            title: format!("{peer_label} could not be read through {via_label}"),
            detail: format!(
                "{via_label} returned an unsupported response for linked thread reads."
            ),
            retryable: false,
        };
    };
    let label = err.label.as_deref().unwrap_or(peer_label);
    match err.reason {
        PeerLinkThreadReadReason::Unreachable => LinkedThreadFailure {
            title: format!("{label} is unreachable from {via_label}"),
            detail: if err.message.trim().is_empty() {
                format!("Waiting to reconnect to {label} through {via_label}.")
            } else {
                err.message.clone()
            },
            retryable: true,
        },
        PeerLinkThreadReadReason::Disabled => LinkedThreadFailure {
            title: format!("Linked reading is disabled on {via_label}"),
            detail: err.message.clone(),
            retryable: false,
        },
        PeerLinkThreadReadReason::UnknownLink | PeerLinkThreadReadReason::Expired => {
            LinkedThreadFailure {
                title: format!("Link to {label} is missing or expired"),
                detail: format!("Relink {label} on {via_label} to resume viewing this thread."),
                retryable: false,
            }
        }
        PeerLinkThreadReadReason::Rejected => LinkedThreadFailure {
            title: format!("{label} refused the read request"),
            detail: err.message.clone(),
            retryable: false,
        },
        PeerLinkThreadReadReason::ThreadNotFound => LinkedThreadFailure {
            title: "Thread not found".to_owned(),
            detail: err.message.clone(),
            retryable: false,
        },
        PeerLinkThreadReadReason::Incompatible => LinkedThreadFailure {
            title: format!("{label} runs an older incompatible server"),
            detail: err.message.clone(),
            retryable: false,
        },
    }
}

/// Returns `true` when any run in a linked projection is still preparing/queued/running/waiting.
#[must_use]
pub fn is_linked_thread_working(runs: &[Run]) -> bool {
    runs.iter().any(|run| {
        matches!(
            run.status,
            RunStatus::Preparing
                | RunStatus::Queued
                | RunStatus::Starting
                | RunStatus::Running
                | RunStatus::Waiting
        )
    })
}

/// Pagination state for loading older linked-thread history (`#39406`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HistoryLoadStatus {
    /// No pagination request is currently in flight.
    #[default]
    Idle,
    /// A `load_earlier` request is currently waiting for a response.
    Loading,
    /// The latest `load_earlier` request failed.
    Failed,
}

/// Pure view state for a read-only linked-environment thread stream (`#39406`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedThreadViewState {
    /// True once at least one stream snapshot has arrived.
    pub has_snapshot: bool,
    /// Latest applied event/snapshot sequence number.
    pub snapshot_sequence: i64,
    /// Thread title synced from snapshots or retitled stream deltas.
    pub title: Option<String>,
    /// Next pagination cursor (`None` when all history loaded).
    pub history_cursor: Option<String>,
    /// True while older history exists on the peer.
    pub has_more_history: bool,
    /// Single-flight pagination status preventing duplicate `load_earlier` RPCs while in flight.
    pub history_status: HistoryLoadStatus,
    /// Whether any run on the linked thread is active.
    pub working: bool,
    /// Latest read failure, if any (snapshot remains visible if `keep_linked_thread_view`).
    pub failure: Option<LinkedThreadFailure>,
}

impl LinkedThreadViewState {
    /// Returns whether an existing snapshot view should stay visible across `failure`
    /// (`keepLinkedThreadView`, `#39406`).
    #[must_use]
    pub const fn should_keep_view_on_failure(&self, failure: &LinkedThreadFailure) -> bool {
        self.has_snapshot && failure.retryable
    }

    /// Applies an initial or refreshed snapshot item from the linked thread stream (`#39406`).
    pub fn apply_snapshot(
        &mut self,
        sequence: i64,
        title: Option<String>,
        history_cursor: Option<String>,
        has_more_history: bool,
        runs: &[Run],
    ) {
        self.has_snapshot = true;
        self.snapshot_sequence = sequence;
        if title.is_some() {
            self.title = title;
        }
        self.history_cursor = history_cursor;
        self.has_more_history = has_more_history;
        self.working = is_linked_thread_working(runs);
        self.failure = None;
    }

    /// Applies a retitle delta (`sequence`, `title`) emitted on the linked stream (`#39406`).
    pub fn apply_title_delta(&mut self, sequence: i64, title: impl Into<String>) {
        if sequence > self.snapshot_sequence {
            self.snapshot_sequence = sequence;
            self.title = Some(title.into());
        }
    }

    /// Requests the next earlier history page if a cursor exists and no load is already active.
    pub fn begin_load_earlier(&mut self) -> Option<String> {
        if self.history_status == HistoryLoadStatus::Loading {
            return None;
        }
        let cursor = self.history_cursor.clone()?;
        self.history_status = HistoryLoadStatus::Loading;
        Some(cursor)
    }

    /// Completes an in-flight `load_earlier` page request (`#39406`).
    pub fn finish_history_page(
        &mut self,
        next_cursor: Option<String>,
        has_more_history: bool,
        failed: bool,
    ) {
        if failed {
            self.history_status = HistoryLoadStatus::Failed;
        } else {
            self.history_status = HistoryLoadStatus::Idle;
            self.history_cursor = next_cursor;
            self.has_more_history = has_more_history;
        }
    }

    /// Records a stream failure while retaining the snapshot view if `retryable` (`#39406`).
    pub fn record_failure(&mut self, failure: LinkedThreadFailure) {
        if !self.should_keep_view_on_failure(&failure) {
            self.has_snapshot = false;
        }
        self.failure = Some(failure);
    }
}
