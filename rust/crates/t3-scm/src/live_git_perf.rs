//! High-throughput git diff, single-pass status/commit/push/pull, and verified branch renames
//! (`7ef7ba0e3b`, `0dc32b5c8b`, `69228fe179`, `989d7ec882`, `997575be5f`, `d3bb8f8108`, `b775933737`).

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::checkpoints::diff_query::{
    CheckpointDiffQuery, ComputedDiff, DiffFileInput, DiffQueryOptions,
};
pub use crate::git_clone_progress::{
    GitCloneProgressLine, GitCloneStage, build_turn_authorship_log_args,
    collect_multi_parent_shared_paths, git_lfs_download_progress_line, git_supports_remerge_diff,
    is_git_filter_progress_line, parse_git_clone_progress_line, parse_merge_parent_groups,
    scan_lfs_incomplete_bytes,
};
use crate::live_git_cli::{RawProcessOutput, execute_raw_bytes};
use crate::live_http::LiveClientError;

fn branch_renamed_config_failed_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled =
            Regex::new(r"(?im)^fatal: branch is renamed, but update of config-file failed");
        // INFALLIBLE: constant git branch -m stderr matcher (`#40359`).
        compiled.expect("branch_renamed_config_failed_regex must compile")
    })
}

/// Computes a unified diff against `base_ref` in a real `git` repository via [`CheckpointDiffQuery`].
///
/// Compares tracked changes (`git diff --no-renames -z --name-status <base_ref>`) and untracked
/// files (`git ls-files --others --exclude-standard -z`) directly from disk without copying `.git/index`.
pub async fn compute_live_git_diff(
    repo_dir: &Path,
    base_ref: &str,
    opts: &DiffQueryOptions,
    base_env: &[(String, String)],
) -> Result<ComputedDiff, LiveClientError> {
    compute_live_git_diff_with_mode(repo_dir, base_ref, opts, false, base_env).await
}

/// Computes a unified diff with optional `tracked_only` bypass skipping `ls-files --others` (`7ef7ba0e3b`).
pub async fn compute_live_git_diff_with_mode(
    repo_dir: &Path,
    base_ref: &str,
    opts: &DiffQueryOptions,
    tracked_only: bool,
    base_env: &[(String, String)],
) -> Result<ComputedDiff, LiveClientError> {
    let clean_ref = base_ref.trim();
    if clean_ref.is_empty() || clean_ref.starts_with('-') || clean_ref.contains('\0') {
        return Err(LiveClientError::InvalidInput(format!(
            "unsafe git base_ref `{base_ref}`"
        )));
    }

    let diff_raw = execute_git_checked(
        repo_dir,
        &["diff", "--no-renames", "-z", "--name-status", clean_ref],
        base_env,
    )
    .await?;
    let mut tracked_entries = parse_name_status_z(&diff_raw);

    if !tracked_only {
        let untracked_raw = execute_git_checked(
            repo_dir,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            base_env,
        )
        .await?;
        for slice in untracked_raw.split(|&b| b == 0) {
            if !slice.is_empty()
                && let Ok(path) = std::str::from_utf8(slice)
                && !tracked_entries.iter().any(|(_, p)| p == path)
            {
                tracked_entries.push(('A', path.to_owned()));
            }
        }
    }

    tracked_entries.sort_by(|a, b| a.1.cmp(&b.1));

    let mut buffers: Vec<OwnedDiffPair> = Vec::with_capacity(tracked_entries.len());
    for (status, path) in tracked_entries {
        let old_bytes = if status == 'A' {
            Vec::new()
        } else {
            read_git_blob(repo_dir, clean_ref, &path, base_env).await?
        };
        let new_bytes = if status == 'D' {
            Vec::new()
        } else {
            read_working_file_or_empty(&repo_dir.join(&path)).await?
        };
        buffers.push(OwnedDiffPair::from_bytes(path, old_bytes, new_bytes));
    }

    let inputs: Vec<DiffFileInput<'_>> = buffers.iter().map(OwnedDiffPair::as_input).collect();
    Ok(CheckpointDiffQuery::compute_files(&inputs, *opts))
}

