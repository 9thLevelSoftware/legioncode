//! Fast per-file review diff rendering, `--no-index` untracked diff planning (`0dc32b5c8b`),
//! tracked-file untracked-listing bypass (`7ef7ba0e3b`), `completionToken` tracked preview reuse
//! (`67933a2236`), and preview merge-base reuse (`0fd4822887`).

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::OnceLock;

use regex::Regex;
use similar::{Algorithm, ChangeTag, TextDiff, capture_diff_slices, group_diff_ops};

use crate::checkpoints::diff_query::{DiffFileInput, DiffQueryOptions};
use crate::core::revisions::quote_git_patch_path;

/// Maximum number of `trackedOnly` preview snapshots kept in [`TrackedPreviewCache`] (`67933a2236`).
pub const TRACKED_PREVIEW_CAPACITY: usize = 16;
/// Expiration window for a `completionToken` tracked preview (`30s`, `67933a2236`).
pub const TRACKED_PREVIEW_TTL_MS: u64 = 30_000;

/// Classification of a file entry inside a review preview (`packages/contracts/src/review.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewDiffChangeKind {
    /// Modified tracked file.
    Change,
    /// Pure rename without content edits.
    RenamePure,
    /// Rename with content edits.
    RenameChanged,
    /// Newly added or untracked file.
    New,
    /// Deleted tracked file.
    Deleted,
}

/// Review diff source kind (`working-tree`, `branch-range`, `commit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewDiffSourceKind {
    /// Uncommitted working tree + index vs `HEAD`.
    WorkingTree,
    /// Branch divergence against merge base.
    BranchRange,
    /// Single historical commit diff.
    Commit,
}

/// Strategy decided before invoking `git` for untracked working-tree paths (`0dc32b5c8b`, `7ef7ba0e3b`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UntrackedIndexPlan {
    /// True when reading one path cleanly without copying `.git/index` or scanning staged deletions.
    pub isolated_single_file: bool,
    /// True when `git ls-files --others` needs `--with-tree=HEAD` to exclude staged deletions cleanly.
    pub use_with_tree_head: bool,
}

/// Plans untracked index handling for a diff read (`0dc32b5c8b`).
///
/// Reading a single untracked file uses an isolated empty index or `git diff --no-index`,
/// avoiding copying `.git/index` and skipping the `stagedDeletions` read.
#[must_use]
pub const fn isolated_untracked_index_plan(
    only_path: Option<&str>,
    path_arg_count: usize,
) -> UntrackedIndexPlan {
    let isolated = only_path.is_some() && path_arg_count == 1;
    UntrackedIndexPlan {
        isolated_single_file: isolated,
        use_with_tree_head: isolated,
    }
}

/// Returns `true` when `git ls-files --others` can be skipped for a file diff request (`7ef7ba0e3b`).
///
/// If the caller asked only for tracked files, or the file already stats in the real index with a
/// non-new change type (and non-deleted so `ls-files --others` cannot list it), untracked scanning
/// is unnecessary.
#[must_use]
pub const fn should_skip_untracked_listing(
    tracked_only: bool,
    preview_change_type: Option<ReviewDiffChangeKind>,
    indexed_non_deleted: bool,
) -> bool {
    tracked_only
        || (indexed_non_deleted && !matches!(preview_change_type, Some(ReviewDiffChangeKind::New)))
}

/// Builds argv for `git diff --no-index` of an untracked working-tree file without copying `.git/index` (`0dc32b5c8b`).
#[must_use]
pub fn build_untracked_no_index_diff_args(relative_path: &str) -> Vec<String> {
    vec![
        "diff".to_owned(),
        "--no-index".to_owned(),
        "--no-ext-diff".to_owned(),
        "--no-color".to_owned(),
        "--unified=3".to_owned(),
        "--".to_owned(),
        "/dev/null".to_owned(),
        relative_path.to_owned(),
    ]
}

/// Directly renders an untracked file patch against empty content via [`render_file_diff`] (`0dc32b5c8b`).
#[must_use]
pub fn render_untracked_file_diff(
    path: &str,
    new_content: &str,
    is_binary: bool,
    options: DiffQueryOptions,
) -> String {
    render_file_diff(
        DiffFileInput {
            old_path: path,
            new_path: path,
            old_content: "",
            new_content,
            is_binary,
        },
        options,
    )
}

