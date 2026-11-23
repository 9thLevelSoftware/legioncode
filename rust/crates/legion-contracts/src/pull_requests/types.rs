//! Supporting metadata types for thread pull-request links (`threadPullRequest.ts`).

use serde::{Deserialize, Serialize};

/// Lifecycle state of a pull or merge request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullRequestState {
    /// Open and active.
    Open,
    /// Closed without merging.
    Closed,
    /// Merged into its target branch.
    Merged,
}

/// Aggregated review decision on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullRequestReviewDecision {
    /// Approved by required reviewers.
    Approved,
    /// Changes requested by a reviewer.
    ChangesRequested,
    /// Review still required.
    ReviewRequired,
}

/// Aggregated CI check status on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullRequestChecksState {
    /// All checks passing.
    Passing,
    /// At least one check failed.
    Failing,
    /// Checks are pending or running.
    Pending,
}

/// Mergeability state reported by the forge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullRequestMergeability {
    /// Cleanly mergeable.
    Mergeable,
    /// Merge conflicts present.
    Conflicting,
    /// Mergeability not yet computed.
    Unknown,
}

/// Forge user or bot account metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestActor {
    /// Whether this actor is a bot account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_bot: Option<bool>,
    /// Login handle.
    pub login: String,
    /// Display name, or `null`.
    pub name: Option<String>,
    /// Avatar image URL, or `null`.
    pub avatar_url: Option<String>,
}

/// Cached metadata snapshot for a linked pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestSnapshot {
    /// Current forge state.
    pub state: PullRequestState,
    /// Pull request title.
    pub title: String,
    /// Head branch name.
    pub head_branch: String,
    /// Base branch name.
    pub base_branch: String,
    /// Whether the pull request is a draft.
    pub is_draft: bool,
    /// Last forge update timestamp, or `null`.
    pub updated_at: Option<String>,
    /// Timestamp when `LegionCode` synced this snapshot.
    pub synced_at: String,
    /// Timestamp when the PR was closed, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<Option<String>>,
    /// Timestamp when the PR was merged, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<Option<String>>,
    /// Author metadata, if fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Option<PullRequestActor>>,
    /// Line additions count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u64>,
    /// Line deletions count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
    /// Changed files count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<u64>,
    /// Review decision, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<Option<PullRequestReviewDecision>>,
    /// Aggregated checks state, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_state: Option<Option<PullRequestChecksState>>,
    /// Mergeability verdict, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeability: Option<PullRequestMergeability>,
    /// Whether the forge indicates a rebase is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebase_required: Option<bool>,
}

/// One layer in a native pull-request stack, ordered bottom to top.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestStackLayer {
    /// Positive PR number.
    pub number: u32,
    /// Branch name for this stack layer.
    pub head_branch: String,
    /// Current state of this layer's PR.
    pub state: PullRequestState,
}

/// Native stack metadata reported by a forge integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestStack {
    /// Stack kind (`"native"`).
    pub kind: String,
    /// Forge stack identifier.
    pub id: String,
    /// Stack number.
    pub number: u32,
    /// Stack web URL.
    pub url: String,
    /// Base trunk branch of the stack.
    pub base: String,
    /// Layers from bottom to top.
    pub layers: Vec<ThreadPullRequestStackLayer>,
}

/// Automated pull-request watch state for CI checks, conflicts, and review comments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestWatch {
    /// ISO timestamp when watching started.
    pub started_at: String,
    /// Head commit SHA last inspected, or `null`.
    pub head_sha: Option<String>,
    /// Check names that have failed on the current head.
    pub failed_checks: Vec<String>,
    /// Whether required checks passed.
    pub passed: bool,
    /// Check names that have passed on the current head.
    #[serde(default)]
    pub passed_checks: Vec<String>,
    /// Optional checks still pending after required checks passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owed_checks: Option<Vec<String>>,
    /// Workflow runs waiting for maintainer approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_approval: Option<Vec<String>>,
    /// Whether pending approval was already announced to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_announced: Option<bool>,
    /// High-water timestamp for processed PR remarks.
    pub remarks_through: String,
    /// Remark IDs already processed at `remarks_through`.
    pub remark_ids: Vec<String>,
    /// Whether the PR currently has merge conflicts.
    pub conflicting: bool,
    /// Whether the PR requires a rebase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebase_required: Option<bool>,
    /// Last observed merge queue removal ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_queue_removal_id: Option<Option<String>>,
    /// Consecutive comment-only wake count.
    pub wakes: u64,
    /// Baseline capture stage (`"all"` or `"checks"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_baseline: Option<String>,
    /// Timestamp since checks were awaited on the head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_awaited_since: Option<Option<String>>,
    /// Whether checks were pending on the last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_pending: Option<bool>,
    /// Watch duration target (`"merged"` when watching until merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}