/// Detects git's `"fatal: branch is renamed, but update of config-file failed"` stderr (`#40359`, `b775933737`).
#[must_use]
pub fn is_branch_renamed_config_update_error(stderr: &str) -> bool {
    branch_renamed_config_failed_regex().is_match(stderr)
}

/// Renames `old_branch` to `new_branch`, accepting config-file update failure only after
/// verifying `refs/heads/<new_branch>` exists and `refs/heads/<old_branch>` is absent (`#40359`, `b775933737`).
pub async fn rename_branch_verified(
    repo_dir: &Path,
    old_branch: &str,
    new_branch: &str,
    base_env: &[(String, String)],
) -> Result<(), LiveClientError> {
    let old = validate_branch_arg(old_branch)?;
    let new = validate_branch_arg(new_branch)?;
    let raw = execute_git_raw(repo_dir, &["branch", "-m", old, new], base_env).await?;
    if raw.success {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_owned();
    if is_branch_renamed_config_update_error(&stderr)
        && verify_branch_ref_exists(repo_dir, new, base_env).await
        && !verify_branch_ref_exists(repo_dir, old, base_env).await
    {
        return Ok(());
    }
    Err(LiveClientError::CliExit {
        program: "git".to_owned(),
        code: raw.status_code,
        stderr,
    })
}

async fn verify_branch_ref_exists(
    repo_dir: &Path,
    branch: &str,
    base_env: &[(String, String)],
) -> bool {
    let full_ref = format!("refs/heads/{branch}");
    execute_git_raw(
        repo_dir,
        &["show-ref", "--verify", "--quiet", &full_ref],
        base_env,
    )
    .await
    .is_ok_and(|out| out.success)
}

fn validate_branch_arg(name: &str) -> Result<&str, LiveClientError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.contains(['\0', '\r', '\n']) {
        return Err(LiveClientError::InvalidInput(format!(
            "unsafe git branch name `{name}`"
        )));
    }
    Ok(trimmed)
}

/// Options controlling remote status lookups (`989d7ec882`: push skips extra base-branch ahead check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteStatusQueryOptions {
    /// Refresh upstream tracking ref before computing ahead/behind counts.
    pub refresh_upstream: bool,
    /// Compute ahead-of-default-branch count (`false` on push/commit-and-push).
    pub include_ahead_of_default: bool,
}

impl Default for RemoteStatusQueryOptions {
    fn default() -> Self {
        Self {
            refresh_upstream: true,
            include_ahead_of_default: true,
        }
    }
}

/// Returns whether staging with an unverified message requires a scratch-index check first (`69228fe179`).
#[must_use]
pub const fn should_probe_scratch_index(unverified: bool, index_matches_head: bool) -> bool {
    unverified && !index_matches_head
}

/// Checks if any file changed in `git diff --cached --no-renames --name-only -z` belongs to `selected` (`69228fe179`).
#[must_use]
pub fn has_selected_staged_changes<S: std::hash::BuildHasher>(
    diff_cached_names_z: &[u8],
    selected: &HashSet<String, S>,
) -> bool {
    diff_cached_names_z
        .split(|&b| b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .any(|path| !path.is_empty() && selected.contains(path))
}

/// Resolves the pull upstream string for an unborn branch from `branch.<name>.{remote,merge}` (`997575be5f`).
#[must_use]
pub fn resolve_pull_upstream_from_config(
    configured_remote: Option<&str>,
    merge_ref: Option<&str>,
) -> Option<String> {
    let remote = configured_remote?.trim();
    let merge = merge_ref?
        .trim()
        .strip_prefix("refs/heads/")
        .or(merge_ref.map(str::trim))?;
    if remote.is_empty() || merge.is_empty() {
        return None;
    }
    Some(if remote == "." {
        merge.to_owned()
    } else {
        format!("{remote}/{merge}")
    })
}

/// Controls status refresh behavior after deleting a thread's worktree (`d3bb8f8108`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeDeleteRefreshPlan {
    /// Never trigger a duplicate project-root refresh; rely on the client's single follow-up refresh.
    pub skip_duplicate_project_root_refresh: bool,
}

