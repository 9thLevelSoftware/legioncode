//! Multi-repo task-folder planning and SQLite persistence coordinator (`60863edc41` `#35925`),
//! integrated with [`legion_contracts::workspace_folders`] and [`legion_persistence::task_folder_store`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use legion_contracts::ids::ProjectId;
use legion_contracts::workspace_folders::{
    PROJECT_TRACKED_FOLDERS_MAX, ProjectTrackedFolder, TASK_FOLDER_DEFAULT_REPOSITORIES_MAX,
    TaskFolder, TaskFolderError, TaskFolderOperation, TaskFolderRepository, WorkspaceFolderState,
};
use legion_persistence::task_folder_store::{
    TaskFolderStoreError, add_repository as store_add_repository, ensure_task_folders_schema,
    get_by_worktree_or_child_path, list_task_folders as store_list_task_folders, remove_by_root,
    upsert_task_folder,
};
use rusqlite::Connection;

use crate::workspace_folders_scanner::lexical_clean;
pub use crate::workspace_folders_scanner::{
    classify_tracked_folders, discover_project_folders, is_within_root, merge_project_folders,
    parse_gitmodules_paths, scan_workspace_folders, strip_lenient_jsonc,
};

/// Computed multi-repository layout plan for a task folder prior to checkout (`TaskFolderPlan`, `60863edc41`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFolderPlan {
    /// Owning project identifier.
    pub project_id: ProjectId,
    /// Canonical project workspace root path.
    pub project_root: PathBuf,
    /// `true` when the project root itself has a `.git` entry.
    pub project_is_repository: bool,
    /// Deepest directory holding `project_root` and every reachable repository.
    pub anchor: PathBuf,
    /// Tracked Git folders receiving their own worktree inside the task folder.
    pub repositories: Vec<ProjectTrackedFolder>,
    /// All tracked Git folder paths belonging to the project.
    pub tracked_repositories: Vec<String>,
    /// Tracked repositories on separate drives / unrelated filesystem roots that cannot be mirrored.
    pub unreachable_repositories: Vec<PathBuf>,
}

/// Deepest directory that holds every folder in `folders`, or `None` when none share a root
/// (such as paths on distinct Windows drive letters, `60863edc41`).
#[must_use]
pub fn common_ancestor(folders: &[PathBuf]) -> Option<PathBuf> {
    let first = folders.first()?;
    let mut ancestor = lexical_clean(first);
    for folder in &folders[1..] {
        let cleaned = lexical_clean(folder);
        while !is_within_root(&cleaned, &ancestor) {
            let parent = ancestor.parent()?.to_path_buf();
            if parent == ancestor {
                return None;
            }
            ancestor = parent;
        }
    }
    Some(ancestor)
}

/// Computed common mirror anchor and unreachable sibling repositories (`TaskFolders.layoutAnchor`, `60863edc41`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutAnchorResult {
    /// Deepest common ancestor holding `project_root` and all reachable `repositories`.
    pub anchor: PathBuf,
    /// Repositories sharing no root with `anchor` (e.g., distinct Windows drive).
    pub unreachable: Vec<PathBuf>,
}

/// Computes the common task-folder anchor for `project_root` and `repositories` (`60863edc41`).
#[must_use]
pub fn layout_anchor(project_root: &Path, repositories: &[PathBuf]) -> LayoutAnchorResult {
    let mut anchor = lexical_clean(project_root);
    let mut unreachable = Vec::new();
    for repo in repositories {
        if let Some(shared) = common_ancestor(&[anchor.clone(), repo.clone()]) {
            anchor = shared;
        } else {
            unreachable.push(repo.clone());
        }
    }
    LayoutAnchorResult {
        anchor,
        unreachable,
    }
}

/// Maps `<root>/<relative_from_anchor>` cleanly inside a laid-out task folder (`60863edc41`).
#[must_use]
pub fn build_mirrored_worktree_path(
    task_folder_root: &Path,
    anchor: &Path,
    repo_path: &Path,
) -> PathBuf {
    let clean_anchor = lexical_clean(anchor);
    let clean_repo = lexical_clean(repo_path);
    match clean_repo.strip_prefix(&clean_anchor) {
        Ok(rel) if !rel.as_os_str().is_empty() => task_folder_root.join(rel),
        Ok(_) => task_folder_root.to_path_buf(),
        Err(_) => task_folder_root.join(
            clean_repo
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("repo")),
        ),
    }
}

/// Stateless coordinator bridging workspace folder discovery, layout planning, and `SQLite`
/// [`legion_persistence::task_folder_store`] persistence (`60863edc41`).
#[derive(Debug, Default, Clone, Copy)]
pub struct TaskFolderCoordinator;

impl TaskFolderCoordinator {
    /// Returns effective tracked folders for a project (auto-discovered + manual, capped at
    /// [`PROJECT_TRACKED_FOLDERS_MAX`]).
    #[must_use]
    pub fn resolve_project_folders(
        project_root: &Path,
        tracked_folders: &[ProjectTrackedFolder],
    ) -> Vec<ProjectTrackedFolder> {
        let mut merged =
            merge_project_folders(&discover_project_folders(project_root), tracked_folders);
        merged.truncate(PROJECT_TRACKED_FOLDERS_MAX);
        merged
    }

