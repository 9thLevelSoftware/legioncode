//! Multi-repository thread section ordering, worktree repo selection, and rewind restore checks
//! split from [`crate::linked_threads_repos`] (`60863edc41` / #35925).

use std::collections::{BTreeMap, BTreeSet};

use legion_contracts::workspace_folders::{
    TASK_FOLDER_DEFAULT_REPOSITORIES_MAX, TaskFolder, WorkspaceFolder, WorkspaceFolderState,
};

use crate::linked_threads_repos::{
    PinnedThreadRepository, RepositorySectionOrigin, ThreadRepositoryRef, ThreadRepositorySection,
    ThreadRepositorySections, normalize_path,
};

/// Derives visible multi-repository sections for a thread (`#35925`).
///
/// Worktree threads display their own task-folder checkouts (changed first, then clean worktrees,
/// sorted deterministically) plus changed submodules located inside those worktree paths, never
/// leaking unrelated shared project checkouts.
#[must_use]
pub fn thread_repository_sections(
    changed: &[ThreadRepositoryRef],
    pinned: &[PinnedThreadRepository],
    task_folder: Option<&TaskFolder>,
    show_all: bool,
) -> ThreadRepositorySections {
    if let Some(tf) = task_folder {
        return worktree_repository_sections(changed, tf);
    }
    let changed_by_key: BTreeMap<String, &ThreadRepositoryRef> = changed
        .iter()
        .map(|c| (normalize_path(&c.repository_path), c))
        .collect();
    let mut visible = Vec::new();
    let mut seen = BTreeSet::new();

    for c in changed {
        let key = normalize_path(&c.repository_path);
        if seen.insert(key) {
            visible.push(ThreadRepositorySection {
                key: c.repository_path.clone(),
                cwd: c.cwd.clone(),
                name: c.name.clone(),
                scope_id: c.scope_id.clone(),
                origin: RepositorySectionOrigin::Changed,
            });
        }
    }
    let mut extra_pinned = 0_usize;
    for p in pinned {
        let key = normalize_path(&p.repository_path);
        if changed_by_key.contains_key(&key) || !seen.insert(key) {
            continue;
        }
        if show_all || visible.len() < 12 {
            visible.push(ThreadRepositorySection {
                key: p.repository_path.clone(),
                cwd: p.repository_path.clone(),
                name: p.name.clone(),
                scope_id: None,
                origin: RepositorySectionOrigin::Pinned,
            });
        } else {
            extra_pinned += 1;
        }
    }
    ThreadRepositorySections {
        visible,
        hidden_count: extra_pinned,
    }
}

fn worktree_repository_sections(
    changed: &[ThreadRepositoryRef],
    tf: &TaskFolder,
) -> ThreadRepositorySections {
    let changed_keys: BTreeSet<String> = changed
        .iter()
        .map(|c| normalize_path(&c.repository_path))
        .collect();
    let mut worktrees = tf.repositories.clone();
    worktrees.sort_by(|a, b| {
        let a_changed = changed_keys.contains(&normalize_path(&a.repository_path));
        let b_changed = changed_keys.contains(&normalize_path(&b.repository_path));
        b_changed
            .cmp(&a_changed)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.repository_path.cmp(&b.repository_path))
    });

    let mut visible = Vec::new();
    let mut worktree_roots = Vec::new();
    for repo in &worktrees {
        worktree_roots.push(normalize_path(&repo.worktree_path));
        let changed_entry = changed
            .iter()
            .find(|c| normalize_path(&c.repository_path) == normalize_path(&repo.repository_path));
        visible.push(ThreadRepositorySection {
            key: repo.repository_path.clone(),
            cwd: repo.worktree_path.clone(),
            name: repo.name.clone(),
            scope_id: changed_entry.and_then(|c| c.scope_id.clone()),
            origin: RepositorySectionOrigin::Worktree,
        });
    }

    for item in changed {
        let norm_cwd = normalize_path(&item.cwd);
        let in_worktree = worktree_roots.iter().any(|root| {
            norm_cwd != *root
                && (norm_cwd.starts_with(&format!("{root}/"))
                    || norm_cwd.starts_with(&format!("{root}\\")))
        });
        let already_listed = tf
            .repositories
            .iter()
            .any(|r| normalize_path(&r.repository_path) == normalize_path(&item.repository_path));
        if in_worktree && !already_listed {
            visible.push(ThreadRepositorySection {
                key: item.repository_path.clone(),
                cwd: item.cwd.clone(),
                name: item.name.clone(),
                scope_id: item.scope_id.clone(),
                origin: RepositorySectionOrigin::Changed,
            });
        }
    }

    ThreadRepositorySections {
        visible,
        hidden_count: 0,
    }
}