/// Returns the canonical single-pass worktree deletion refresh plan (`d3bb8f8108`).
#[must_use]
pub const fn worktree_delete_refresh_plan() -> WorktreeDeleteRefreshPlan {
    WorktreeDeleteRefreshPlan {
        skip_duplicate_project_root_refresh: true,
    }
}

async fn read_working_file_or_empty(path: &Path) -> Result<Vec<u8>, LiveClientError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(LiveClientError::InvalidInput(format!(
            "failed reading `{}`: {err}",
            path.display()
        ))),
    }
}

struct OwnedDiffPair {
    path: String,
    old_text: String,
    new_text: String,
    is_binary: bool,
}

impl OwnedDiffPair {
    fn as_input(&self) -> DiffFileInput<'_> {
        DiffFileInput {
            old_path: &self.path,
            new_path: &self.path,
            old_content: &self.old_text,
            new_content: &self.new_text,
            is_binary: self.is_binary,
        }
    }
    fn from_bytes(path: String, old_bytes: Vec<u8>, new_bytes: Vec<u8>) -> Self {
        match (String::from_utf8(old_bytes), String::from_utf8(new_bytes)) {
            (Ok(old_text), Ok(new_text))
                if !old_text.contains('\0') && !new_text.contains('\0') =>
            {
                Self {
                    path,
                    old_text,
                    new_text,
                    is_binary: false,
                }
            }
            _ => Self {
                path,
                old_text: String::new(),
                new_text: String::new(),
                is_binary: true,
            },
        }
    }
}

pub(crate) async fn execute_git_raw(
    repo_dir: &Path,
    args: &[&str],
    base_env: &[(String, String)],
) -> Result<RawProcessOutput, LiveClientError> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    execute_raw_bytes("git", &owned, base_env, &[], None, Some(repo_dir)).await
}

async fn execute_git_checked(
    repo_dir: &Path,
    args: &[&str],
    base_env: &[(String, String)],
) -> Result<Vec<u8>, LiveClientError> {
    let raw = execute_git_raw(repo_dir, args, base_env).await?;
    if !raw.success {
        return Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: raw.status_code,
            stderr: String::from_utf8_lossy(&raw.stderr).trim().to_owned(),
        });
    }
    Ok(raw.stdout)
}

async fn read_git_blob(
    repo_dir: &Path,
    base_ref: &str,
    path: &str,
    base_env: &[(String, String)],
) -> Result<Vec<u8>, LiveClientError> {
    let args = ["show".to_owned(), format!("{base_ref}:{path}")];
    let raw = execute_raw_bytes("git", &args, base_env, &[], None, Some(repo_dir)).await?;
    if raw.success {
        return Ok(raw.stdout);
    }
    let stderr = String::from_utf8_lossy(&raw.stderr);
    if stderr.contains("does not exist in") || stderr.contains("exists on disk, but not in") {
        Ok(Vec::new())
    } else {
        Err(LiveClientError::CliExit {
            program: "git".to_owned(),
            code: raw.status_code,
            stderr: stderr.trim().to_owned(),
        })
    }
}

fn parse_name_status_z(raw: &[u8]) -> Vec<(char, String)> {
    let tokens: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < tokens.len() {
        let status_bytes = tokens[i];
        let path_bytes = tokens[i + 1];
        i += 2;
        if status_bytes.is_empty() || path_bytes.is_empty() {
            continue;
        }
        let status = char::from(status_bytes[0]);
        if let Ok(path) = std::str::from_utf8(path_bytes) {
            out.push((status, path.to_owned()));
        }
    }
    out
}
