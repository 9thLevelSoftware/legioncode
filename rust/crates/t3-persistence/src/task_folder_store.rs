//! SQLite store for multi-repository task folders (`task_folders`).
//!
//! Mirrors `apps/server/src/workspace/TaskFolderStore.ts`.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension as _, params};
use t3_contracts::ids::ProjectId;
use t3_contracts::workspace_folders::{
    TASK_FOLDER_DEFAULT_REPOSITORIES_MAX, TaskFolder, TaskFolderRepository,
};
use thiserror::Error;

use crate::ownership::format_iso8601_utc;

/// Errors returned by [`crate::task_folder_store`] operations.
#[derive(Debug, Error)]
pub enum TaskFolderStoreError {
    /// Underlying SQLite failure.
    #[error("task folder store SQLite error during '{operation}': {source}")]
    Sqlite {
        /// Operation label.
        operation: &'static str,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
    /// Failed to parse or encode `repositories_json`.
    #[error("task folder store JSON error during '{operation}': {source}")]
    Json {
        /// Operation label.
        operation: &'static str,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Invalid input path or project identifier.
    #[error("invalid task folder data: {0}")]
    InvalidInput(String),
    /// Requested task folder root does not exist in `task_folders`.
    #[error("task folder not found: '{root}'")]
    NotFound {
        /// Missing task folder root.
        root: String,
    },
    /// Adding another repository exceeds the configured maximum per task folder.
    #[error("task folder '{root}' already reached its maximum of {limit} tracked repositories")]
    RepositoryLimitExceeded {
        /// Target task folder root.
        root: String,
        /// Maximum repositories allowed.
        limit: usize,
    },
}

fn sqlite_err(operation: &'static str, source: rusqlite::Error) -> TaskFolderStoreError {
    TaskFolderStoreError::Sqlite { operation, source }
}

fn json_err(operation: &'static str, source: serde_json::Error) -> TaskFolderStoreError {
    TaskFolderStoreError::Json { operation, source }
}

/// Ensures `task_folders` and `idx_task_folders_real_root` exist on `conn`.
pub fn ensure_task_folders_schema(conn: &Connection) -> Result<(), TaskFolderStoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_folders (
           root TEXT PRIMARY KEY,
           real_root TEXT NOT NULL,
           project_id TEXT NOT NULL,
           project_root TEXT NOT NULL,
           anchor TEXT NOT NULL,
           project_path TEXT NOT NULL,
           project_is_worktree INTEGER NOT NULL,
           repositories_json TEXT NOT NULL DEFAULT '[]',
           created_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_task_folders_real_root
           ON task_folders (real_root);",
    )
    .map_err(|source| sqlite_err("ensure_task_folders_schema", source))
}

fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolves the canonical path for the deepest existing ancestor joined with any non-existent tail.
fn resolve_real_location(target: &Path) -> PathBuf {
    if let Ok(real) = fs::canonicalize(target) {
        return real;
    }
    let Some(parent) = target.parent() else {
        return lexical_clean(target);
    };
    if parent == target {
        return lexical_clean(target);
    }
    match target.file_name() {
        Some(name) => resolve_real_location(parent).join(name),
        None => resolve_real_location(parent),
    }
}

fn collect_ancestor_candidates(target: &Path) -> Vec<String> {
    let named = lexical_clean(target);
    let real = resolve_real_location(&named);
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for path in named.ancestors().chain(real.ancestors()) {
        let s = path.to_string_lossy().into_owned();
        if !s.is_empty() && seen.insert(s.clone()) {
            candidates.push(s);
        }
    }
    candidates
}

fn decode_row(row: &rusqlite::Row<'_>) -> Result<(TaskFolder, String), TaskFolderStoreError> {
    let root: String = row.get(0).map_err(|e| sqlite_err("read_row:root", e))?;
    let project_id_raw: String = row
        .get(1)
        .map_err(|e| sqlite_err("read_row:project_id", e))?;
    let project_root: String = row
        .get(2)
        .map_err(|e| sqlite_err("read_row:project_root", e))?;
    let anchor: String = row.get(3).map_err(|e| sqlite_err("read_row:anchor", e))?;
    let project_path: String = row
        .get(4)
        .map_err(|e| sqlite_err("read_row:project_path", e))?;
    let project_is_worktree_int: i64 = row
        .get(5)
        .map_err(|e| sqlite_err("read_row:project_is_worktree", e))?;
    let repositories_json: String = row
        .get(6)
        .map_err(|e| sqlite_err("read_row:repositories_json", e))?;

    let project_id = ProjectId::new(&project_id_raw)
        .map_err(|err| TaskFolderStoreError::InvalidInput(err.to_string()))?;
    let repositories: Vec<TaskFolderRepository> =
        serde_json::from_str(&repositories_json).map_err(|e| json_err("decode_repositories", e))?;

    Ok((
        TaskFolder {
            root: root.clone(),
            project_id,
            project_root,
            anchor,
            project_path,
            project_is_worktree: project_is_worktree_int != 0,
            repositories,
        },
        root,
    ))
}

/// Inserts or updates a task folder row (`TaskFolderStore.put`).
pub fn upsert_task_folder(
    conn: &Connection,
    task_folder: &TaskFolder,
    created_at_iso: Option<&str>,
) -> Result<(), TaskFolderStoreError> {
    if task_folder.root.trim().is_empty() {
        return Err(TaskFolderStoreError::InvalidInput(
            "task folder root must not be empty".to_owned(),
        ));
    }
    ensure_task_folders_schema(conn)?;
    let repositories_json = serde_json::to_string(&task_folder.repositories)
        .map_err(|e| json_err("encode_repositories", e))?;
    let real_root = resolve_real_location(Path::new(&task_folder.root))
        .to_string_lossy()
        .into_owned();
    let created_at =
        created_at_iso.map_or_else(|| format_iso8601_utc(SystemTime::now()), str::to_owned);

    conn.execute(
        "INSERT INTO task_folders (
           root, real_root, project_id, project_root, anchor, project_path,
           project_is_worktree, repositories_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (root) DO UPDATE SET
           real_root = excluded.real_root,
           project_path = excluded.project_path,
           project_is_worktree = excluded.project_is_worktree,
           repositories_json = excluded.repositories_json",
        params![
            task_folder.root,
            real_root,
            task_folder.project_id.as_str(),
            task_folder.project_root,
            task_folder.anchor,
            task_folder.project_path,
            i64::from(task_folder.project_is_worktree),
            repositories_json,
            created_at,
        ],
    )
    .map_err(|source| sqlite_err("upsert_task_folder", source))?;

    Ok(())
}

/// Finds the deepest matching task folder whose `root` or `real_root` equals or contains `target`.
pub fn get_by_worktree_or_child_path(
    conn: &Connection,
    target: &Path,
) -> Result<Option<TaskFolder>, TaskFolderStoreError> {
    if !target.is_absolute() {
        return Ok(None);
    }
    ensure_task_folders_schema(conn)?;
    let candidates = collect_ancestor_candidates(target);
    if candidates.is_empty() {
        return Ok(None);
    }
    let placeholders = vec!["?"; candidates.len()].join(", ");
    let sql = format!(
        "SELECT root, project_id, project_root, anchor, project_path, project_is_worktree, repositories_json
         FROM task_folders
         WHERE root IN ({placeholders}) OR real_root IN ({placeholders})
         ORDER BY length(root) DESC
         LIMIT 1"
    );
    let mut doubled: Vec<&str> = candidates.iter().map(String::as_str).collect();
    doubled.extend(candidates.iter().map(String::as_str));

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| sqlite_err("get_by_worktree_or_child_path:prepare", e))?;
    let maybe_row = stmt
        .query_row(rusqlite::params_from_iter(doubled), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .optional()
        .map_err(|e| sqlite_err("get_by_worktree_or_child_path:query", e))?;

    let Some((root, project_id_raw, project_root, anchor, project_path, is_wt, repos_json)) =
        maybe_row
    else {
        return Ok(None);
    };
    let project_id = ProjectId::new(&project_id_raw)
        .map_err(|err| TaskFolderStoreError::InvalidInput(err.to_string()))?;
    let repositories: Vec<TaskFolderRepository> =
        serde_json::from_str(&repos_json).map_err(|e| json_err("decode_repositories", e))?;
    Ok(Some(TaskFolder {
        root,
        project_id,
        project_root,
        anchor,
        project_path,
        project_is_worktree: is_wt != 0,
        repositories,
    }))
}

/// Lists all persisted task folders sorted by `root`.
pub fn list_task_folders(conn: &Connection) -> Result<Vec<TaskFolder>, TaskFolderStoreError> {
    ensure_task_folders_schema(conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT root, project_id, project_root, anchor, project_path, project_is_worktree, repositories_json
             FROM task_folders
             ORDER BY root",
        )
        .map_err(|e| sqlite_err("list_task_folders:prepare", e))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| sqlite_err("list_task_folders:query", e))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| sqlite_err("list_task_folders:next", e))?
    {
        let (folder, _) = decode_row(row)?;
        out.push(folder);
    }
    Ok(out)
}