/// Key ensuring a cached `trackedOnly` preview matches the follow-up complete request (`67933a2236`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackedPreviewKey {
    /// Repository working directory.
    pub cwd: String,
    /// Optional explicit base ref.
    pub base_ref: Option<String>,
    /// Optional explicit source filter.
    pub source_kind: Option<ReviewDiffSourceKind>,
    /// Whitespace-ignore setting.
    pub ignore_whitespace: bool,
    /// Metadata-only flag.
    pub metadata_only: bool,
}

/// Cached tracked-file diff output from an initial `trackedOnly` Changes pass (`67933a2236`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedTrackedDiff {
    /// Unified diff text or summary payload.
    pub stdout: String,
    /// Whether the bounded tracked output hit truncation.
    pub stdout_truncated: bool,
    /// Number of tracked files changed.
    pub files_count: usize,
}

/// Stored `trackedOnly` snapshot consumed once by the follow-up complete preview read (`67933a2236`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedPreviewEntry {
    /// Cache matching key.
    pub key: TrackedPreviewKey,
    /// Expiration timestamp in epoch milliseconds.
    pub expires_at_ms: u64,
    /// Resolved base ref string, if any.
    pub base_ref: Option<String>,
    /// Resolved merge base commit OID, if any.
    pub merge_base: Option<String>,
    /// Working-tree vs `HEAD` tracked pass.
    pub dirty: CachedTrackedDiff,
    /// Merge-base vs working tree tracked pass.
    pub base: CachedTrackedDiff,
}

/// Bounded FIFO/single-use store for `trackedOnly` preview handoffs keyed by `completionToken` (`67933a2236`).
#[derive(Debug, Default)]
pub struct TrackedPreviewCache {
    order: VecDeque<String>,
    entries: HashMap<String, TrackedPreviewEntry>,
}

impl TrackedPreviewCache {
    /// Creates an empty [`TrackedPreviewCache`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a tracked-only snapshot under `completion_token`, evicting oldest entries above [`TRACKED_PREVIEW_CAPACITY`].
    pub fn insert(&mut self, completion_token: String, entry: TrackedPreviewEntry) {
        if self.entries.contains_key(&completion_token) {
            self.order.retain(|t| t != &completion_token);
        }
        self.order.push_back(completion_token.clone());
        self.entries.insert(completion_token, entry);
        while self.entries.len() > TRACKED_PREVIEW_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Consumes a cached tracked-only snapshot once if `completion_token` matches `key` and has not expired (`67933a2236`).
    pub fn take_if_valid(
        &mut self,
        completion_token: &str,
        key: &TrackedPreviewKey,
        now_ms: u64,
    ) -> Option<TrackedPreviewEntry> {
        self.order.retain(|t| t != completion_token);
        let entry = self.entries.remove(completion_token)?;
        if &entry.key != key || now_ms > entry.expires_at_ms {
            return None;
        }
        Some(entry)
    }

    /// Returns active entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn merge_base_oid_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^(?:[0-9a-f]{40}|[0-9a-f]{64}|HEAD)$");
        // INFALLIBLE: constant hexadecimal OID / HEAD validator compiled once (`0fd4822887`).
        compiled.expect("merge_base_oid_regex must compile")
    })
}

/// Validates a preview `mergeBase` token before reusing it as a git revision (`0fd4822887`).
///
/// Rejects arbitrary branch names or option-like strings (`-n`, `--all`).
#[must_use]
pub fn is_valid_preview_merge_base(raw: &str) -> bool {
    merge_base_oid_regex().is_match(raw)
}

/// Decision for whether a per-file review request can reuse the preview's `mergeBase` (`0fd4822887`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReusedMergeBaseDecision<'a> {
    /// Validated `mergeBase` revision to pass to `git diff`, if reusable.
    pub merge_base: Option<&'a str>,
    /// `false` when reusing a branch-range `mergeBase` avoids looking up `currentBranch`.
    pub needs_current_branch: bool,
}

