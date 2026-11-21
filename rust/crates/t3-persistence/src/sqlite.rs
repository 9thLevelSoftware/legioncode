//! SQLite WAL storage initialization, exact PRAGMAs, damaged-DB detection, and migration head.
//!
//! Mirrors `apps/server/src/persistence/Sqlite.ts` and
//! `apps/server/src/persistence/databaseMigrationHead.ts`.

pub(crate) mod auth_peer_schema;
mod migration_head;
pub(crate) mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

pub use migration_head::{RecordedMigration, read_migration_head, read_recorded_migrations};

/// Maximum WAL size retained after checkpoints (`128 MiB`, matching `Sqlite.ts:16`).
pub const WAL_SIZE_LIMIT_BYTES: i64 = 128 * 1024 * 1024;

/// Latest migration ID known to this build (`Migrations.ts:217`).
pub const BUILD_MIGRATION_HEAD: i64 = 102;

/// Primary SQLite error codes indicating database file corruption or a non-SQLite file.
///
/// `11` = `SQLITE_CORRUPT`, `26` = `SQLITE_NOTADB`.
pub const fn is_damaged_sqlite_code(code: i32) -> bool {
    matches!(code & 0xff, 11 | 26)
}

/// Returns `true` when `err` carries an extended SQLite code whose low byte is `11` or `26`.
pub fn is_damaged_sqlite_error(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(ffi_err, _)
            if is_damaged_sqlite_code(ffi_err.extended_code)
    )
}

/// Options controlling [`SqliteStore::open_with_options`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqliteOpenOptions {
    /// When `true` (default), ensures core V2 DDL tables, indexes, and FTS5 triggers exist.
    pub ensure_core_schema: bool,
}

impl Default for SqliteOpenOptions {
    fn default() -> Self {
        Self {
            ensure_core_schema: true,
        }
    }
}