/// Appends `repository` to the task folder identified by `root` (or resolved real path), enforcing the repository cap.
pub fn add_repository(
    conn: &Connection,
    root: &str,
    repository: TaskFolderRepository,
    max_repositories: Option<usize>,
) -> Result<TaskFolder, TaskFolderStoreError> {
    ensure_task_folders_schema(conn)?;
    let real_root = resolve_real_location(Path::new(root))
        .to_string_lossy()
        .into_owned();
    let mut stmt = conn
        .prepare(
            "SELECT root, project_id, project_root, anchor, project_path, project_is_worktree, repositories_json
             FROM task_folders
             WHERE root = ?1 OR real_root = ?2
             LIMIT 1",
        )
        .map_err(|e| sqlite_err("add_repository:prepare", e))?;
    let mut rows = stmt
        .query(params![root, real_root])
        .map_err(|e| sqlite_err("add_repository:query", e))?;
    let Some(row) = rows
        .next()
        .map_err(|e| sqlite_err("add_repository:next", e))?
    else {
        return Err(TaskFolderStoreError::NotFound {
            root: root.to_owned(),
        });
    };
    let (mut folder, stored_root) = decode_row(row)?;
    drop(rows);
    drop(stmt);

    if folder.repositories.iter().any(|existing| {
        existing.repository_path == repository.repository_path
            || existing.worktree_path == repository.worktree_path
    }) {
        return Ok(folder);
    }

    let limit = max_repositories.unwrap_or(TASK_FOLDER_DEFAULT_REPOSITORIES_MAX);
    if folder.repositories.len() >= limit {
        return Err(TaskFolderStoreError::RepositoryLimitExceeded {
            root: stored_root,
            limit,
        });
    }

    folder.repositories.push(repository);
    let repositories_json = serde_json::to_string(&folder.repositories)
        .map_err(|e| json_err("encode_repositories", e))?;
    conn.execute(
        "UPDATE task_folders SET repositories_json = ?1 WHERE root = ?2",
        params![repositories_json, stored_root],
    )
    .map_err(|e| sqlite_err("add_repository:update", e))?;

    Ok(folder)
}

/// Removes a task folder record by its exact `root` primary key (`true` if a row was deleted).
pub fn remove_by_root(conn: &Connection, root: &str) -> Result<bool, TaskFolderStoreError> {
    ensure_task_folders_schema(conn)?;
    let deleted = conn
        .execute("DELETE FROM task_folders WHERE root = ?1", [root])
        .map_err(|e| sqlite_err("remove_by_root", e))?;
    Ok(deleted > 0)
}
