//! Automatic thread settlement policies covering `stop_on_settle` terminals (`#40489`),
//! PR ownership isolation across active+archived threads (`#40564`), and deferred open-PR branch
//! checks under `auto_settle_keep_pull_requests` (`#41116`).

use std::collections::HashSet;

use legion_contracts::ids::ThreadId;
use legion_contracts::pull_requests::{
    ThreadLinkedPullRequest, ThreadPullRequestKey, ThreadPullRequestLink, effective_pull_requests,
    legacy_pull_request_key,
};
use serde::{Deserialize, Serialize};

/// Minimal terminal session summary checked when guarding auto-settlement (`#40489`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSessionActivity {
    /// Terminal lifecycle status (`"running"`, `"exited"`, etc.).
    pub status: String,
    /// Whether the session owns a live OS subprocess PID.
    pub has_pid: bool,
    /// Whether settling the thread automatically closes this managed session (`#40489`).
    pub stop_on_settle: bool,
}

/// Returns `true` when any running non-`stop_on_settle` terminal subprocess should block
/// automatic thread settlement (`#40489`).
#[must_use]
pub fn has_blocking_terminal_subprocess(sessions: &[TerminalSessionActivity]) -> bool {
    sessions
        .iter()
        .any(|session| session.status == "running" && session.has_pid && !session.stop_on_settle)
}

/// State of a pull request resolved for a branch or thread link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchPullRequestState {
    /// Pull request is currently open.
    Open,
    /// Pull request was closed without merging.
    Closed,
    /// Pull request was merged.
    Merged,
}

/// Minimal branch pull-request lookup result checked for PR ownership (`#40564`, `#41116`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPullRequestRef {
    /// Normalized PR identity (`host`, `repository`, `number`).
    pub key: ThreadPullRequestKey,
    /// Current state (`open`, `closed`, `merged`).
    pub state: BranchPullRequestState,
}

/// Index of all PRs explicitly linked across active and archived threads (`#40564`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullRequestOwnershipIndex {
    explicitly_linked: HashSet<ThreadPullRequestKey>,
}

impl PullRequestOwnershipIndex {
    /// Collects visible PR links across both active and archived threads (`#40564`).
    #[must_use]
    pub fn from_thread_links<'a>(
        threads_pull_requests: impl IntoIterator<Item = &'a [ThreadPullRequestLink]>,
    ) -> Self {
        let mut explicitly_linked = HashSet::new();
        for links in threads_pull_requests {
            for link in effective_pull_requests(Some(links), None) {
                explicitly_linked.insert(ThreadPullRequestKey {
                    host: link.host.trim().to_ascii_lowercase(),
                    repository: link.repository.trim().to_ascii_lowercase(),
                    number: link.number,
                });
            }
        }
        Self { explicitly_linked }
    }

    /// Returns `true` when `branch_pr` is terminal (`closed`/`merged`) and explicitly linked by
    /// another thread, preventing unlinked threads sharing the branch from auto-settling (`#40564`).
    #[must_use]
    pub fn is_owned_by_another_thread(&self, branch_pr: Option<&BranchPullRequestRef>) -> bool {
        let Some(pr) = branch_pr else {
            return false;
        };
        if pr.state == BranchPullRequestState::Open {
            return false;
        }
        self.explicitly_linked.contains(&normalized_key(&pr.key))
    }

    /// Returns `true` when a branch-inferred reference (`linked_pull_request == None`,
    /// `branch_pull_request != None`) points to a PR explicitly owned by another thread (`#40564`).
    #[must_use]
    pub fn should_skip_branch_reference(
        &self,
        linked_pull_request: Option<&ThreadLinkedPullRequest>,
        branch_pull_request: Option<&ThreadLinkedPullRequest>,
    ) -> bool {
        if linked_pull_request.is_some() {
            return false;
        }
        let Some(reference) = branch_pull_request else {
            return false;
        };
        let key = legacy_pull_request_key(reference, None);
        self.explicitly_linked.contains(&key)
    }
}

fn normalized_key(key: &ThreadPullRequestKey) -> ThreadPullRequestKey {
    ThreadPullRequestKey {
        host: key.host.trim().to_ascii_lowercase(),
        repository: key.repository.trim().to_ascii_lowercase(),
        number: key.number,
    }
}

/// Immediate settlement plan before remote branch PR lookup (`#41116`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSettleRetentionPlan {
    /// Leave thread active (no settle needed now).
    KeepActive,
    /// Defer settling until branch lookup verifies whether an open PR exists (`#41116`).
    DeferForBranchLookup,
    /// Settle immediately without retaining a PR link.
    SettleImmediately,
}

/// Evaluates whether an inactive candidate thread should settle immediately or wait for a branch
/// open-PR check under `auto_settle_keep_pull_requests` (`#41116`).
#[must_use]
pub fn plan_auto_settle_with_pr_lookup(
    wants_settle_by_inactivity: bool,
    auto_settle_keep_pull_requests: bool,
    branch: Option<&str>,
    visible_pull_requests: &[ThreadPullRequestLink],
) -> AutoSettleRetentionPlan {
    if !wants_settle_by_inactivity {
        return AutoSettleRetentionPlan::KeepActive;
    }
    if auto_settle_keep_pull_requests && branch.is_some() && visible_pull_requests.is_empty() {
        return AutoSettleRetentionPlan::DeferForBranchLookup;
    }
    AutoSettleRetentionPlan::SettleImmediately
}

/// Resolves what happens to a deferred thread after the branch PR lookup completes or fails (`#41116`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredPrLookupOutcome {
    /// Open PR found on the branch: keep thread active (do not settle).
    KeepActiveForOpenPr(BranchPullRequestRef),
    /// Branch PR is non-open or lookup found nothing / failed: settle deferred thread now.
    SettleNow {
        /// Deferred thread ID to settle.
        thread_id: ThreadId,
    },
}

/// Resolves one deferred candidate thread against the completed branch lookup result (`#41116`).
#[must_use]
pub fn resolve_deferred_pr_settlement(
    thread_id: ThreadId,
    lookup_result: Option<&BranchPullRequestRef>,
) -> DeferredPrLookupOutcome {
    match lookup_result {
        Some(pr) if pr.state == BranchPullRequestState::Open => {
            DeferredPrLookupOutcome::KeepActiveForOpenPr(pr.clone())
        }
        _ => DeferredPrLookupOutcome::SettleNow { thread_id },
    }
}
