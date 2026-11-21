//! Public SQLite queries and offline project mutations against `projection_projects`.

use std::time::SystemTime;

use rusqlite::params;

use crate::ownership::format_iso8601_utc;

use super::row::{
    ValidatedCreateInput, apply_unarchive_conn, count_non_deleted_threads,
    find_active_by_workspace_conn, find_non_deleted_project_conn, get_by_id_conn,
    insert_project_row, parse_optional_title, project_id_exists, read_projects_conn,
    soft_delete_child_threads,
};
use super::schema::WriteTx;
use super::{
    CreateProjectInput, ProjectAddResult, ProjectDatabase, ProjectRecord, ProjectStoreError,
    RenameProjectResult, SoftDeleteProjectResult,
};

/// Lists all project rows from `projection_projects` (active, archived, and soft-deleted),
/// ordered by `created_at ASC, project_id ASC`.
///
/// Missing database files and databases without a `projection_projects` table return `Ok(vec![])`.
pub fn list_projects(db: impl ProjectDatabase) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
    db.read_projects_with(|maybe_conn| {
        let Some(conn) = maybe_conn else {
            return Ok(Vec::new());
        };
        read_projects_conn(conn, true)
    })
}

/// Lists all non-deleted project rows (`deleted_at IS NULL`, both active and archived),
/// ordered by `created_at ASC, project_id ASC`.
pub fn list_non_deleted_projects(
    db: impl ProjectDatabase,
) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
    db.read_projects_with(|maybe_conn| {
        let Some(conn) = maybe_conn else {
            return Ok(Vec::new());
        };
        read_projects_conn(conn, false)
    })
}

/// Looks up a single non-deleted project by exact `project_id` or exact `workspace_root`.
pub fn find_project_by_id_or_workspace(
    db: impl ProjectDatabase,
    target: &str,
) -> Result<Option<ProjectRecord>, ProjectStoreError> {
    db.read_projects_with(|maybe_conn| {
        let Some(conn) = maybe_conn else {
            return Ok(None);
        };
        find_non_deleted_project_conn(conn, target)
    })
}

/// Creates a new project row in `projection_projects`, returning [`ProjectStoreError::WorkspaceConflict`]
/// if any non-deleted project (active or archived) already owns `input.workspace_root`.
pub fn create_project(
    db: impl ProjectDatabase,
    input: &CreateProjectInput,
) -> Result<ProjectRecord, ProjectStoreError> {
    let validated = ValidatedCreateInput::parse(input)?;
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        if let Some(existing_id) = input.project_id.as_deref()
            && project_id_exists(conn, existing_id)?
        {
            return Err(ProjectStoreError::ProjectConflict {
                project_id: existing_id.to_owned(),
            });
        }
        if let Some(conflict) = find_active_by_workspace_conn(conn, &validated.workspace_root)? {
            return Err(ProjectStoreError::WorkspaceConflict {
                workspace_root: validated.workspace_root,
                conflicting_project_id: conflict.id,
            });
        }
        let record = insert_project_row(conn, &validated)?;
        tx.commit()?;
        Ok(record)
    })
}

/// Unarchives a non-deleted project matched by exact `project_id` or `workspace_root`,
/// optionally updating its title (`has_custom_title = 1`).
pub fn unarchive_project(
    db: impl ProjectDatabase,
    target: &str,
    title_override: Option<&str>,
) -> Result<ProjectRecord, ProjectStoreError> {
    let trimmed_title = parse_optional_title(title_override)?;
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        let Some(existing) = find_non_deleted_project_conn(conn, target)? else {
            return Err(ProjectStoreError::NotFound {
                target: target.to_owned(),
            });
        };
        let now = format_iso8601_utc(SystemTime::now());
        let record = apply_unarchive_conn(conn, &existing.id, trimmed_title.as_deref(), &now)?;
        tx.commit()?;
        Ok(record)
    })
}

