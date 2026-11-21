//! Thread pull-request link contracts and legacy compatibility helpers.
//!
//! Mirrors `packages/contracts/src/threadPullRequest.ts`,
//! `packages/source-control-core/src/shared/threadPullRequests.ts`, and
//! `packages/shared/src/threadPullRequests.ts`, fixing BUG-19:
//! - `pullRequests: Some(&[])` explicitly represents an unlinked thread and
//!   never falls back to stale `legacy` `linkedPullRequest`.
//! - Entries with `source == ThreadPullRequestLinkSource::StackDismissed` are
//!   filtered out of effective/visible pull requests.
//!
//! # Examples
//!
//! ```
//! use t3_contracts::ids::ProjectId;
//! use t3_contracts::pull_requests::{
//!     ThreadLinkedPullRequest, ThreadPullRequestLinkSource, effective_pull_requests,
//! };
//!
//! let legacy = ThreadLinkedPullRequest {
//!     project_id: ProjectId::new("project-1")?,
//!     repository: "pingdotgg/t3code".to_owned(),
//!     number: 42,
//!     url: "https://github.com/pingdotgg/t3code/pull/42".to_owned(),
//! };
//!
//! // Absent `pullRequests` synthesizes the legacy link.
//! let from_legacy = effective_pull_requests(None, Some(&legacy));
//! assert_eq!(from_legacy.len(), 1);
//! assert_eq!(from_legacy[0].host, "github.com");
//! assert_eq!(from_legacy[0].source, ThreadPullRequestLinkSource::Manual);
//!
//! // Explicit empty `Some(&[])` stays unlinked even if `legacy` is still present.
//! assert!(effective_pull_requests(Some(&[]), Some(&legacy)).is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod types;

use serde::{Deserialize, Serialize};

use crate::ids::ProjectId;

pub use types::{
    PullRequestActor, PullRequestChecksState, PullRequestMergeability, PullRequestReviewDecision,
    PullRequestState, ThreadPullRequestSnapshot, ThreadPullRequestStack,
    ThreadPullRequestStackLayer, ThreadPullRequestWatch,
};

const LEGACY_LINKED_AT: &str = "1970-01-01T00:00:00.000Z";

/// Legacy single pull-request link persisted on older V2 thread payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLinkedPullRequest {
    /// Project owning this pull request link.
    pub project_id: ProjectId,
    /// Canonical `owner/repo` path.
    pub repository: String,
    /// Positive pull-request number.
    pub number: u32,
    /// Web URL of the pull request.
    pub url: String,
}

/// Canonical key identifying a pull request across forges.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestKey {
    /// Normalized forge host name (e.g. `github.com`).
    pub host: String,
    /// Canonical `owner/repo` path.
    pub repository: String,
    /// Positive pull-request number.
    pub number: u32,
}

/// How a pull request became linked to a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadPullRequestLinkSource {
    /// Manually linked by the user.
    Manual,
    /// Created by T3 Code's PR workflow.
    Created,
    /// Linked by an agent during a run.
    Agent,
    /// Discovered as part of a native PR stack.
    Stack,
    /// Tombstone hiding a dismissed stack member from re-sync.
    StackDismissed,
}

/// Full multi-PR link stored on `OrchestrationV2AppThread::pullRequests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestLink {
    /// Normalized forge host name.
    pub host: String,
    /// Canonical `owner/repo` path.
    pub repository: String,
    /// Positive pull-request number.
    pub number: u32,
    /// Web URL of the pull request.
    pub url: String,
    /// Canonical repository web/clone URL, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_web_url: Option<String>,
    /// How this pull request was linked (or `stack-dismissed` tombstone).
    pub source: ThreadPullRequestLinkSource,
    /// ISO timestamp when this link was created.
    pub linked_at: String,
    /// Cached PR metadata snapshot, or `null`.
    pub snapshot: Option<ThreadPullRequestSnapshot>,
    /// Native stack metadata, or `null`.
    pub stack: Option<ThreadPullRequestStack>,
    /// Active PR watch state, if watching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<ThreadPullRequestWatch>,
    /// Last ended PR watch state for deduplicating resumed notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_watch: Option<ThreadPullRequestWatch>,
}

impl ThreadPullRequestLink {
    /// Returns `true` if this link is visible (not a `stack-dismissed` tombstone).
    pub fn is_visible(&self) -> bool {
        self.source != ThreadPullRequestLinkSource::StackDismissed
    }
}

fn extract_url_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next()?
    } else {
        host_port.split(':').next()?
    };
    let normalized = host.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// Derives the normalized [`ThreadPullRequestKey`] for a legacy [`ThreadLinkedPullRequest`].
pub fn legacy_pull_request_key(
    linked: &ThreadLinkedPullRequest,
    fallback_host: Option<&str>,
) -> ThreadPullRequestKey {
    let host = fallback_host
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .or_else(|| extract_url_host(&linked.url))
        .unwrap_or_else(|| "unknown".to_owned());
    ThreadPullRequestKey {
        host,
        repository: linked.repository.trim().to_ascii_lowercase(),
        number: linked.number,
    }
}

/// Resolves the effective visible pull requests for a thread (fixing BUG-19).
///
/// - When `pull_requests` is `Some(links)` (even `Some(&[])`), `legacy` is ignored
///   so an explicitly unlinked thread stays unlinked, and any `stack-dismissed`
///   tombstones are filtered out.
/// - Only when `pull_requests` is `None` does this fall back to synthesizing a
///   single manual link from `legacy`.
pub fn effective_pull_requests(
    pull_requests: Option<&[ThreadPullRequestLink]>,
    legacy: Option<&ThreadLinkedPullRequest>,
) -> Vec<ThreadPullRequestLink> {
    if let Some(links) = pull_requests {
        return links
            .iter()
            .filter(|link| link.is_visible())
            .cloned()
            .collect();
    }
    match legacy {
        None => Vec::new(),
        Some(linked) => {
            let key = legacy_pull_request_key(linked, None);
            vec![ThreadPullRequestLink {
                host: key.host,
                repository: key.repository,
                number: key.number,
                url: linked.url.clone(),
                repository_web_url: None,
                source: ThreadPullRequestLinkSource::Manual,
                linked_at: LEGACY_LINKED_AT.to_owned(),
                snapshot: None,
                stack: None,
                watch: None,
                ended_watch: None,
            }]
        }
    }
}
