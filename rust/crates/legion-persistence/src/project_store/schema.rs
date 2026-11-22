//! Schema bootstrap, column inspection, and connection/transaction helpers for `project_store`.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::sqlite::{SqliteOpenOptions, SqliteStore};

use super::ProjectStoreError;

const CREATE_PROJECTION_PROJECTS_SQL: &str = "
CREATE TABLE IF NOT EXISTS projection_projects (
  project_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  workspace_root TEXT NOT NULL,
  scripts_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT,
  default_model_selection_json TEXT,
  default_thread_env_mode TEXT,
  favicon_path TEXT,
  auto_pull INTEGER NOT NULL DEFAULT 0,
  project_icon_json TEXT,
  pinned_at TEXT,
  archived_at TEXT,
  has_custom_title INTEGER NOT NULL DEFAULT 0,
  tracked_folders_json TEXT NOT NULL DEFAULT '[]'
);
";

const COMPAT_COLUMNS: &[(&str, &str)] = &[
    ("title", "TEXT NOT NULL DEFAULT ''"),
    ("workspace_root", "TEXT NOT NULL DEFAULT ''"),
    ("scripts_json", "TEXT NOT NULL DEFAULT '[]'"),
    ("created_at", "TEXT NOT NULL DEFAULT ''"),
    ("updated_at", "TEXT NOT NULL DEFAULT ''"),
    ("deleted_at", "TEXT"),
    ("default_model_selection_json", "TEXT"),
    ("default_thread_env_mode", "TEXT"),
    ("favicon_path", "TEXT"),
    ("auto_pull", "INTEGER NOT NULL DEFAULT 0"),
    ("project_icon_json", "TEXT"),
    ("pinned_at", "TEXT"),
    ("archived_at", "TEXT"),
    ("has_custom_title", "INTEGER NOT NULL DEFAULT 0"),
    ("tracked_folders_json", "TEXT NOT NULL DEFAULT '[]'"),
];

pub(super) struct WriteTx<'a> {
    conn: &'a Connection,
    savepoint: bool,
    committed: bool,
}

impl<'a> WriteTx<'a> {
    pub(super) fn begin(conn: &'a Connection) -> Result<Self, ProjectStoreError> {
        let savepoint = !conn.is_autocommit();
        let sql = if savepoint {
            "SAVEPOINT legion_project_store;"
        } else {
            "BEGIN IMMEDIATE;"
        };
        conn.execute_batch(sql)
            .map_err(|source| ProjectStoreError::Sqlite {
                operation: "begin_transaction",
                source,
            })?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    pub(super) fn commit(mut self) -> Result<(), ProjectStoreError> {
        let sql = if self.savepoint {
            "RELEASE SAVEPOINT legion_project_store;"
        } else {
            "COMMIT;"
        };
        self.conn
            .execute_batch(sql)
            .map_err(|source| ProjectStoreError::Sqlite {
                operation: "commit_transaction",
                source,
            })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for WriteTx<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let sql = if self.savepoint {
                "ROLLBACK TO SAVEPOINT legion_project_store; RELEASE SAVEPOINT legion_project_store;"
            } else {
                "ROLLBACK;"
            };
            let _ = self.conn.execute_batch(sql);
        }
    }
}

/// Ensures `projection_projects` and its indexes exist on `conn`, adding any missing V2 columns
/// when run against an older partial database.
pub fn ensure_project_store_schema(conn: &Connection) -> Result<(), ProjectStoreError> {
    conn.execute_batch(CREATE_PROJECTION_PROJECTS_SQL)
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "ensure_projection_projects_table",
            source,
        })?;

    let existing = projection_project_columns(conn)?;
    for &(column_name, column_def) in COMPAT_COLUMNS {
        if !existing.contains(column_name) {
            let alter =
                format!("ALTER TABLE projection_projects ADD COLUMN {column_name} {column_def}");
            conn.execute_batch(&alter)
                .map_err(|source| ProjectStoreError::Sqlite {
                    operation: "alter_projection_projects_column",
                    source,
                })?;
        }
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_projection_projects_updated_at ON projection_projects(updated_at);
         CREATE INDEX IF NOT EXISTS idx_projection_projects_workspace_root_deleted_at ON projection_projects(workspace_root, deleted_at);",
    )
    .map_err(|source| ProjectStoreError::Sqlite {
        operation: "ensure_projection_projects_indexes",
        source,
    })?;

    Ok(())
}

pub(super) fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, ProjectStoreError> {
    match conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table_name],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(source) => Err(ProjectStoreError::Sqlite {
            operation: "check_table_exists",
            source,
        }),
    }
}

pub(super) fn projection_project_columns(
    conn: &Connection,
) -> Result<HashSet<String>, ProjectStoreError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(projection_projects)")
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "pragma_table_info_prepare",
            source,
        })?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "pragma_table_info_query",
            source,
        })?;

    let mut cols = HashSet::new();
    for row in rows {
        cols.insert(row.map_err(|source| ProjectStoreError::Sqlite {
            operation: "pragma_table_info_row",
            source,
        })?);
    }
    Ok(cols)
}

pub(super) fn open_read_only(path: &Path) -> Result<Option<Connection>, ProjectStoreError> {
    if path != Path::new(":memory:") && !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|source| ProjectStoreError::Sqlite {
        operation: "open_read_only",
        source,
    })?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")
        .map_err(|source| ProjectStoreError::Sqlite {
            operation: "read_only_busy_timeout",
            source,
        })?;
    Ok(Some(conn))
}

pub(super) fn open_read_write(path: &Path) -> Result<SqliteStore, ProjectStoreError> {
    SqliteStore::open_with_options(
        path,
        SqliteOpenOptions {
            ensure_core_schema: false,
        },
    )
    .map_err(|source| ProjectStoreError::OpenDatabase {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}