/// Errors produced while opening, configuring, or inspecting a SQLite database store.
#[derive(Debug, Error)]
pub enum SqliteStoreError {
    /// Failed to create parent directories for the database file.
    #[error("failed to create database directory '{}': {source}", path.display())]
    CreateDirectory {
        /// Target parent directory.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Database file is corrupt (`SQLITE_CORRUPT` = 11) or not a SQLite database (`SQLITE_NOTADB` = 26).
    #[error("{message}")]
    DamagedDatabase {
        /// Path to the damaged database file.
        path: PathBuf,
        /// User-actionable recovery guidance matching `Sqlite.ts:49`.
        message: String,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// Any other SQLite failure while opening, applying PRAGMAs, or running DDL.
    #[error("SQLite error for '{}': {source}", path.display())]
    Sqlite {
        /// Database path (or `:memory:`).
        path: PathBuf,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
}

/// Configured SQLite store enforcing T3 Code's WAL PRAGMAs, core V2 schema, and stat cleanup.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::sqlite::SqliteStore;
///
/// let dir = tempfile::tempdir()?;
/// let store = SqliteStore::open(dir.path().join("statev2.sqlite"))?;
/// let mode: String = store
///     .connection()
///     .query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
/// assert_eq!(mode.to_ascii_lowercase(), "wal");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SqliteStore {
    conn: Connection,
    path: Option<PathBuf>,
}

impl SqliteStore {
    /// Opens (or creates) `db_path`, applies exact startup PRAGMAs, creates core V2 DDL tables,
    /// and clears stale `sqlite_stat1` planner statistics.
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, SqliteStoreError> {
        Self::open_with_options(db_path, SqliteOpenOptions::default())
    }

    /// Opens (or creates) `db_path` with explicit [`SqliteOpenOptions`].
    pub fn open_with_options(
        db_path: impl AsRef<Path>,
        options: SqliteOpenOptions,
    ) -> Result<Self, SqliteStoreError> {
        let db_path = db_path.as_ref();
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| SqliteStoreError::CreateDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let conn = Connection::open(db_path).map_err(|err| map_sqlite_error(db_path, err))?;
        let store = Self {
            conn,
            path: Some(db_path.to_path_buf()),
        };
        store.configure(db_path, options)?;
        Ok(store)
    }

    /// Opens an in-memory SQLite store with the same PRAGMAs and core V2 DDL tables.
    pub fn open_in_memory() -> Result<Self, SqliteStoreError> {
        let virtual_path = Path::new(":memory:");
        let conn =
            Connection::open_in_memory().map_err(|err| map_sqlite_error(virtual_path, err))?;
        let store = Self { conn, path: None };
        store.configure(virtual_path, SqliteOpenOptions::default())?;
        Ok(store)
    }

    /// Returns a shared reference to the underlying `rusqlite::Connection`.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Returns a mutable reference to the underlying `rusqlite::Connection`.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Consumes the store and returns the underlying `rusqlite::Connection`.
    pub fn into_connection(self) -> Connection {
        self.conn
    }

    /// Returns the filesystem path of the opened database, or `None` for `:memory:`.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Ensures core V2 tables, indexes, and FTS5 trigram message search triggers exist.
    pub fn ensure_core_v2_schema(&self) -> Result<(), SqliteStoreError> {
        let path = self
            .path
            .as_deref()
            .unwrap_or_else(|| Path::new(":memory:"));
        schema::apply_core_v2_ddl(&self.conn).map_err(|err| map_sqlite_error(path, err))
    }

    /// Drops cached planner statistics from `sqlite_stat1` if present (`Sqlite.ts:93-97`).
    pub fn drop_planner_statistics(&self) -> Result<(), SqliteStoreError> {
        let path = self.display_path();
        let has_stat1 = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'sqlite_stat1'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .is_ok();
        if has_stat1 {
            self.conn
                .execute_batch("DELETE FROM sqlite_stat1; ANALYZE sqlite_schema;")
                .map_err(|err| map_sqlite_error(path, err))?;
        }
        Ok(())
    }

    /// Reads `MAX(migration_id)` from `effect_sql_migrations`, or `Ok(None)` if empty/absent.
    pub fn recorded_migration_head(&self) -> Result<Option<i64>, SqliteStoreError> {
        let path = self.display_path();
        let has_table = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'effect_sql_migrations'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .is_ok();
        if !has_table {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT MAX(migration_id) FROM effect_sql_migrations",
                [],
                |r| r.get(0),
            )
            .map_err(|err| map_sqlite_error(path, err))
    }

    /// Explicitly closes the underlying SQLite connection, surfacing any SQLite close error.
    pub fn close(self) -> Result<(), SqliteStoreError> {
        let path = self.path.unwrap_or_else(|| PathBuf::from(":memory:"));
        self.conn
            .close()
            .map_err(|(_, err)| map_sqlite_error(&path, err))
    }

    fn display_path(&self) -> &Path {
        self.path
            .as_deref()
            .unwrap_or_else(|| Path::new(":memory:"))
    }

    fn configure(
        &self,
        display_path: &Path,
        options: SqliteOpenOptions,
    ) -> Result<(), SqliteStoreError> {
        apply_startup_pragmas(&self.conn).map_err(|err| map_sqlite_error(display_path, err))?;
        if options.ensure_core_schema {
            schema::apply_core_v2_ddl(&self.conn)
                .map_err(|err| map_sqlite_error(display_path, err))?;
        }
        self.drop_planner_statistics()
    }
}

fn apply_startup_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")?;
    let table_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table'",
        [],
        |row| row.get(0),
    )?;
    if table_count == 0 {
        conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL;")?;
    }
    let pragmas = format!(
        "PRAGMA journal_mode = WAL;\nPRAGMA synchronous = NORMAL;\nPRAGMA checkpoint_fullfsync = ON;\nPRAGMA temp_store = MEMORY;\nPRAGMA journal_size_limit = {WAL_SIZE_LIMIT_BYTES};"
    );
    conn.execute_batch(&pragmas)
}

fn map_sqlite_error(path: &Path, source: rusqlite::Error) -> SqliteStoreError {
    if is_damaged_sqlite_error(&source) {
        SqliteStoreError::DamagedDatabase {
            path: path.to_path_buf(),
            message: format!(
                "The T3 Code database at {} is damaged or is not a SQLite database, so T3 Code cannot start. To recover, quit T3 Code, then move that file and any -wal and -shm files next to it out of the folder, or replace them with a backup. T3 Code creates a new database on the next start.",
                path.display()
            ),
            source,
        }
    } else {
        SqliteStoreError::Sqlite {
            path: path.to_path_buf(),
            source,
        }
    }
}
