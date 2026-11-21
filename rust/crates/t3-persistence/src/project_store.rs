//! Offline SQLite project storage (`projection_projects`) for `t3 project` CLI and V2 stores.
//!
//! Mirrors `apps/server/src/cli/project.ts`, `apps/server/src/project/ProjectService.ts`, and
//! `apps/server/src/orchestration-v2/ProjectStore.ts`.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_persistence::project_store::{
//!     CreateProjectInput, add_or_unarchive_project, archive_project, create_project, list_projects,
//!     rename_project, soft_delete_project,
//! };
//!
//! let dir = tempfile::tempdir()?;
//! let db_path = dir.path().join("statev2.sqlite");
//! let created = create_project(
//!     &db_path,
//!     &CreateProjectInput::new("/work/alpha").with_title("Alpha"),
//! )?;
//! let renamed = rename_project(&db_path, "/work/alpha", "Beta")?;
//! assert!(renamed.changed);
//! archive_project(&db_path, &created.id)?;
//! let unarchived = add_or_unarchive_project(
//!     &db_path,
//!     &CreateProjectInput::new("/work/alpha").with_title("Restored"),
//! )?;
//! assert!(unarchived.unarchived);
//! assert_eq!(unarchived.project.title, "Restored");
//! let deleted = soft_delete_project(&db_path, &created.id, false)?;
//! assert!(deleted.project.is_deleted());
//! assert_eq!(list_projects(&db_path)?.len(), 1);
//! # Ok(())
//! # }
//! ```

mod db;
mod favicon;
mod queries;
mod row;
mod schema;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::sqlite::{SqliteStore, SqliteStoreError};

pub use db::ProjectDatabase;
pub use favicon::{
    FALLBACK_FAVICON_CANDIDATES, FAVICON_CANDIDATES, ICON_SOURCE_FILES, resolve_project_favicon,
};
pub use queries::{
    add_or_unarchive_project, archive_project, create_project, find_project_by_id_or_workspace,
    list_non_deleted_projects, list_projects, read_project_tracked_folders, rename_project,
    set_project_tracked_folders, soft_delete_project, unarchive_project,
};
pub use schema::ensure_project_store_schema;

/// Errors produced when reading or mutating offline `projection_projects` state.
#[derive(Debug, Error)]
pub enum ProjectStoreError {
    /// Opening or closing the SQLite database file failed.
    #[error("failed to open project database '{}': {source}", path.display())]
    OpenDatabase {
        /// Target SQLite file path.
        path: PathBuf,
        /// Underlying [`SqliteStoreError`].
        #[source]
        source: Box<SqliteStoreError>,
    },
    /// Underlying SQLite query or statement error.
    #[error("project store SQLite error during '{operation}': {source}")]
    Sqlite {
        /// Operation label.
        operation: &'static str,
        /// Underlying `rusqlite::Error`.
        #[source]
        source: rusqlite::Error,
    },
    /// No non-deleted project matched the target ID or workspace root.
    #[error("No active project found matching '{target}'.")]
    NotFound {
        /// Project ID or workspace path requested by caller.
        target: String,
    },
    /// Another non-deleted project already owns `workspace_root`.
    #[error(
        "Active project '{conflicting_project_id}' already exists for workspace root '{workspace_root}'."
    )]
    WorkspaceConflict {
        /// Conflicting workspace root path.
        workspace_root: String,
        /// Existing owning project ID.
        conflicting_project_id: String,
    },
    /// Explicit `project_id` already exists in `projection_projects`.
    #[error("Project '{project_id}' already exists and cannot be created twice.")]
    ProjectConflict {
        /// Conflicting project ID.
        project_id: String,
    },
    /// Non-deleted child threads remain and `force` was not passed.
    #[error(
        "Project '{project_id}' is not empty ({active_thread_count} thread(s) remain); pass force to delete."
    )]
    ProjectNotEmpty {
        /// Project ID refusing deletion.
        project_id: String,
        /// Count of non-deleted child threads across V1/V2 projection tables.
        active_thread_count: usize,
    },
    /// Empty or whitespace-only title was supplied.
    #[error("Project title must not be empty.")]
    InvalidTitle,
    /// Empty or whitespace-only workspace root was supplied.
    #[error("Project workspace_root must not be empty.")]
    InvalidWorkspaceRoot,
    /// Provided `scripts_json` is not a valid JSON array.
    #[error("Project scripts_json must be a valid JSON array.")]
    InvalidScriptsJson,
}

/// Row stored in `projection_projects`, including active, archived, and soft-deleted states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    /// Primary project identifier (`project_id`).
    pub id: String,
    /// Display title.
    pub title: String,
    /// Project workspace directory path.
    pub workspace_root: String,
    /// Whether the user explicitly renamed the project (`has_custom_title`).
    pub has_custom_title: bool,
    /// Serialized JSON array of project scripts (`scripts_json`).
    pub scripts_json: String,
    /// Serialized default model selection JSON, if configured.
    pub default_model_selection_json: Option<String>,
    /// Default thread environment mode, if configured.
    pub default_thread_env_mode: Option<String>,
    /// Whether auto-pull is enabled for this project.
    pub auto_pull: bool,
    /// Resolved favicon path override, if any.
    pub favicon_path: Option<String>,
    /// Serialized custom project icon JSON, if configured.
    pub project_icon_json: Option<String>,
    /// ISO-8601 UTC timestamp when pinned, if pinned.
    pub pinned_at: Option<String>,
    /// ISO-8601 UTC timestamp when archived, if archived.
    pub archived_at: Option<String>,
    /// ISO-8601 UTC creation timestamp.
    pub created_at: String,
    /// ISO-8601 UTC last-update timestamp.
    pub updated_at: String,
    /// ISO-8601 UTC soft-delete timestamp, if soft-deleted.
    pub deleted_at: Option<String>,
}

