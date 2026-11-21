//! `similar`-backed unified diff computation, merging, filename whitespace preservation, and truncation.

use crate::checkpoints::diff_patch_fast::render_file_diff;

/// Default unified diff output ceiling in bytes (`10 MB`), matching T3 Code server limits.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 10_000_000;

/// Maximum combined character budget per git `pathspec` command batch (`24_000`).
pub const MAX_PATHSPEC_CHARS: usize = 24_000;

/// Marker appended when a unified diff exceeds `max_output_bytes`.
pub const DIFF_TRUNCATED_MARKER: &str = "\n\n[truncated]";

/// Single file pair input for [`CheckpointDiffQuery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffFileInput<'a> {
    /// Original repository-relative path (whitespace preserved).
    pub old_path: &'a str,
    /// Target repository-relative path (whitespace preserved).
    pub new_path: &'a str,
    /// Old file UTF-8 text content.
    pub old_content: &'a str,
    /// New file UTF-8 text content.
    pub new_content: &'a str,
    /// When `true`, emit a binary placeholder notice instead of text hunks.
    pub is_binary: bool,
}

/// Formatting and safety options for diff computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffQueryOptions {
    /// Number of unchanged context lines around edits (default `3`).
    pub context_lines: usize,
    /// Ignore whitespace differences inside line content (never strips filename whitespace).
    pub ignore_whitespace: bool,
    /// Output byte cap after which [`DIFF_TRUNCATED_MARKER`] is appended.
    pub max_output_bytes: usize,
}

