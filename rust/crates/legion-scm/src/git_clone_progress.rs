//! Git clone & Git LFS progress parser (`302b28c4ec`) and git `< 2.36` turn authorship compat (`c7157d376d`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

fn git_version_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^git version (\d+)\.(\d+)");
        // INFALLIBLE: constant `git version` output matcher (`c7157d376d`).
        compiled.expect("git_version_regex must compile")
    })
}

fn clone_percent_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r":\s*(\d{1,3})%");
        // INFALLIBLE: constant git progress percentage matcher (`302b28c4ec`).
        compiled.expect("clone_percent_regex must compile")
    })
}

/// Returns `true` when `git --version` output is `>= 2.36`, where `--diff-merges=remerge` exists (`c7157d376d`).
#[must_use]
pub fn git_supports_remerge_diff(version_output: &str) -> bool {
    let Some(caps) = git_version_regex().captures(version_output.trim()) else {
        return false;
    };
    let major = caps
        .get(1)
        .and_then(|m| m.as_str().parse::<u32>().ok())
        .unwrap_or(0);
    let minor = caps
        .get(2)
        .and_then(|m| m.as_str().parse::<u32>().ok())
        .unwrap_or(0);
    major > 2 || (major == 2 && minor >= 36)
}

/// Builds turn-authorship `git log` args for either modern (`--diff-merges=remerge`) or legacy (`--cc`) git (`c7157d376d`).
#[must_use]
pub fn build_turn_authorship_log_args(remerge_supported: bool, range: &str) -> Vec<String> {
    vec![
        "log".to_owned(),
        range.to_owned(),
        "--format=".to_owned(),
        if remerge_supported {
            "--diff-merges=remerge"
        } else {
            "--cc"
        }
        .to_owned(),
    ]
}

/// Parses `git rev-list --merges --parents` lines into vectors of parent commits per merge commit (`c7157d376d`).
#[must_use]
pub fn parse_merge_parent_groups(merges_parents_output: &str) -> Vec<Vec<String>> {
    merges_parents_output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _merge_commit = parts.next()?;
            let parents: Vec<String> = parts.map(str::to_owned).collect();
            (!parents.is_empty()).then_some(parents)
        })
        .collect()
}

/// Given per-parent diff file lists from legacy Git `< 2.36` fallback, returns sorted paths modified by
/// more than one merge parent (`c7157d376d`).
#[must_use]
pub fn collect_multi_parent_shared_paths(parent_diff_lists: &[Vec<String>]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for list in parent_diff_lists {
        let unique: HashSet<&str> = list
            .iter()
            .map(String::as_str)
            .filter(|p| !p.is_empty())
            .collect();
        for path in unique {
            *counts.entry(path.to_owned()).or_insert(0) += 1;
        }
    }
    let mut out: Vec<String> = counts
        .into_iter()
        .filter_map(|(path, count)| (count > 1).then_some(path))
        .collect();
    out.sort();
    out
}

/// Stage reported while cloning or filtering a repository (`302b28c4ec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitCloneStage {
    /// Enumerating objects.
    Enumerating,
    /// Counting objects.
    Counting,
    /// Compressing objects.
    Compressing,
    /// Receiving packfile objects.
    Receiving,
    /// Resolving packfile deltas.
    Resolving,
    /// Checking out / updating working-tree files and Git LFS filters.
    Checkout,
}

/// Structured clone progress update emitted from `git clone --progress` or Git LFS scan (`302b28c4ec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCloneProgressLine {
    /// Normalized clone phase.
    pub stage: GitCloneStage,
    /// Optional completion percentage (`0..=100`).
    pub percent: Option<u8>,
    /// Optional bandwidth/object count or LFS byte summary.
    pub detail: Option<String>,
}

/// Parses a line of `git clone --progress` stderr, including `Filtering content` Git LFS steps (`302b28c4ec`).
#[must_use]
pub fn parse_git_clone_progress_line(line: &str) -> Option<GitCloneProgressLine> {
    const PREFIXES: &[(&str, GitCloneStage)] = &[
        ("Enumerating objects", GitCloneStage::Enumerating),
        ("Counting objects", GitCloneStage::Counting),
        ("Compressing objects", GitCloneStage::Compressing),
        ("Receiving objects", GitCloneStage::Receiving),
        ("Resolving deltas", GitCloneStage::Resolving),
        ("Updating files", GitCloneStage::Checkout),
        ("Checking out files", GitCloneStage::Checkout),
        ("Filtering content", GitCloneStage::Checkout),
    ];
    let trimmed = line.trim();
    let (prefix, stage) = PREFIXES
        .iter()
        .find(|(p, _)| trimmed.starts_with(p))
        .copied()?;
    let rest = trimmed[prefix.len()..].trim();
    let percent = clone_percent_regex()
        .captures(trimmed)
        .and_then(|c| c.get(1)?.as_str().parse::<u8>().ok());
    let detail = rest
        .split_once(',')
        .map(|(_, d)| d.trim())
        .filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case("done."))
        .map(str::to_owned);
    Some(GitCloneProgressLine {
        stage,
        percent,
        detail,
    })
}

/// Returns `true` when git stderr indicates the Git LFS `Filtering content` phase (`302b28c4ec`).
#[must_use]
pub fn is_git_filter_progress_line(line: &str) -> bool {
    line.trim_start().starts_with("Filtering content")
}

/// Formats active Git LFS download byte progress when git prints nothing during large LFS transfers (`302b28c4ec`).
#[must_use]
pub fn git_lfs_download_progress_line(
    partial_bytes: u64,
    percent: Option<u8>,
) -> GitCloneProgressLine {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    #[expect(
        clippy::cast_precision_loss,
        reason = "formatting human-readable MiB/GiB progress strings allows 53-bit float precision"
    )]
    let bytes_f = partial_bytes as f64;
    let size = if partial_bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB", bytes_f / GIB)
    } else {
        format!("{:.1} MiB", bytes_f / MIB)
    };
    GitCloneProgressLine {
        stage: GitCloneStage::Checkout,
        percent,
        detail: Some(format!("Downloading Git LFS objects: {size} in progress")),
    }
}

/// Sums byte sizes inside `<repo_dir>/.git/lfs/incomplete` to detect live LFS object downloads (`302b28c4ec`).
pub async fn scan_lfs_incomplete_bytes(repo_dir: &Path) -> u64 {
    let dir = repo_dir.join(".git").join("lfs").join("incomplete");
    let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
        return 0;
    };
    let mut total = 0u64;
    while let Ok(Some(entry)) = rd.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            total = total.saturating_add(meta.len());
        }
    }
    total
}