    /// Plans a multi-repo task folder for `project_root`, returning `Ok(None)` when the project
    /// tracks no existing Git repositories (`60863edc41`).
    ///
    /// When `requested_repositories` is `None`, at most [`TASK_FOLDER_DEFAULT_REPOSITORIES_MAX`] (`8`)
    /// Git repositories are selected by default. When `Some`, every entry must match a tracked Git repo.
    pub fn plan(
        project_id: ProjectId,
        project_root: &Path,
        tracked_folders: &[ProjectTrackedFolder],
        requested_repositories: Option<&[&str]>,
    ) -> Result<Option<TaskFolderPlan>, TaskFolderError> {
        let effective = Self::resolve_project_folders(project_root, tracked_folders);
        let git_folders: Vec<ProjectTrackedFolder> = classify_tracked_folders(&effective)
            .into_iter()
            .zip(effective)
            .filter(|(wf, _)| wf.state == WorkspaceFolderState::Git)
            .map(|(_, tf)| tf)
            .collect();
        if git_folders.is_empty() {
            return Ok(None);
        }

        let tracked_repositories: Vec<String> =
            git_folders.iter().map(|f| f.path.clone()).collect();
        let selected: Vec<ProjectTrackedFolder> = if let Some(wanted) = requested_repositories {
            let mut seen = HashSet::new();
            let mut chosen = Vec::with_capacity(wanted.len());
            for req in wanted {
                let clean = lexical_clean(Path::new(req)).to_string_lossy().into_owned();
                let found = git_folders
                    .iter()
                    .find(|f| lexical_clean(Path::new(&f.path)) == Path::new(&clean))
                    .ok_or_else(|| TaskFolderError {
                        operation: TaskFolderOperation::Plan,
                        detail: format!(
                            "Repository `{req}` is not a tracked Git folder on this project."
                        ),
                    })?;
                if seen.insert(found.path.clone()) {
                    chosen.push(found.clone());
                }
            }
            chosen
        } else {
            git_folders
                .into_iter()
                .take(TASK_FOLDER_DEFAULT_REPOSITORIES_MAX)
                .collect()
        };

        let selected_paths: Vec<PathBuf> =
            selected.iter().map(|f| PathBuf::from(&f.path)).collect();
        let LayoutAnchorResult {
            anchor,
            unreachable,
        } = layout_anchor(project_root, &selected_paths);
        let reachable_repos: Vec<ProjectTrackedFolder> = selected
            .into_iter()
            .filter(|f| !unreachable.contains(&PathBuf::from(&f.path)))
            .collect();

        Ok(Some(TaskFolderPlan {
            project_id,
            project_root: lexical_clean(project_root),
            project_is_repository: project_root.join(".git").exists(),
            anchor,
            repositories: reachable_repos,
            tracked_repositories,
            unreachable_repositories: unreachable,
        }))
    }

    /// Ensures the `task_folders` schema exists and persists `task_folder` via [`upsert_task_folder`].
    pub fn record_claimed(
        conn: &Connection,
        task_folder: &TaskFolder,
        created_at_iso: Option<&str>,
    ) -> Result<(), TaskFolderError> {
        ensure_task_folders_schema(conn)
            .map_err(|e| map_store_err(TaskFolderOperation::Create, &e))?;
        upsert_task_folder(conn, task_folder, created_at_iso)
            .map_err(|e| map_store_err(TaskFolderOperation::Create, &e))
    }

    /// Looks up the deepest persisted task folder whose root contains `target`.
    pub fn find(conn: &Connection, target: &Path) -> Result<Option<TaskFolder>, TaskFolderError> {
        get_by_worktree_or_child_path(conn, target)
            .map_err(|e| map_store_err(TaskFolderOperation::Read, &e))
    }

    /// Lists all persisted task folders sorted by `root`.
    pub fn list(conn: &Connection) -> Result<Vec<TaskFolder>, TaskFolderError> {
        store_list_task_folders(conn).map_err(|e| map_store_err(TaskFolderOperation::Read, &e))
    }

    /// Appends `repository` to the task folder identified by `root`, respecting
    /// [`TASK_FOLDER_DEFAULT_REPOSITORIES_MAX`] unless overridden.
    pub fn add_repository(
        conn: &Connection,
        root: &str,
        repository: TaskFolderRepository,
        max_repositories: Option<usize>,
    ) -> Result<TaskFolder, TaskFolderError> {
        store_add_repository(conn, root, repository, max_repositories)
            .map_err(|e| map_store_err(TaskFolderOperation::AddRepository, &e))
    }

    /// Removes the task folder record keyed by `root`.
    pub fn forget(conn: &Connection, root: &str) -> Result<bool, TaskFolderError> {
        remove_by_root(conn, root).map_err(|e| map_store_err(TaskFolderOperation::Remove, &e))
    }
}

fn map_store_err(operation: TaskFolderOperation, err: &TaskFolderStoreError) -> TaskFolderError {
    TaskFolderError {
        operation,
        detail: err.to_string(),
    }
}