/// CLI `project add` semantics: creates a new project, or unarchives an existing archived project
/// with the same `workspace_root` (applying an optional `input.title` override).
///
/// Fails with [`ProjectStoreError::WorkspaceConflict`] if an unarchived active project already
/// owns `input.workspace_root`.
pub fn add_or_unarchive_project(
    db: impl ProjectDatabase,
    input: &CreateProjectInput,
) -> Result<ProjectAddResult, ProjectStoreError> {
    let validated = ValidatedCreateInput::parse(input)?;
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        if let Some(existing) = find_active_by_workspace_conn(conn, &validated.workspace_root)? {
            if existing.is_archived() {
                let record = apply_unarchive_conn(
                    conn,
                    &existing.id,
                    validated.explicit_title.as_deref(),
                    &validated.now_iso,
                )?;
                tx.commit()?;
                return Ok(ProjectAddResult {
                    project: record,
                    created: false,
                    unarchived: true,
                });
            }
            return Err(ProjectStoreError::WorkspaceConflict {
                workspace_root: validated.workspace_root,
                conflicting_project_id: existing.id,
            });
        }

        if let Some(existing_id) = input.project_id.as_deref()
            && project_id_exists(conn, existing_id)?
        {
            return Err(ProjectStoreError::ProjectConflict {
                project_id: existing_id.to_owned(),
            });
        }
        let record = insert_project_row(conn, &validated)?;
        tx.commit()?;
        Ok(ProjectAddResult {
            project: record,
            created: true,
            unarchived: false,
        })
    })
}

/// Archives a non-deleted project matched by exact `project_id` or `workspace_root`,
/// preserving any existing `archived_at` timestamp.
pub fn archive_project(
    db: impl ProjectDatabase,
    target: &str,
) -> Result<ProjectRecord, ProjectStoreError> {
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        let Some(existing) = find_non_deleted_project_conn(conn, target)? else {
            return Err(ProjectStoreError::NotFound {
                target: target.to_owned(),
            });
        };
        let now = format_iso8601_utc(SystemTime::now());
        conn.execute(
            "UPDATE projection_projects
             SET archived_at = COALESCE(archived_at, ?1), updated_at = ?1
             WHERE project_id = ?2 AND deleted_at IS NULL",
            params![now, existing.id],
        )
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "archive_project",
            source,
        })?;
        let updated = get_by_id_conn(conn, &existing.id)?;
        tx.commit()?;
        Ok(updated)
    })
}

/// Renames a non-deleted project matched by exact `project_id` or `workspace_root`.
///
/// When the trimmed `new_title` already matches the existing title, no row update is written and
/// [`RenameProjectResult::changed`] is `false`.
pub fn rename_project(
    db: impl ProjectDatabase,
    target: &str,
    new_title: &str,
) -> Result<RenameProjectResult, ProjectStoreError> {
    let trimmed = new_title.trim();
    if trimmed.is_empty() {
        return Err(ProjectStoreError::InvalidTitle);
    }
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        let Some(existing) = find_non_deleted_project_conn(conn, target)? else {
            return Err(ProjectStoreError::NotFound {
                target: target.to_owned(),
            });
        };
        if existing.title == trimmed {
            tx.commit()?;
            return Ok(RenameProjectResult {
                project: existing,
                changed: false,
            });
        }
        let now = format_iso8601_utc(SystemTime::now());
        conn.execute(
            "UPDATE projection_projects
             SET title = ?1, has_custom_title = 1, updated_at = ?2
             WHERE project_id = ?3 AND deleted_at IS NULL",
            params![trimmed, now, existing.id],
        )
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "rename_project",
            source,
        })?;
        let updated = get_by_id_conn(conn, &existing.id)?;
        tx.commit()?;
        Ok(RenameProjectResult {
            project: updated,
            changed: true,
        })
    })
}

