//! Multi-repository thread tracking, pinning, and checkout resolution (`60863edc41` / #35925).

use std::collections::BTreeSet;

use legion_contracts::ids::CheckpointScopeId;
use legion_contracts::workspace_folders::{
    PROJECT_TRACKED_FOLDERS_MAX, ProjectTrackedFolder, TaskFolder, WorkspaceFolder,
    WorkspaceFolderState, WorkspaceScannedFolder,
};
use serde::{Deserialize, Serialize};

/// A Git repository tracked or changed in a thread (`#35925`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRepositoryRef {
    /// Optional checkpoint scope associated with changed files in this repo.
    pub scope_id: Option<CheckpointScopeId>,
    /// Canonical project repository path.
    pub repository_path: String,
    /// Display repository name.
    pub name: String,
    /// Working checkout path for this thread (`worktree_path` when isolated, else `repository_path`).
    pub cwd: String,
}

/// Pinned repository entry stored on a thread view (`#35925`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedThreadRepository {
    /// Canonical project repository path.
    pub repository_path: String,
    /// Display name shown when pinned.
    pub name: String,
}

/// Origin classifying why a repository section is displayed in the thread review drawer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositorySectionOrigin {
    /// Repository has uncommitted/turn changes.
    Changed,
    /// Repository was explicitly pinned by the user.
    Pinned,
    /// Repository has an isolated checkout inside the thread's task folder.
    Worktree,
}

/// One repository entry shown in a multi-repo diff/review sidebar (`#35925`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRepositorySection {
    /// Canonical key (`repository_path`).
    pub key: String,
    /// Working directory (`worktree_path` or `repository_path`).
    pub cwd: String,
    /// Display name.
    pub name: String,
    /// Optional checkpoint scope ID if changed in turn diffs.
    pub scope_id: Option<CheckpointScopeId>,
    /// Why this repository appears in the visible list.
    pub origin: RepositorySectionOrigin,
}

/// Computed visible repository sections (`#35925`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRepositorySections {
    /// Ordered repository sections to render (`changed -> worktree/pinned`).
    pub visible: Vec<ThreadRepositorySection>,
    /// Count of additional unpinned, unchanged repositories hidden when `show_all` is `false`.
    pub hidden_count: usize,
}

/// Suggests untracked scanned repositories that can be added up to [`PROJECT_TRACKED_FOLDERS_MAX`].
#[must_use]
pub fn suggested_tracked_folders(
    existing: &[ProjectTrackedFolder],
    scanned: &[WorkspaceScannedFolder],
    project_root: Option<&str>,
) -> Vec<ProjectTrackedFolder> {
    let mut seen: BTreeSet<String> = existing.iter().map(|f| normalize_path(&f.path)).collect();
    if let Some(root) = project_root {
        seen.insert(normalize_path(root));
    }
    let remaining_cap = PROJECT_TRACKED_FOLDERS_MAX.saturating_sub(existing.len());
    let mut out = Vec::new();
    for item in scanned {
        if out.len() >= remaining_cap {
            break;
        }
        if !item.is_git_repository {
            continue;
        }
        let norm = normalize_path(&item.path);
        if seen.insert(norm) {
            out.push(ProjectTrackedFolder {
                path: item.path.clone(),
                name: item.name.clone(),
                discovered: Some(true),
            });
        }
    }
    out
}

/// Appends new tracked folders without duplicating normalized paths or exceeding
/// [`PROJECT_TRACKED_FOLDERS_MAX`].
#[must_use]
pub fn append_tracked_folders(
    existing: &[ProjectTrackedFolder],
    additions: &[ProjectTrackedFolder],
) -> Vec<ProjectTrackedFolder> {
    let mut out = existing.to_vec();
    let mut seen: BTreeSet<String> = existing.iter().map(|f| normalize_path(&f.path)).collect();
    for folder in additions {
        if out.len() >= PROJECT_TRACKED_FOLDERS_MAX {
            break;
        }
        if seen.insert(normalize_path(&folder.path)) {
            out.push(folder.clone());
        }
    }
    out
}

/// Pins a repository into `pinned` if not already present (idempotent by normalized path, `#35925`).
#[must_use]
pub fn pin_thread_repository(
    pinned: &[PinnedThreadRepository],
    repo: PinnedThreadRepository,
) -> Vec<PinnedThreadRepository> {
    let key = normalize_path(&repo.repository_path);
    if pinned
        .iter()
        .any(|p| normalize_path(&p.repository_path) == key)
    {
        return pinned.to_vec();
    }
    let mut out = pinned.to_vec();
    out.push(repo);
    out
}

/// Unpins a repository matching `repository_path` (reverse state of [`pin_thread_repository`], `#35925`).
#[must_use]
pub fn unpin_thread_repository(
    pinned: &[PinnedThreadRepository],
    repository_path: &str,
) -> Vec<PinnedThreadRepository> {
    let key = normalize_path(repository_path);
    pinned
        .iter()
        .filter(|p| normalize_path(&p.repository_path) != key)
        .cloned()
        .collect()
}

/// Resolves all Git checkouts usable by a thread: isolated task-folder worktrees when present,
/// or project Git folders outside worktrees (`#35925`).
#[must_use]
pub fn thread_git_repositories(
    changed: &[ThreadRepositoryRef],
    task_folder: Option<&TaskFolder>,
    folders: &[WorkspaceFolder],
) -> Vec<ThreadRepositoryRef> {
    if let Some(tf) = task_folder {
        return tf
            .repositories
            .iter()
            .map(|r| {
                let matching = changed.iter().find(|c| {
                    normalize_path(&c.repository_path) == normalize_path(&r.repository_path)
                });
                ThreadRepositoryRef {
                    scope_id: matching.and_then(|m| m.scope_id.clone()),
                    repository_path: r.repository_path.clone(),
                    name: r.name.clone(),
                    cwd: r.worktree_path.clone(),
                }
            })
            .collect();
    }
    let mut out = changed.to_vec();
    let mut seen: BTreeSet<String> = changed
        .iter()
        .map(|c| normalize_path(&c.repository_path))
        .collect();
    for folder in folders {
        if folder.state != WorkspaceFolderState::Git {
            continue;
        }
        let norm = normalize_path(&folder.path);
        if seen.insert(norm) {
            out.push(ThreadRepositoryRef {
                scope_id: None,
                repository_path: folder.path.clone(),
                name: folder.name.clone(),
                cwd: folder.path.clone(),
            });
        }
    }
    out
}

pub(crate) fn normalize_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed == "/" {
        return "/".to_owned();
    }
    trimmed.trim_end_matches(['/', '\\']).to_owned()
}