impl Default for DiffQueryOptions {
    fn default() -> Self {
        Self {
            context_lines: 3,
            ignore_whitespace: false,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// Aggregated diff output and summary statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedDiff {
    /// Combined unified diff string.
    pub diff: String,
    /// `true` if output exceeded `max_output_bytes` and was cut at a `UTF-8` boundary.
    pub truncated: bool,
    /// Number of files containing changes or binary markers.
    pub files_changed: usize,
    /// Count of `+` diff lines across emitted hunks.
    pub insertions: usize,
    /// Count of `-` diff lines across emitted hunks.
    pub deletions: usize,
}

/// Entry parsed from `git diff --numstat -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumstatEntry {
    /// Added line count, or `None` for binary files (`-`).
    pub insertions: Option<u64>,
    /// Deleted line count, or `None` for binary files (`-`).
    pub deletions: Option<u64>,
    /// Old path for renames, if different from `path`.
    pub old_path: Option<String>,
    /// Current path with whitespace intact.
    pub path: String,
}

/// Computes and merges unified diffs via `similar` while preserving filename whitespace.
///
/// ```
/// use t3_scm::checkpoints::diff_query::{CheckpointDiffQuery, DiffFileInput, DiffQueryOptions};
///
/// let file = DiffFileInput {
///     old_path: "  spaced dir/hello world .txt ",
///     new_path: "  spaced dir/hello world .txt ",
///     old_content: "alpha\nbeta\n",
///     new_content: "alpha\ngamma\n",
///     is_binary: false,
/// };
/// let result = CheckpointDiffQuery::compute_files(&[file], DiffQueryOptions::default());
/// assert!(result.diff.contains("--- a/  spaced dir/hello world .txt "));
/// assert!(result.diff.contains("+gamma"));
/// assert_eq!(result.files_changed, 1);
/// assert_eq!(result.insertions, 1);
/// assert_eq!(result.deletions, 1);
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct CheckpointDiffQuery;

impl CheckpointDiffQuery {
    /// Computes unified diff for a slice of file inputs according to `options`.
    #[must_use]
    pub fn compute_files(files: &[DiffFileInput<'_>], options: DiffQueryOptions) -> ComputedDiff {
        let mut patches = Vec::with_capacity(files.len());
        for file in files {
            let patch = render_file_diff(*file, options);
            if !patch.is_empty() {
                patches.push(patch);
            }
        }
        let refs: Vec<&str> = patches.iter().map(String::as_str).collect();
        merge_unified_diffs(&refs, options.max_output_bytes)
    }
}

/// Merges pre-rendered per-file unified diffs, gathers statistics, and applies `max_output_bytes`.
#[must_use]
pub fn merge_unified_diffs(patches: &[&str], max_output_bytes: usize) -> ComputedDiff {
    let mut files_changed = 0usize;
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut merged = String::new();

    for patch in patches {
        let trimmed_end = patch.trim_end_matches('\n');
        if trimmed_end.is_empty() {
            continue;
        }
        files_changed += 1;
        for line in trimmed_end.lines() {
            if line.starts_with('+') && !line.starts_with("+++") {
                insertions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                deletions += 1;
            }
        }
        if !merged.is_empty() {
            merged.push('\n');
        }
        merged.push_str(trimmed_end);
        merged.push('\n');
    }

    let (diff, truncated) = truncate_diff_utf8(&merged, max_output_bytes);
    ComputedDiff {
        diff,
        truncated,
        files_changed,
        insertions,
        deletions,
    }
}

/// Truncates `diff` to fit within `max_bytes` on a `UTF-8` char boundary and appends [`DIFF_TRUNCATED_MARKER`].
#[must_use]
pub fn truncate_diff_utf8(diff: &str, max_bytes: usize) -> (String, bool) {
    if diff.len() <= max_bytes {
        return (diff.to_owned(), false);
    }
    if max_bytes <= DIFF_TRUNCATED_MARKER.len() {
        let mut idx = max_bytes;
        while idx > 0 && !DIFF_TRUNCATED_MARKER.is_char_boundary(idx) {
            idx -= 1;
        }
        return (DIFF_TRUNCATED_MARKER[..idx].to_owned(), true);
    }
    let body_budget = max_bytes - DIFF_TRUNCATED_MARKER.len();
    let mut cut = body_budget.min(diff.len());
    while cut > 0 && !diff.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(cut + DIFF_TRUNCATED_MARKER.len());
    out.push_str(&diff[..cut]);
    out.push_str(DIFF_TRUNCATED_MARKER);
    (out, true)
}

/// Batches pathspecs into chunks whose total string size stays within `max_chars`.
#[must_use]
pub fn batch_pathspecs(paths: &[&str], max_chars: usize) -> Vec<Vec<String>> {
    let budget = max_chars.max(1);
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_chars = 0usize;

    for path in paths {
        let cost = path.len() + 1;
        if !current.is_empty() && current_chars + cost > budget {
            batches.push(current);
            current = Vec::new();
            current_chars = 0;
        }
        current.push((*path).to_owned());
        current_chars += cost;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Parses `git diff --numstat -z` NUL-delimited output while preserving filename whitespace.
#[must_use]
pub fn parse_numstat_z(raw: &[u8]) -> Vec<NumstatEntry> {
    let fields: Vec<&str> = raw
        .split(|&b| b == 0)
        .filter_map(|slice| std::str::from_utf8(slice).ok())
        .collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < fields.len() {
        let entry = fields[i];
        if entry.is_empty() {
            i += 1;
            continue;
        }
        let mut cols = entry.splitn(3, '\t');
        let (Some(add_str), Some(del_str), Some(path_part)) =
            (cols.next(), cols.next(), cols.next())
        else {
            i += 1;
            continue;
        };
        let insertions = add_str.parse::<u64>().ok();
        let deletions = del_str.parse::<u64>().ok();
        if path_part.is_empty() && i + 2 < fields.len() {
            out.push(NumstatEntry {
                insertions,
                deletions,
                old_path: Some(fields[i + 1].to_owned()),
                path: fields[i + 2].to_owned(),
            });
            i += 3;
        } else {
            out.push(NumstatEntry {
                insertions,
                deletions,
                old_path: None,
                path: path_part.to_owned(),
            });
            i += 1;
        }
    }
    out
}
