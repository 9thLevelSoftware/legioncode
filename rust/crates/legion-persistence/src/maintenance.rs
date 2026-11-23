//! Fast read-only database inspection, doctor reporting, and offline database compaction.
//!
//! Mirrors `apps/server/src/persistence/DatabaseMaintenance.ts` and
//! `apps/server/src/cli/doctor.ts`.

mod legacy;
mod prune;
mod prune_search;
pub mod report;
pub mod storage_files;

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

pub use legacy::remove_legacy_database;
pub use prune_search::cleanup_imported_transcript;
pub use report::{
    DATABASE_COMPACTION_RECOMMENDED_BYTES, DoctorOptions, DoctorRepair, DoctorRepairId,
    format_bytes, plan_doctor_repairs, render_doctor_report,
};
pub use storage_files::{
    StorageFileCleanupResult, clean_expired_storage_files, clean_expired_storage_files_with_hook,
};

/// Counts of active records still present in `state.sqlite` that are absent from `statev2.sqlite`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyOnlyRecords {
    /// Non-deleted projects in `state.sqlite` missing from `statev2.sqlite`.
    pub projects: u64,
    /// Non-deleted threads in `state.sqlite` missing from `statev2.sqlite`.
    pub threads: u64,
}

/// Result of [`inspect_database`], matching `DatabaseMaintenance.inspect` in `DatabaseMaintenance.ts:99-108`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseInspection {
    /// Total allocated size (`page_count * page_size`).
    pub database_bytes: u64,
    /// Freelist reclaimable bytes (`freelist_count * page_size`).
    pub reclaimable_bytes: u64,
    /// `true` when `PRAGMA auto_vacuum` equals `2` (`INCREMENTAL`).
    pub incremental_vacuum_enabled: bool,
    /// File size of sibling `state.sqlite` if present, else `None`.
    pub legacy_database_bytes: Option<u64>,
    /// Unmigrated record counts when `state.sqlite` is present and readable, else `None`.
    pub legacy_only_records: Option<LegacyOnlyRecords>,
}

/// Ordered stages executed by [`compact_database_with_steps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseCompactStep {
    /// Remove superseded orchestration events and imported legacy receipts/events.
    PruneEvents,
    /// Switch `auto_vacuum = INCREMENTAL` and `VACUUM` the database file.
    Vacuum,
    /// Rebuild the FTS5 trigram `orchestration_v2_message_search` index after `VACUUM` renumbers rowids.
    RebuildSearch,
    /// Flush and truncate the WAL (`PRAGMA wal_checkpoint(TRUNCATE)`).
    Checkpoint,
}

/// Summary produced by [`compact_database`] and [`compact_database_with_steps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactDatabaseResult {
    /// Database file byte size before compaction.
    pub before_bytes: u64,
    /// Database file byte size after `VACUUM` and `wal_checkpoint(TRUNCATE)`.
    pub after_bytes: u64,
    /// Count of deleted superseded or imported legacy orchestration events.
    pub deleted_event_count: u64,
    /// Count of deleted imported legacy command receipts.
    pub deleted_receipt_count: u64,
}