/// Soft-deletes a non-deleted project matched by exact `project_id` or `workspace_root`.
///
/// Non-deleted threads (active or archived) in `orchestration_v2_projection_threads` or
/// `projection_threads` reject deletion with [`ProjectStoreError::ProjectNotEmpty`] unless `force`
/// is `true`, in which case those child threads are soft-deleted in the same transaction first.
pub fn soft_delete_project(
    db: impl ProjectDatabase,
    target: &str,
    force: bool,
) -> Result<SoftDeleteProjectResult, ProjectStoreError> {
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        let Some(existing) = find_non_deleted_project_conn(conn, target)? else {
            return Err(ProjectStoreError::NotFound {
                target: target.to_owned(),
            });
        };
        let active_thread_count = count_non_deleted_threads(conn, &existing.id)?;
        if active_thread_count > 0 && !force {
            return Err(ProjectStoreError::ProjectNotEmpty {
                project_id: existing.id,
                active_thread_count,
            });
        }
        let now = format_iso8601_utc(SystemTime::now());
        if active_thread_count > 0 && force {
            soft_delete_child_threads(conn, &existing.id, &now)?;
        }
        conn.execute(
            "UPDATE projection_projects
             SET deleted_at = ?1, updated_at = ?1
             WHERE project_id = ?2 AND deleted_at IS NULL",
            params![now, existing.id],
        )
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "soft_delete_project",
            source,
        })?;
        let updated = get_by_id_conn(conn, &existing.id)?;
        tx.commit()?;
        Ok(SoftDeleteProjectResult {
            project: updated,
            deleted_thread_count: active_thread_count,
        })
    })
}

/// Reads persisted tracked workspace folders (`tracked_folders_json`) for a non-deleted project.
pub fn read_project_tracked_folders(
    db: impl ProjectDatabase,
    target: &str,
) -> Result<Vec<t3_contracts::workspace_folders::ProjectTrackedFolder>, ProjectStoreError> {
    db.read_projects_with(|maybe_conn| {
        let Some(conn) = maybe_conn else {
            return Ok(Vec::new());
        };
        let Some(existing) = find_non_deleted_project_conn(conn, target)? else {
            return Err(ProjectStoreError::NotFound {
                target: target.to_owned(),
            });
        };
        let raw: String = conn
            .query_row(
                "SELECT COALESCE(tracked_folders_json, '[]')
                 FROM projection_projects
                 WHERE project_id = ?1",
                [&existing.id],
                |row| row.get(0),
            )
            .map_err(|source| ProjectStoreError::Sqlite {
                operation: "read_project_tracked_folders",
                source,
            })?;
        serde_json::from_str(&raw).map_err(|_| ProjectStoreError::InvalidScriptsJson)
    })
}

/// Updates `tracked_folders_json` on a non-deleted project matched by ID or workspace root.
pub fn set_project_tracked_folders(
    db: impl ProjectDatabase,
    target: &str,
    folders: &[t3_contracts::workspace_folders::ProjectTrackedFolder],
) -> Result<Vec<t3_contracts::workspace_folders::ProjectTrackedFolder>, ProjectStoreError> {
    let encoded =
        serde_json::to_string(folders).map_err(|_| ProjectStoreError::InvalidScriptsJson)?;
    db.write_projects_with(|conn| {
        let tx = WriteTx::begin(conn)?;
        let Some(existing) = find_non_deleted_project_conn(conn, target)? else {
            return Err(ProjectStoreError::NotFound {
                target: target.to_owned(),
            });
        };
        let now = format_iso8601_utc(SystemTime::now());
        conn.execute(
            "UPDATE projection_projects
             SET tracked_folders_json = ?1, updated_at = ?2
             WHERE project_id = ?3 AND deleted_at IS NULL",
            params![encoded, now, existing.id],
        )
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "set_project_tracked_folders",
            source,
        })?;
        tx.commit()?;
        Ok(folders.to_vec())
    })
}
