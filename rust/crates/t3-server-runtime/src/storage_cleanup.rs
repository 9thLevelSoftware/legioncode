//! Storage cleanup for detached worktrees: dangling gitlinks (`#40119`), fork-merged PRs,
//! and running full `git status` only on removable candidates (`8e2949e0ef`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn split_owner_repo(slug: &str) -> Option<(String, String)> {
    let trimmed = slug.trim().trim_matches('/');
    let (owner, name) = trimmed.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_ascii_lowercase(), name.to_ascii_lowercase()))
}

/// Outcome produced when evaluating a worktree cleanup candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCleanupEntry {
    pub path: PathBuf,
    pub removed: bool,
    pub reason: String,
}

/// Metadata known about a worktree's branch and pull requests before deciding removal.
#[derive(Debug, Clone, Default)]
pub struct WorktreeCleanupCandidate {
    /// Worktree root directory on disk.
    pub path: PathBuf,
    /// Whether the worktree still belongs to a live thread (if true, immediately kept).
    pub in_use_by_thread: bool,
    /// Branch checked out in this worktree (`None` if detached/missing).
    pub branch: Option<String>,
    /// Merged into upstream base or directly by PR state.
    pub merged_upstream: bool,
    /// Cross-repository (fork) PR metadata (`owner/repo`, `state == "merged"`).
    pub head_repository_name_with_owner: Option<String>,
    /// Pull request states matching `branch` when `include_head_repository` is inspected.
    pub fork_pull_requests: Vec<ForkPullRequestRef>,
    /// Whether `worktree_on_merge` cleanup is enabled.
    pub worktree_on_merge: bool,
}

/// Lightweight PR summary used for `includeHeadRepository` fork-merged detection (`1de7eaecec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkPullRequestRef {
    pub number: u64,
    pub state: String,
    pub repository_name_with_owner: String,
    pub head_branch: String,
    pub updated_at: String,
}

/// Runner abstraction so tests can count `git status` invocations directly (`8e2949e0ef`).
pub trait CleanupGitStatusRunner {
    /// Returns `true` when `worktree_path` has uncommitted / untracked local changes.
    fn has_local_changes(&mut self, worktree_path: &Path) -> Result<bool, String>;
}

/// Real `git status --porcelain` runner for production storage cleanup.
#[derive(Debug, Default)]
pub struct OsCleanupGitStatusRunner;

impl CleanupGitStatusRunner for OsCleanupGitStatusRunner {
    fn has_local_changes(&mut self, worktree_path: &Path) -> Result<bool, String> {
        let output =
            crate::git_checkpoints_cli::run_git(worktree_path, &["status", "--porcelain"], &[])
                .map_err(|e| e.to_string())?;
        Ok(!String::from_utf8_lossy(&output).trim().is_empty())
    }
}

/// Returns `true` if `git_dir` looks like a UNC or network path (`\\...` or `//...`) where
/// statting it on Windows could trigger credentialed SMB prompts (`#40119`).
pub fn is_network_path(path: &str) -> bool {
    path.starts_with(r"\\") || path.starts_with("//")
}

/// Returns `true` when `directory.join(name)` is a regular `.git` gitfile whose `gitdir: <target>`
/// target does not exist on disk (`#40119`). Unreadable or network-path gitfiles return `false`.
pub fn is_dangling_gitfile(directory: &Path, name: &str) -> bool {
    let git_path = directory.join(name);
    let Ok(meta) = std::fs::symlink_metadata(&git_path) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(raw) = std::fs::read_to_string(&git_path) else {
        return false;
    };
    // Strip only trailing CR/LF so intentional trailing spaces like `git ` stay significant.
    let contents = raw.trim_end_matches(['\r', '\n']);
    let Some(git_dir) = contents.strip_prefix("gitdir: ") else {
        return false;
    };
    if git_dir.is_empty() || is_network_path(git_dir) {
        return false;
    }
    let target = if Path::new(git_dir).is_absolute() {
        PathBuf::from(git_dir)
    } else {
        directory.join(git_dir)
    };
    !target.exists()
}

