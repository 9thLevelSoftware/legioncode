//! Mobile review viewer state models, PR approval freshness, failed git-section fallback,
//! secret-request multi-value/visibility state, test-file filtering, and localized selection menus.
//!
//! Pure Safe Rust and `wasm32-unknown-unknown` clean.

use serde::{Deserialize, Serialize};

pub use crate::mobile_review_approvals::{
    EvaluatedPullRequestReview, PullRequestReviewEntry, PullRequestReviewState,
    PullRequestReviewSummary, evaluate_pull_request_reviews,
};
pub use crate::mobile_review_filters::{
    ChangedFileKind, FilteredReviewFiles, ReviewFileEntry, classify_changed_file,
    filter_review_files, is_test_changed_file, shown_patch_indices,
};
pub use crate::mobile_review_i18n::{
    ReviewApprovalLabels, SUPPORTED_REVIEW_LOCALES, TextSelectionMenuLabels,
};
pub use crate::mobile_review_secrets::{
    SecretCardState, SecretInputSpec, decline_secret_request_input, secret_request_inputs,
    secret_request_privacy_note,
};

/// Minimum selection marker width in density-independent points (`4dp`, `#40295`).
pub const MIN_SELECTION_MARKER_WIDTH_DP: f32 = 4.0;

/// Inclusive normalized source-line range highlighted when linking directly into a source file
/// (`#40295`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedLineRange {
    /// First selected 1-based line number.
    pub start_line: u32,
    /// Last selected 1-based line number (`>= start_line`).
    pub end_line: u32,
}

impl LinkedLineRange {
    /// Creates a normalized line range, ordering `a` and `b` ascending.
    #[must_use]
    pub const fn new(a: u32, b: u32) -> Self {
        if a <= b {
            Self {
                start_line: a,
                end_line: b,
            }
        } else {
            Self {
                start_line: b,
                end_line: a,
            }
        }
    }

    /// Returns `true` when `line` falls inside `[start_line, end_line]`.
    #[must_use]
    pub const fn contains(self, line: u32) -> bool {
        line >= self.start_line && line <= self.end_line
    }

    /// Number of highlighted lines covered by this range.
    #[must_use]
    pub const fn line_count(self) -> u32 {
        self.end_line - self.start_line + 1
    }
}

/// Computes the iOS selection marker bar width (`max(change_bar_width_dp, 4.0)`, `#40295`).
#[must_use]
pub fn selection_marker_width_dp(change_bar_width_dp: f32) -> f32 {
    if change_bar_width_dp.is_finite() {
        change_bar_width_dp.max(MIN_SELECTION_MARKER_WIDTH_DP)
    } else {
        MIN_SELECTION_MARKER_WIDTH_DP
    }
}

/// Computes the Android selection marker bar width (`max(change_bar_width_px, 4.0 * density)`).
#[must_use]
pub fn selection_marker_width_px(change_bar_width_px: f32, density: f32) -> f32 {
    let safe_density = if density.is_finite() && density > 0.0 {
        density
    } else {
        1.0
    };
    let safe_bar = if change_bar_width_px.is_finite() && change_bar_width_px > 0.0 {
        change_bar_width_px
    } else {
        0.0
    };
    safe_bar.max(MIN_SELECTION_MARKER_WIDTH_DP * safe_density)
}

/// Kind of review section displayed in the mobile diff sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewSectionKind {
    /// Checkpoint diff from a single thread turn.
    Turn,
    /// Uncommitted working-tree changes (`git:working-tree`).
    WorkingTree,
    /// Branch comparison diff (`git:branch-range`).
    BranchRange,
}

/// Git-backed review section kind (`working-tree` vs. `branch-range`, `#24841`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewGitSectionKind {
    /// Uncommitted changes in the working tree.
    WorkingTree,
    /// Branch comparison relative to base ref.
    BranchRange,
}

impl ReviewGitSectionKind {
    /// Canonical section identifier (`"git:working-tree"` or `"git:branch-range"`).
    #[must_use]
    pub const fn section_id(self) -> &'static str {
        match self {
            Self::WorkingTree => "git:working-tree",
            Self::BranchRange => "git:branch-range",
        }
    }

    /// Placeholder section title while loading or after failure.
    #[must_use]
    pub const fn placeholder_title(self) -> &'static str {
        match self {
            Self::WorkingTree => "Uncommitted",
            Self::BranchRange => "Changes",
        }
    }
}