/// Returns project Git repositories not yet pinned or shown, eligible for pinning (`#35925`).
#[must_use]
pub fn pinnable_thread_repositories(
    git_repos: &[ThreadRepositoryRef],
    pinned: &[PinnedThreadRepository],
) -> Vec<PinnedThreadRepository> {
    let pinned_keys: BTreeSet<String> = pinned
        .iter()
        .map(|p| normalize_path(&p.repository_path))
        .collect();
    git_repos
        .iter()
        .filter(|r| !pinned_keys.contains(&normalize_path(&r.repository_path)))
        .map(|r| PinnedThreadRepository {
            repository_path: r.repository_path.clone(),
            name: r.name.clone(),
        })
        .collect()
}

/// Lists tracked project Git folders not yet materialized into `task_folder` (`#35925`).
#[must_use]
pub fn addable_task_folder_repositories(
    task_folder: &TaskFolder,
    folders: &[WorkspaceFolder],
) -> Vec<PinnedThreadRepository> {
    let mut existing: BTreeSet<String> = task_folder
        .repositories
        .iter()
        .map(|r| normalize_path(&r.repository_path))
        .collect();
    existing.insert(normalize_path(&task_folder.project_root));
    folders
        .iter()
        .filter(|f| {
            f.state == WorkspaceFolderState::Git && !existing.contains(&normalize_path(&f.path))
        })
        .map(|f| PinnedThreadRepository {
            repository_path: f.path.clone(),
            name: f.name.clone(),
        })
        .collect()
}

/// Resolves default tracked-repository selections for launching a worktree thread, capped at
/// [`TASK_FOLDER_DEFAULT_REPOSITORIES_MAX`] (`#35925`).
#[must_use]
pub fn resolve_worktree_repository_selection(
    folders: &[WorkspaceFolder],
    explicit_selection: Option<&[String]>,
) -> Vec<String> {
    let git_paths: Vec<String> = folders
        .iter()
        .filter(|f| f.state == WorkspaceFolderState::Git)
        .map(|f| f.path.clone())
        .collect();
    match explicit_selection {
        Some(selected) => {
            let valid: BTreeSet<String> = git_paths.iter().map(|p| normalize_path(p)).collect();
            selected
                .iter()
                .filter(|p| valid.contains(&normalize_path(p)))
                .cloned()
                .collect()
        }
        None => git_paths
            .into_iter()
            .take(TASK_FOLDER_DEFAULT_REPOSITORIES_MAX)
            .collect(),
    }
}

/// File restore mode when rewinding a multi-repository thread checkpoint (`#35925`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindFileRestoreOutcome {
    /// Root checkpoint snapshot restores files directly.
    Files,
    /// Multi-repository linked project folder restores individual repo scopes after `target`.
    Repositories,
    /// No snapshot or subsequent repo changes exist to restore.
    None,
}

/// Determines how checkpoint rollback restores workspace files in single- or multi-repo mode
/// (`#35925`).
#[must_use]
pub const fn resolve_rewind_file_restore(
    root_checkpoint_ready: bool,
    task_folder: Option<&TaskFolder>,
    any_repo_changed_after_target: bool,
) -> RewindFileRestoreOutcome {
    if root_checkpoint_ready {
        return RewindFileRestoreOutcome::Files;
    }
    if let Some(folder) = task_folder
        && !folder.project_is_worktree
        && any_repo_changed_after_target
    {
        return RewindFileRestoreOutcome::Repositories;
    }
    RewindFileRestoreOutcome::None
}
