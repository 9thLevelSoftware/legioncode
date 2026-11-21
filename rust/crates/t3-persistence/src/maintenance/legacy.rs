//! Pre-0.1.0 `state.sqlite` legacy record inspection and safe `t3 doctor --fix` removal.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, ErrorCode, OpenFlags};

use super::{LegacyOnlyRecords, MaintenanceError};

pub(super) fn sibling_legacy_path(database_path: &Path) -> PathBuf {
    match database_path.parent() {
        Some(parent) => parent.join("state.sqlite"),
        None => PathBuf::from("state.sqlite"),
    }
}

pub(super) fn read_legacy_only_records(
    legacy_path: &Path,
    current_v2_path: &Path,
) -> Option<LegacyOnlyRecords> {
    let conn = Connection::open_with_flags(
        legacy_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    query_legacy_only_records_on_conn(&conn, current_v2_path).ok()
}

pub(super) fn lock_exclusively_with<F>(
    conn: &Connection,
    database_path: &Path,
    on_other: F,
) -> Result<(), MaintenanceError>
where
    F: FnOnce(String) -> MaintenanceError,
{
    let run = || -> rusqlite::Result<()> {
        conn.execute_batch("PRAGMA busy_timeout = 0; PRAGMA main.locking_mode = EXCLUSIVE;")?;
        // Commit immediately so VACUUM or close/unlink runs after confirming exclusive lock ownership.
        conn.execute_batch("BEGIN EXCLUSIVE; COMMIT;")
    };
    run().map_err(|err| {
        if matches!(
            err,
            rusqlite::Error::SqliteFailure(ffi_err, _)
                if ffi_err.code == ErrorCode::DatabaseBusy || (ffi_err.extended_code & 0xff) == 5
        ) {
            MaintenanceError::DatabaseInUse {
                path: database_path.to_path_buf(),
            }
        } else {
            on_other(err.to_string())
        }
    })
}

/// Safely removes sibling pre-0.1.0 `state.sqlite` (+ `-wal` and `-shm`) after acquiring an
/// exclusive SQLite lock and verifying that no active projects or threads remain unmigrated
/// (`DatabaseMaintenance.ts:280-316`).
///
/// Returns `Ok(true)` when `state.sqlite` existed and was removed, or `Ok(false)` if absent.
pub fn remove_legacy_database(db_path: &Path) -> Result<bool, MaintenanceError> {
    let legacy = sibling_legacy_path(db_path);
    match legacy.try_exists() {
        Ok(false) => return Ok(false),
        Ok(true) => {}
        Err(source) => {
            return Err(MaintenanceError::Io {
                path: legacy,
                source,
            });
        }
    }

    let conn = Connection::open(&legacy).map_err(|err| MaintenanceError::LegacyDatabaseRemove {
        path: legacy.clone(),
        reason: err.to_string(),
    })?;
    lock_exclusively_with(&conn, &legacy, |reason| {
        MaintenanceError::LegacyDatabaseRemove {
            path: legacy.clone(),
            reason,
        }
    })?;

    let counts = query_legacy_only_records_on_conn(&conn, db_path).map_err(|reason| {
        MaintenanceError::LegacyDatabaseRemove {
            path: legacy.clone(),
            reason,
        }
    })?;
    if counts.projects > 0 || counts.threads > 0 {
        return Err(MaintenanceError::LegacyDatabaseHasUnmigratedRecords {
            projects: counts.projects,
            threads: counts.threads,
        });
    }

    // Close the SQLite handle before unlinking so Windows does not fail with EBUSY/EPERM.
    conn.close()
        .map_err(|(_, err)| MaintenanceError::LegacyDatabaseRemove {
            path: legacy.clone(),
            reason: err.to_string(),
        })?;

    // Delete the main database before -wal/-shm so commits stay intact if an unlink fails.
    let base = legacy.as_os_str();
    for suffix in ["", "-wal", "-shm"] {
        let mut target_os = base.to_os_string();
        target_os.push(suffix);
        let target = PathBuf::from(target_os);
        match fs::remove_file(&target) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(MaintenanceError::LegacyDatabaseRemove {
                    path: legacy,
                    reason: format!("failed removing '{}': {err}", target.display()),
                });
            }
        }
    }

    Ok(true)
}

fn query_legacy_only_records_on_conn(
    conn: &Connection,
    current_v2_path: &Path,
) -> Result<LegacyOnlyRecords, String> {
    match current_v2_path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "current V2 database '{}' does not exist",
                current_v2_path.display()
            ));
        }
        Err(err) => return Err(err.to_string()),
    }

    let current_uri = path_to_sqlite_ro_uri(current_v2_path).map_err(|err| err.to_string())?;
    conn.execute("ATTACH DATABASE ?1 AS current_v2", [&current_uri])
        .map_err(|err| err.to_string())?;
    conn.query_row(
        "SELECT
           (SELECT COUNT(*) FROM main.projection_projects legacy
             WHERE legacy.deleted_at IS NULL AND NOT EXISTS (
               SELECT 1 FROM current_v2.projection_projects current
               WHERE current.project_id = legacy.project_id
             )) AS projects,
           (SELECT COUNT(*) FROM main.projection_threads legacy
             WHERE legacy.deleted_at IS NULL AND NOT EXISTS (
               SELECT 1 FROM current_v2.orchestration_v2_projection_threads current
               WHERE current.thread_id = legacy.thread_id
             )) AS threads",
        [],
        |row| {
            let projects: i64 = row.get(0)?;
            let threads: i64 = row.get(1)?;
            Ok(LegacyOnlyRecords {
                projects: u64::try_from(projects.max(0)).unwrap_or(0),
                threads: u64::try_from(threads.max(0)).unwrap_or(0),
            })
        },
    )
    .map_err(|err| err.to_string())
}

fn path_to_sqlite_ro_uri(path: &Path) -> Result<String, std::io::Error> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let raw = abs.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let forward = stripped.replace('\\', "/");
    let with_leading_slash = if forward.starts_with('/') {
        forward
    } else {
        format!("/{forward}")
    };

    let mut encoded = String::with_capacity(with_leading_slash.len() + 16);
    encoded.push_str("file://");
    for byte in with_leading_slash.bytes() {
        match byte {
            b'%' | b'?' | b'#' | b' ' => {
                use std::fmt::Write;
                let _ = write!(encoded, "%{byte:02X}");
            }
            _ => encoded.push(char::from(byte)),
        }
    }
    encoded.push_str("?mode=ro");
    Ok(encoded)
}