/// Independent fetch state for each git review source (`#24841`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitSectionLoadState {
    /// Loaded cleanly (or disabled without placeholder).
    Idle,
    /// Diff preview request in flight.
    Loading,
    /// Diff preview request failed independently of sibling sections.
    Failed,
}

/// Loaded diff source for a git review section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDiffSource {
    /// Source kind.
    pub kind: ReviewGitSectionKind,
    /// Section title.
    pub title: String,
    /// Optional subtitle.
    pub subtitle: Option<String>,
    /// Unified diff patch payload.
    pub diff: String,
    /// Whether the server truncated output.
    pub truncated: bool,
}

/// Turn checkpoint section passed into [`build_review_section_items`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCheckpointSection {
    /// Turn/checkpoint section identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Optional subtitle.
    pub subtitle: Option<String>,
    /// Loaded diff text, if fetched.
    pub diff: Option<String>,
    /// Whether the turn diff is loading.
    pub is_loading: bool,
}

/// Selectable section entry rendered in the review sheet (`#24841`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSectionItem {
    /// Unique section ID (`git:working-tree`, `git:branch-range`, or turn ID).
    pub id: String,
    /// Section kind.
    pub kind: ReviewSectionKind,
    /// Display title.
    pub title: String,
    /// Optional subtitle.
    pub subtitle: Option<String>,
    /// Diff text when loaded (`None` for loading/failed placeholder sections).
    pub diff: Option<String>,
    /// Whether the diff payload was truncated.
    pub truncated: bool,
    /// Whether this section is actively loading.
    pub is_loading: bool,
    /// Independent git fetch state (`Idle` for turns or loaded git sources).
    pub load_state: GitSectionLoadState,
}

/// Builds review sheet sections so an error or loading state in `branch-range` never hides an
/// available `working-tree` section (`Uncommitted`), and vice versa (`#24841`).
#[must_use]
pub fn build_review_section_items(
    checkpoints: &[ReviewCheckpointSection],
    git_sources: &[ReviewDiffSource],
    working_tree_state: GitSectionLoadState,
    branch_range_state: GitSectionLoadState,
) -> Vec<ReviewSectionItem> {
    let mut items: Vec<ReviewSectionItem> = checkpoints
        .iter()
        .map(|cp| ReviewSectionItem {
            id: cp.id.clone(),
            kind: ReviewSectionKind::Turn,
            title: cp.title.clone(),
            subtitle: cp.subtitle.clone(),
            diff: cp.diff.clone(),
            truncated: false,
            is_loading: cp.is_loading,
            load_state: GitSectionLoadState::Idle,
        })
        .collect();

    for (kind, state) in [
        (ReviewGitSectionKind::WorkingTree, working_tree_state),
        (ReviewGitSectionKind::BranchRange, branch_range_state),
    ] {
        if let Some(source) = git_sources.iter().find(|s| s.kind == kind) {
            items.push(ReviewSectionItem {
                id: kind.section_id().to_owned(),
                kind: git_to_section_kind(kind),
                title: source.title.clone(),
                subtitle: source.subtitle.clone(),
                diff: Some(source.diff.clone()),
                truncated: source.truncated,
                is_loading: false,
                load_state: GitSectionLoadState::Idle,
            });
        } else if !matches!(state, GitSectionLoadState::Idle) {
            items.push(ReviewSectionItem {
                id: kind.section_id().to_owned(),
                kind: git_to_section_kind(kind),
                title: kind.placeholder_title().to_owned(),
                subtitle: None,
                diff: None,
                truncated: false,
                is_loading: matches!(state, GitSectionLoadState::Loading),
                load_state: state,
            });
        }
    }
    items
}

const fn git_to_section_kind(kind: ReviewGitSectionKind) -> ReviewSectionKind {
    match kind {
        ReviewGitSectionKind::WorkingTree => ReviewSectionKind::WorkingTree,
        ReviewGitSectionKind::BranchRange => ReviewSectionKind::BranchRange,
    }
}

/// Rejects mismatched `sourceKind` vs. `file.sourceKind` selectors (`GitVcsDriverCore.ts`).
///
/// # Errors
/// Returns `"Review source selectors must agree."` when both selectors are present and differ.
pub fn validate_review_source_selectors(
    request_source_kind: Option<ReviewGitSectionKind>,
    file_source_kind: Option<ReviewGitSectionKind>,
) -> Result<Option<ReviewGitSectionKind>, &'static str> {
    if let (Some(req), Some(file)) = (request_source_kind, file_source_kind)
        && req != file
    {
        return Err("Review source selectors must agree.");
    }
    Ok(file_source_kind.or(request_source_kind))
}
