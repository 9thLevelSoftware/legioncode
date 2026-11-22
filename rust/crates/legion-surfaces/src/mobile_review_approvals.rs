//! Pull request reviewer approval freshness evaluation (`68299face0` #36868).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Normalized pull request review decision (`normalizedReviewState`, `#36868`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PullRequestReviewState {
    /// Reviewer approved changes.
    Approved,
    /// Reviewer requested changes.
    ChangesRequested,
    /// Reviewer left a comment without approval/rejection.
    Commented,
    /// Prior review was dismissed.
    Dismissed,
}

impl PullRequestReviewState {
    /// Parses case-insensitive, hyphen-or-underscore review states across GitHub, Forgejo,
    /// Bitbucket, and `GitCafe`.
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        let normalized = raw?.trim().to_ascii_uppercase().replace('-', "_");
        match normalized.as_str() {
            "APPROVED" => Some(Self::Approved),
            "CHANGES_REQUESTED" => Some(Self::ChangesRequested),
            "COMMENTED" => Some(Self::Commented),
            "DISMISSED" => Some(Self::Dismissed),
            _ => None,
        }
    }
}

/// Input review record used by [`evaluate_pull_request_reviews`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReviewEntry {
    /// Unique review identifier.
    pub id: String,
    /// Author login if known (`None` renders localized `"Unknown reviewer"`).
    pub author_login: Option<String>,
    /// Normalized review state string (`"APPROVED"`, `"changes-requested"`, etc.).
    pub raw_state: Option<String>,
    /// Unix timestamp in milliseconds when the review was submitted.
    pub created_at_ms: i64,
}

/// Evaluated review row with outdated approval detection (`#36868`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedPullRequestReview {
    /// Unique review identifier.
    pub id: String,
    /// Author login if present.
    pub author_login: Option<String>,
    /// Normalized review decision.
    pub state: PullRequestReviewState,
    /// Unix timestamp in milliseconds when submitted.
    pub created_at_ms: i64,
    /// `true` when `state == Approved` and a newer commit arrived afterward.
    pub outdated: bool,
}

impl EvaluatedPullRequestReview {
    /// `true` only when the approval is still valid for the latest commit.
    #[must_use]
    pub const fn is_current_approval(&self) -> bool {
        matches!(self.state, PullRequestReviewState::Approved) && !self.outdated
    }
}

/// High-level summary status across deduplicated PR reviews (`#36868`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewSummary {
    /// Deduplicated reviews with `outdated` flags populated.
    pub reviews: Vec<EvaluatedPullRequestReview>,
    /// Number of non-outdated approvals.
    pub current_approval_count: usize,
    /// Number of outdated approvals.
    pub outdated_approval_count: usize,
    /// Number of active change-request reviews.
    pub changes_requested_count: usize,
}

/// Deduplicates per-reviewer decisions and marks approvals submitted before `commit_timestamps_ms`
/// as `outdated` (`pullRequestStatus.ts` / `#36868`).
#[must_use]
pub fn evaluate_pull_request_reviews(
    entries: &[PullRequestReviewEntry],
    commit_timestamps_ms: &[i64],
) -> PullRequestReviewSummary {
    let mut latest: BTreeMap<String, (PullRequestReviewEntry, PullRequestReviewState)> =
        BTreeMap::new();
    let mut comments: BTreeMap<String, PullRequestReviewEntry> = BTreeMap::new();

    for entry in entries {
        let Some(state) = PullRequestReviewState::parse(entry.raw_state.as_deref()) else {
            continue;
        };
        let key = entry
            .author_login
            .clone()
            .unwrap_or_else(|| entry.id.clone());
        if matches!(state, PullRequestReviewState::Commented) {
            let replace = comments
                .get(&key)
                .is_none_or(|prev| entry.created_at_ms >= prev.created_at_ms);
            if replace {
                comments.insert(key, entry.clone());
            }
            continue;
        }
        let replace = latest
            .get(&key)
            .is_none_or(|(prev, _)| entry.created_at_ms >= prev.created_at_ms);
        if replace {
            latest.insert(key, (entry.clone(), state));
        }
    }

    let newest_commit = commit_timestamps_ms.iter().copied().max().unwrap_or(0);
    let mut reviews = Vec::new();
    for (entry, state) in latest.values() {
        if matches!(state, PullRequestReviewState::Dismissed) {
            continue;
        }
        reviews.push(EvaluatedPullRequestReview {
            id: entry.id.clone(),
            author_login: entry.author_login.clone(),
            state: *state,
            created_at_ms: entry.created_at_ms,
            outdated: matches!(state, PullRequestReviewState::Approved)
                && entry.created_at_ms < newest_commit,
        });
    }
    for (key, entry) in comments {
        if !latest.contains_key(&key) {
            reviews.push(EvaluatedPullRequestReview {
                id: entry.id,
                author_login: entry.author_login,
                state: PullRequestReviewState::Commented,
                created_at_ms: entry.created_at_ms,
                outdated: false,
            });
        }
    }

    let current_approval_count = reviews.iter().filter(|r| r.is_current_approval()).count();
    let outdated_approval_count = reviews
        .iter()
        .filter(|r| matches!(r.state, PullRequestReviewState::Approved) && r.outdated)
        .count();
    let changes_requested_count = reviews
        .iter()
        .filter(|r| matches!(r.state, PullRequestReviewState::ChangesRequested))
        .count();

    PullRequestReviewSummary {
        reviews,
        current_approval_count,
        outdated_approval_count,
        changes_requested_count,
    }
}