/// Errors produced by database inspection or compaction.
#[derive(Debug, Error)]
pub enum MaintenanceError {
    /// Target database does not exist.
    #[error("No existing LegionCode database was found in this data directory ({}).", path.display())]
    DatabaseMissing {
        /// Missing database path.
        path: PathBuf,
    },
    /// Another connection or running server currently holds the database open.
    #[error("Stop the LegionCode server and desktop app, including any version before 0.1.0, before repairing the database ({}).", path.display())]
    DatabaseInUse {
        /// Contended database path.
        path: PathBuf,
    },
    /// Failed to inspect the database read-only.
    #[error("Could not read the database at '{}'. Check that this user can open it: {source}", path.display())]
    DatabaseInspect {
        /// Inspected database path.
        path: PathBuf,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
    /// Failed while compacting the database.
    #[error("Database compaction failed at '{}'. Check that no other process has the database open and enough disk space is available: {reason}", path.display())]
    DatabaseCompact {
        /// Target database path.
        path: PathBuf,
        /// Underlying failure details.
        reason: String,
    },
    /// Legacy `state.sqlite` could not be inspected or deleted safely.
    #[error("Could not delete the database from before 0.1.0 at '{}'. Check that both databases are readable and the data directory is writable: {reason}", path.display())]
    LegacyDatabaseRemove {
        /// Legacy `state.sqlite` path.
        path: PathBuf,
        /// Underlying failure details.
        reason: String,
    },
    /// Legacy `state.sqlite` still holds records missing from `statev2.sqlite`.
    #[error(
        "The current database is missing legacy records. Projects missing: {projects}. Threads missing: {threads}. Keep the old database for recovery."
    )]
    LegacyDatabaseHasUnmigratedRecords {
        /// Count of non-deleted legacy projects absent from `statev2.sqlite`.
        projects: u64,
        /// Count of non-deleted legacy threads absent from `statev2.sqlite`.
        threads: u64,
    },
    /// Filesystem I/O failure during locate/stat.
    #[error("database maintenance I/O failed at '{}': {source}", path.display())]
    Io {
        /// Target path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Transcript cleanup failure surfacing the specific FS or SQLite reason (`1458a42419`).
    #[error("Transcript cleanup failed at '{}': {reason}", path.display())]
    TranscriptCleanup {
        /// Target transcript file path.
        path: PathBuf,
        /// Specific FS or SQLite failure reason.
        reason: String,
    },
    /// Storage file cleanup failure (symlinks skipped safely, `#40528`).
    #[error("Storage file cleanup failed at '{}': {message}", path.display())]
    StorageCleanup {
        /// Target directory path.
        path: PathBuf,
        /// Underlying failure message.
        message: String,
    },
}

/// Fast, zero-mutex read-only inspection of `database_path` and any sibling legacy `state.sqlite`.
///
/// Opens SQLite with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX` so inspection has minimal
/// overhead and never mutates WAL or database headers.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use legion_persistence::maintenance::inspect_database;
/// use legion_persistence::sqlite::SqliteStore;
///
/// let dir = tempfile::tempdir()?;
/// let db_path = dir.path().join("statev2.sqlite");
/// SqliteStore::open(&db_path)?.close()?;
///
/// let status = inspect_database(&db_path)?;
/// assert!(status.incremental_vacuum_enabled);
/// assert_eq!(status.reclaimable_bytes, 0);
/// assert_eq!(status.legacy_database_bytes, None);
/// # Ok(())
/// # }
/// ```
pub fn inspect_database(
    database_path: impl AsRef<Path>,
) -> Result<DatabaseInspection, MaintenanceError> {
    let database_path = database_path.as_ref();
    ensure_exists(database_path)?;

    let conn = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|source| MaintenanceError::DatabaseInspect {
        path: database_path.to_path_buf(),
        source,
    })?;

    let (database_bytes, reclaimable_bytes, incremental_vacuum_enabled) = read_storage(&conn)
        .map_err(|source| MaintenanceError::DatabaseInspect {
            path: database_path.to_path_buf(),
            source,
        })?;
    drop(conn);

    let legacy_path = legacy::sibling_legacy_path(database_path);
    let legacy_database_bytes = match fs::metadata(&legacy_path) {
        Ok(meta) => Some(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(MaintenanceError::Io {
                path: legacy_path,
                source,
            });
        }
    };

    let legacy_only_records = legacy_database_bytes
        .and_then(|_| legacy::read_legacy_only_records(&legacy_path, database_path));

    Ok(DatabaseInspection {
        database_bytes,
        reclaimable_bytes,
        incremental_vacuum_enabled,
        legacy_database_bytes,
        legacy_only_records,
    })
}

