//! Multi-repository workspace folder and task-folder contracts (`workspaceFolders.ts` & `project.ts`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{ProjectId, ThreadId};

/// Maximum number of tracked folders supported on a project (`PROJECT_TRACKED_FOLDERS_MAX`).
pub const PROJECT_TRACKED_FOLDERS_MAX: usize = 128;

/// Maximum tracked repositories isolated automatically by default when creating a worktree thread.
pub const TASK_FOLDER_DEFAULT_REPOSITORIES_MAX: usize = 8;

/// State of a tracked or discovered workspace folder (`WorkspaceFolderState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceFolderState {
    /// Git repository with independent history.
    Git,
    /// Non-Git directory.
    Plain,
    /// Configured folder path does not currently exist on disk.
    Missing,
}

/// A folder associated with a project (`ProjectTrackedFolder`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrackedFolder {
    /// Absolute path on the owning environment.
    pub path: String,
    /// Display folder name.
    pub name: String,
    /// Whether the server discovered this repository automatically inside the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered: Option<bool>,
}

/// Resolved folder entry returned by `workspace.listFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFolder {
    /// Display folder name.
    pub name: String,
    /// Absolute path on the environment.
    pub path: String,
    /// Discovered Git/plain/missing state.
    pub state: WorkspaceFolderState,
    /// `Some(true)` when found automatically inside the project folder rather than added manually.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered: Option<bool>,
}

/// Request payload for `workspace.listFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListFoldersInput {
    /// Target project ID.
    pub project_id: ProjectId,
}

/// Response payload for `workspace.listFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListFoldersResult {
    /// Discovered repositories first, followed by user-added folders.
    pub folders: Vec<WorkspaceFolder>,
}

/// Request payload for `workspace.scanFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScanFoldersInput {
    /// Candidate workspace root directory to inspect.
    pub workspace_root: String,
}

/// Discovery source of a [`WorkspaceScannedFolder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceScannedFolderSource {
    /// Direct child directory immediately inside the scanned root.
    Child,
    /// Listed in a `.code-workspace` file.
    CodeWorkspace,
}

/// One candidate folder discovered by `workspace.scanFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScannedFolder {
    /// Absolute path on the environment.
    pub path: String,
    /// Display folder name.
    pub name: String,
    /// Whether the folder is a Git repository.
    pub is_git_repository: bool,
    /// How the scan discovered this entry.
    pub source: WorkspaceScannedFolderSource,
}

/// Response payload for `workspace.scanFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScanFoldersResult {
    /// Resolved path of the scanned workspace directory.
    pub workspace_root: String,
    /// Whether the workspace root itself is a Git repository.
    pub is_git_repository: bool,
    /// Path to the `.code-workspace` file read during scanning, if any.
    pub code_workspace_file: Option<String>,
    /// Candidate child or `.code-workspace` folders.
    pub folders: Vec<WorkspaceScannedFolder>,
}

/// One tracked repository checkout inside a thread's [`TaskFolder`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFolderRepository {
    /// Tracked repository path as stored on the project.
    pub repository_path: String,
    /// Worktree directory created inside the task folder.
    pub worktree_path: String,
    /// Display name of the repository.
    pub name: String,
}

/// Multi-repository task folder layout for a worktree thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFolder {
    /// Root directory containing the laid-out task folder.
    pub root: String,
    /// Owning project identifier.
    pub project_id: ProjectId,
    /// Project's canonical root directory.
    pub project_root: String,
    /// Common ancestor directory mirrored beneath `root`.
    pub anchor: String,
    /// Path of the project folder inside `root` (the thread's `worktreePath`).
    pub project_path: String,
    /// `true` when `project_path` is itself a Git worktree; `false` when linked/mirrored from a plain root.
    pub project_is_worktree: bool,
    /// Per-repository worktree entries inside this task folder.
    pub repositories: Vec<TaskFolderRepository>,
}

/// Request payload for `workspace.taskFolder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTaskFolderInput {
    /// Thread `worktreePath` or any child path inside the task folder.
    pub worktree_path: String,
}

/// Response payload for `workspace.taskFolder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTaskFolderResult {
    /// Matching task folder, or `None` for single-worktree threads.
    pub task_folder: Option<TaskFolder>,
}

/// Request payload for `workspace.addTaskFolderRepository`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAddTaskFolderRepositoryInput {
    /// Target thread identifier.
    pub thread_id: ThreadId,
    /// Tracked Git repository path to materialize inside the thread's task folder.
    pub repository_path: String,
}

/// Operation tag for [`TaskFolderError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskFolderOperation {
    /// Planning a task folder layout.
    Plan,
    /// Creating a task folder on disk.
    Create,
    /// Adding an extra repository worktree to an existing task folder.
    AddRepository,
    /// Removing a task folder on cleanup or deletion.
    Remove,
    /// Reading persisted task-folder state.
    Read,
}

/// Typed contract error for task-folder operations.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{detail}")]
pub struct TaskFolderError {
    /// Failing operation kind.
    pub operation: TaskFolderOperation,
    /// Safe user-facing detail message.
    pub detail: String,
}

/// Operation tag for [`WorkspaceFoldersError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceFoldersOperation {
    /// Listing project folders.
    List,
    /// Scanning a directory for candidate repositories.
    Scan,
}

/// Typed contract error for `workspace.listFolders` / `workspace.scanFolders`.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{detail}")]
pub struct WorkspaceFoldersError {
    /// Failing operation kind.
    pub operation: WorkspaceFoldersOperation,
    /// Safe user-facing detail message.
    pub detail: String,
}

/// Tracked repository patch returned as part of multi-repo turn diffs (`ThreadTurnRepositoryDiff`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurnRepositoryDiff {
    /// Repository display name.
    pub name: String,
    /// Project's tracked repository path (matching checkpoint scope `repositoryPath`).
    pub repository_path: String,
    /// Repository path relative to the thread cwd (`/`-separated).
    pub relative_path: String,
    /// Unified Git diff relative to the repository.
    pub diff: String,
}