/// Inspects a worktree directory tree for non-dangling nested `.git` repositories (`#40119`).
///
/// Skipping nested `.git` gitfiles whose target is dangling prevents third-party packages
/// (such as test fixtures containing `.git -> ../.git/modules/x`) from blocking worktree removal.
pub fn contains_non_dangling_nested_git_repo(worktree_root: &Path) -> bool {
    fn walk(dir: &Path, is_root: bool) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let name_os = entry.file_name();
            let name = name_os.to_string_lossy();
            let path = entry.path();
            if !is_root && name.eq_ignore_ascii_case(".git") {
                if !is_dangling_gitfile(dir, &name) {
                    return true;
                }
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() && !ft.is_symlink() && walk(&path, false) {
                return true;
            }
        }
        false
    }
    walk(worktree_root, true)
}

/// Finds the newest pull request merged inside a fork's own repository when upstream queries missed
/// cross-repository PRs (`include_head_repository`, `1de7eaecec`).
pub fn find_merged_pr_in_head_repository(
    candidate: &WorktreeCleanupCandidate,
    include_head_repository: bool,
) -> Option<&ForkPullRequestRef> {
    if !include_head_repository {
        return None;
    }
    let head_repo = candidate.head_repository_name_with_owner.as_deref()?;
    let (owner, name) = split_owner_repo(head_repo)?;
    let branch = candidate.branch.as_deref()?;
    candidate
        .fork_pull_requests
        .iter()
        .filter(|pr| {
            pr.state.eq_ignore_ascii_case("merged")
                && pr.head_branch == branch
                && split_owner_repo(&pr.repository_name_with_owner)
                    == Some((owner.clone(), name.clone()))
        })
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
}

/// Evaluates worktree cleanup candidates, running expensive `git status` ONLY on candidates that
/// are otherwise removable (`8e2949e0ef`), checking `include_head_repository` fork-merged PRs
/// (`1de7eaecec`), and skipping dangling gitlinks (`#40119`).
pub fn evaluate_and_clean_worktrees<R: CleanupGitStatusRunner>(
    candidates: &[WorktreeCleanupCandidate],
    runner: &mut R,
    dry_run: bool,
) -> Vec<WorktreeCleanupEntry> {
    let mut outcomes = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if candidate.in_use_by_thread {
            outcomes.push(WorktreeCleanupEntry {
                path: candidate.path.clone(),
                removed: false,
                reason: "Worktree is still attached to an active thread".to_owned(),
            });
            continue;
        }
        if !candidate.worktree_on_merge {
            outcomes.push(WorktreeCleanupEntry {
                path: candidate.path.clone(),
                removed: false,
                reason: "Worktree cleanup on merge is disabled".to_owned(),
            });
            continue;
        }
        let merged = candidate.merged_upstream
            || find_merged_pr_in_head_repository(candidate, true).is_some();
        if !merged {
            outcomes.push(WorktreeCleanupEntry {
                path: candidate.path.clone(),
                removed: false,
                reason: "Branch or pull request is not merged".to_owned(),
            });
            continue;
        }
        if contains_non_dangling_nested_git_repo(&candidate.path) {
            outcomes.push(WorktreeCleanupEntry {
                path: candidate.path.clone(),
                removed: false,
                reason: "Contains a nested Git repository".to_owned(),
            });
            continue;
        }

        // Only removable candidates pay the cost of walking the checkout with `git status`.
        match runner.has_local_changes(&candidate.path) {
            Ok(true) => {
                outcomes.push(WorktreeCleanupEntry {
                    path: candidate.path.clone(),
                    removed: false,
                    reason: "Worktree has uncommitted local changes".to_owned(),
                });
                continue;
            }
            Ok(false) => {}
            Err(err) => {
                outcomes.push(WorktreeCleanupEntry {
                    path: candidate.path.clone(),
                    removed: false,
                    reason: format!("Failed to inspect git status: {err}"),
                });
                continue;
            }
        }

        if !dry_run && let Err(err) = std::fs::remove_dir_all(&candidate.path) {
            outcomes.push(WorktreeCleanupEntry {
                path: candidate.path.clone(),
                removed: false,
                reason: format!("Failed to remove worktree: {err}"),
            });
            continue;
        }

        outcomes.push(WorktreeCleanupEntry {
            path: candidate.path.clone(),
            removed: true,
            reason: "Removed merged worktree".to_owned(),
        });
    }
    outcomes
}