/// Offline compaction: acquires an exclusive lock (`BEGIN EXCLUSIVE`), prunes obsolete events and
/// imported legacy rows, enables incremental auto-vacuum via `VACUUM`, rebuilds FTS5 search,
/// and truncates the WAL.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use legion_persistence::maintenance::{compact_database, inspect_database};
/// use legion_persistence::sqlite::SqliteStore;
///
/// let dir = tempfile::tempdir()?;
/// let db_path = dir.path().join("statev2.sqlite");
/// let store = SqliteStore::open(&db_path)?;
/// store.connection().execute_batch(
///     "PRAGMA auto_vacuum = NONE; VACUUM;
///      CREATE TABLE junk (b BLOB);
///      INSERT INTO junk VALUES (zeroblob(65536));
///      DROP TABLE junk;"
/// )?;
/// store.close()?;
///
/// let result = compact_database(&db_path)?;
/// assert!(result.after_bytes < result.before_bytes);
/// assert!(inspect_database(&db_path)?.incremental_vacuum_enabled);
/// # Ok(())
/// # }
/// ```
pub fn compact_database(
    database_path: impl AsRef<Path>,
) -> Result<CompactDatabaseResult, MaintenanceError> {
    compact_database_with_steps(database_path, |_| {})
}

/// Runs [`compact_database`] while invoking `on_step` as each compaction phase starts.
pub fn compact_database_with_steps<F>(
    database_path: impl AsRef<Path>,
    mut on_step: F,
) -> Result<CompactDatabaseResult, MaintenanceError>
where
    F: FnMut(DatabaseCompactStep),
{
    let database_path = database_path.as_ref();
    ensure_exists(database_path)?;

    let before_bytes = fs::metadata(database_path)
        .map_err(|source| MaintenanceError::Io {
            path: database_path.to_path_buf(),
            source,
        })?
        .len();

    let mut conn =
        Connection::open(database_path).map_err(|err| MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason: format!("open database: {err}"),
        })?;

    legacy::lock_exclusively_with(&conn, database_path, |reason| {
        MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason,
        }
    })?;

    on_step(DatabaseCompactStep::PruneEvents);
    let (deleted_event_count, deleted_receipt_count) = prune::prune_event_store(&mut conn)
        .map_err(|err| MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason: format!("prune event store: {err}"),
        })?;

    on_step(DatabaseCompactStep::Vacuum);
    conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")
        .map_err(|err| MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason: format!("vacuum: {err}"),
        })?;

    on_step(DatabaseCompactStep::RebuildSearch);
    prune_search::rebuild_message_search(&mut conn).map_err(|err| {
        MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason: format!("rebuild search index: {err}"),
        }
    })?;

    on_step(DatabaseCompactStep::Checkpoint);
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|err| MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason: format!("wal checkpoint: {err}"),
        })?;

    conn.close()
        .map_err(|(_, err)| MaintenanceError::DatabaseCompact {
            path: database_path.to_path_buf(),
            reason: format!("close database: {err}"),
        })?;

    let after_bytes = fs::metadata(database_path)
        .map_err(|source| MaintenanceError::Io {
            path: database_path.to_path_buf(),
            source,
        })?
        .len();

    Ok(CompactDatabaseResult {
        before_bytes,
        after_bytes,
        deleted_event_count,
        deleted_receipt_count,
    })
}

fn ensure_exists(database_path: &Path) -> Result<(), MaintenanceError> {
    match database_path.try_exists() {
        Ok(true) => Ok(()),
        Ok(false) => Err(MaintenanceError::DatabaseMissing {
            path: database_path.to_path_buf(),
        }),
        Err(source) => Err(MaintenanceError::Io {
            path: database_path.to_path_buf(),
            source,
        }),
    }
}

fn read_storage(conn: &Connection) -> rusqlite::Result<(u64, u64, bool)> {
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let auto_vacuum: i64 = conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;

    let page_size_u64 = u64::try_from(page_size.max(0)).unwrap_or(0);
    let page_count_u64 = u64::try_from(page_count.max(0)).unwrap_or(0);
    let freelist_u64 = u64::try_from(freelist_count.max(0)).unwrap_or(0);

    Ok((
        page_count_u64.saturating_mul(page_size_u64),
        freelist_u64.saturating_mul(page_size_u64),
        auto_vacuum == 2,
    ))
}