/// Reuses a validated `mergeBase` for `BranchRange` file diffs so `git merge-base` and
/// `currentBranch` do not run per file (`0fd4822887`).
#[must_use]
pub fn reuse_preview_merge_base(
    source_kind: Option<ReviewDiffSourceKind>,
    preview_merge_base: Option<&str>,
    has_commit_sha: bool,
) -> ReusedMergeBaseDecision<'_> {
    let reusable = matches!(source_kind, Some(ReviewDiffSourceKind::BranchRange))
        && preview_merge_base.is_some_and(is_valid_preview_merge_base);
    let merge_base = if reusable { preview_merge_base } else { None };
    let needs_current_branch = !has_commit_sha
        && !matches!(source_kind, Some(ReviewDiffSourceKind::WorkingTree))
        && !reusable;
    ReusedMergeBaseDecision {
        merge_base,
        needs_current_branch,
    }
}

/// Renders a single file pair into a unified git diff patch, preserving exact filename whitespace.
#[must_use]
pub fn render_file_diff(input: DiffFileInput<'_>, options: DiffQueryOptions) -> String {
    let old_label = format!("a/{}", quote_git_patch_path(input.old_path));
    let new_label = format!("b/{}", quote_git_patch_path(input.new_path));

    if input.is_binary {
        return format!(
            "diff --git {old_label} {new_label}\nBinary files {old_label} and {new_label} differ\n"
        );
    }

    if !options.ignore_whitespace {
        let diff = TextDiff::from_lines(input.old_content, input.new_content);
        let hunks = diff
            .unified_diff()
            .context_radius(options.context_lines)
            .header(&old_label, &new_label)
            .to_string();
        if hunks.is_empty() {
            return String::new();
        }
        return format!("diff --git {old_label} {new_label}\n{hunks}");
    }

    render_ignore_whitespace_diff(input, &old_label, &new_label, options.context_lines)
}

fn render_ignore_whitespace_diff(
    input: DiffFileInput<'_>,
    old_label: &str,
    new_label: &str,
    context_lines: usize,
) -> String {
    let old_lines: Vec<&str> = split_lines_inclusive(input.old_content);
    let new_lines: Vec<&str> = split_lines_inclusive(input.new_content);

    let old_norm: Vec<String> = old_lines.iter().map(|l| strip_ws(l)).collect();
    let new_norm: Vec<String> = new_lines.iter().map(|l| strip_ws(l)).collect();
    let old_refs: Vec<&str> = old_norm.iter().map(String::as_str).collect();
    let new_refs: Vec<&str> = new_norm.iter().map(String::as_str).collect();

    let ops = capture_diff_slices(Algorithm::Myers, &old_refs, &new_refs);
    let groups = group_diff_ops(ops, context_lines);
    if groups.is_empty() {
        return String::new();
    }

    let mut out = format!("diff --git {old_label} {new_label}\n--- {old_label}\n+++ {new_label}\n");
    for group in groups {
        let Some((first_op, last_op)) = group.first().zip(group.last()) else {
            continue;
        };
        let old_range_start = first_op.old_range().start;
        let old_range_end = last_op.old_range().end;
        let new_range_start = first_op.new_range().start;
        let new_range_end = last_op.new_range().end;
        let old_len = old_range_end - old_range_start;
        let new_len = new_range_end - new_range_start;
        let old_hdr_start = if old_len == 0 {
            old_range_start
        } else {
            old_range_start + 1
        };
        let new_hdr_start = if new_len == 0 {
            new_range_start
        } else {
            new_range_start + 1
        };
        let _ = writeln!(
            out,
            "@@ -{old_hdr_start},{old_len} +{new_hdr_start},{new_len} @@"
        );

        for op in &group {
            for change in op.iter_changes(&old_lines, &new_lines) {
                let prefix = match change.tag() {
                    ChangeTag::Equal => ' ',
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                };
                out.push(prefix);
                out.push_str(change.value());
                if change.missing_newline() {
                    out.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
    out
}

fn split_lines_inclusive(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split_inclusive('\n').collect()
}

fn strip_ws(line: &str) -> String {
    line.chars().filter(|c| !c.is_whitespace()).collect()
}