impl ProjectRecord {
    /// Returns `true` when the project is neither deleted nor archived.
    pub const fn is_active(&self) -> bool {
        self.deleted_at.is_none() && self.archived_at.is_none()
    }

    /// Returns `true` when the project is non-deleted and archived.
    pub const fn is_archived(&self) -> bool {
        self.deleted_at.is_none() && self.archived_at.is_some()
    }

    /// Returns `true` when the project has a `deleted_at` timestamp.
    pub const fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// Parameters for [`create_project`] and [`add_or_unarchive_project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectInput {
    /// Optional explicit project ID (defaults to a generated `UUIDv4`).
    pub project_id: Option<String>,
    /// Optional project title (defaults to the `workspace_root` directory basename).
    pub title: Option<String>,
    /// Normalized workspace directory path.
    pub workspace_root: String,
    /// Optional serialized JSON array of scripts (defaults to `"[]"`).
    pub scripts_json: Option<String>,
    /// Optional deterministic ISO-8601 UTC timestamp (defaults to `SystemTime::now()`).
    pub now_iso: Option<String>,
}

impl CreateProjectInput {
    /// Creates an input targeting `workspace_root` with default ID, title, scripts, and timestamp.
    pub fn new(workspace_root: impl Into<String>) -> Self {
        Self {
            project_id: None,
            title: None,
            workspace_root: workspace_root.into(),
            scripts_json: None,
            now_iso: None,
        }
    }

    /// Sets an explicit project ID.
    #[must_use]
    pub fn with_project_id(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    /// Sets an explicit title override.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets an explicit `scripts_json` array payload.
    #[must_use]
    pub fn with_scripts_json(mut self, scripts_json: impl Into<String>) -> Self {
        self.scripts_json = Some(scripts_json.into());
        self
    }

    /// Sets an explicit ISO-8601 UTC timestamp.
    #[must_use]
    pub fn with_now_iso(mut self, now_iso: impl Into<String>) -> Self {
        self.now_iso = Some(now_iso.into());
        self
    }
}

/// Outcome returned by [`add_or_unarchive_project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAddResult {
    /// Resulting non-deleted project record.
    pub project: ProjectRecord,
    /// `true` when a brand new row was inserted.
    pub created: bool,
    /// `true` when an existing archived row was unarchived.
    pub unarchived: bool,
}

/// Outcome returned by [`rename_project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameProjectResult {
    /// Project row after the rename check/update.
    pub project: ProjectRecord,
    /// `false` when the trimmed title already matched the persisted title.
    pub changed: bool,
}

/// Outcome returned by [`soft_delete_project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftDeleteProjectResult {
    /// Soft-deleted project record with `deleted_at` populated.
    pub project: ProjectRecord,
    /// Number of non-deleted child threads soft-deleted when `force` was `true`.
    pub deleted_thread_count: usize,
}

impl SqliteStore {
    /// Lists all `projection_projects` rows (active, archived, and soft-deleted).
    pub fn list_projects(&self) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
        list_projects(self)
    }

    /// Lists all non-deleted `projection_projects` rows.
    pub fn list_non_deleted_projects(&self) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
        list_non_deleted_projects(self)
    }

    /// Looks up a non-deleted project by exact ID or workspace root.
    pub fn find_project_by_id_or_workspace(
        &self,
        target: &str,
    ) -> Result<Option<ProjectRecord>, ProjectStoreError> {
        find_project_by_id_or_workspace(self, target)
    }

    /// Creates a new project row in this store.
    pub fn create_project(
        &self,
        input: &CreateProjectInput,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        create_project(self, input)
    }

    /// Unarchives a matching non-deleted project row in this store.
    pub fn unarchive_project(
        &self,
        target: &str,
        title_override: Option<&str>,
    ) -> Result<ProjectRecord, ProjectStoreError> {
        unarchive_project(self, target, title_override)
    }

    /// Creates or unarchives a project row in this store (`cli/project.ts` semantics).
    pub fn add_or_unarchive_project(
        &self,
        input: &CreateProjectInput,
    ) -> Result<ProjectAddResult, ProjectStoreError> {
        add_or_unarchive_project(self, input)
    }

    /// Archives a matching non-deleted project row in this store.
    pub fn archive_project(&self, target: &str) -> Result<ProjectRecord, ProjectStoreError> {
        archive_project(self, target)
    }

    /// Renames a matching non-deleted project row in this store.
    pub fn rename_project(
        &self,
        target: &str,
        new_title: &str,
    ) -> Result<RenameProjectResult, ProjectStoreError> {
        rename_project(self, target, new_title)
    }

    /// Soft-deletes a matching non-deleted project row in this store.
    pub fn soft_delete_project(
        &self,
        target: &str,
        force: bool,
    ) -> Result<SoftDeleteProjectResult, ProjectStoreError> {
        soft_delete_project(self, target, force)
    }
}
