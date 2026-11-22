//! File paths and exclusive SQLite lock probes for service launcher ownership.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, ErrorCode};

use crate::ownership::{legacy_owner_is_live, read_persisted_server_runtime_state};
use crate::paths::ServerDerivedPaths;

use super::{
    SERVICE_LAUNCHER_LOCK_FILE, SERVICE_SERVING_LOCK_FILE, SERVICE_STATE_FILE, ServiceLauncherError,
};

/// Returns `<base_dir>/runtime/service-launcher.lock` (`serviceLauncher.ts:760-761`).
pub fn service_launcher_lock_path(base_dir: &Path) -> PathBuf {
    base_dir.join("runtime").join(SERVICE_LAUNCHER_LOCK_FILE)
}

/// Returns `<base_dir>/runtime/service-serving.lock` (`serviceLauncher.ts:787-788`).
pub fn service_serving_lock_path(base_dir: &Path) -> PathBuf {
    base_dir.join("runtime").join(SERVICE_SERVING_LOCK_FILE)
}

/// Returns `<base_dir>/runtime/service-state.json` (`serviceLauncher.ts:901`).
pub fn service_state_path(base_dir: &Path) -> PathBuf {
    base_dir.join("runtime").join(SERVICE_STATE_FILE)
}

/// Probes whether `lock_path` exists and is currently held via an exclusive SQLite transaction
/// (`serviceLauncher.ts:780-785`).
pub fn service_launcher_is_running(lock_path: &Path) -> bool {
    if !lock_path.exists() {
        return false;
    }
    matches!(try_acquire_sqlite_lock(lock_path, 0), Ok(None))
}

/// Returns `true` when a launcher currently holds `service-serving.lock` (`serviceLauncher.ts:791-792`).
pub fn service_launcher_is_serving(base_dir: &Path) -> bool {
    service_launcher_is_running(&service_serving_lock_path(base_dir))
}

/// Returns `true` when `base_dir`'s state directory is actively owned by a live server lock or a
/// pre-lock legacy runtime PID (`serviceLauncher.ts:807-847`).
pub fn state_dir_owned(base_dir: &Path) -> bool {
    let derived = ServerDerivedPaths::derive(base_dir, false, true);
    let lock_path = derived.state_dir.join("server-owner.sqlite");
    let runtime_path = derived.state_dir.join("server-runtime.json");
    if lock_path.exists() {
        match try_acquire_sqlite_lock(&lock_path, 0) {
            Ok(None) => return true,
            Ok(Some(guard)) => drop(guard),
            Err(_) => return false,
        }
    } else if !derived.state_dir.exists() {
        return false;
    }

    let Ok(Some(record)) = read_persisted_server_runtime_state(&runtime_path) else {
        return false;
    };
    if record.owner_id.is_some() {
        return false;
    }
    legacy_owner_is_live(&record)
}

/// Held exclusive SQLite lock guard (released when dropped).
#[derive(Debug)]
pub struct ServiceLockGuard {
    conn: Connection,
    lock_path: PathBuf,
}

impl ServiceLockGuard {
    /// Acquires `BEGIN EXCLUSIVE` on `lock_path` (creating parent directories if needed), returning
    /// `Ok(None)` on `SQLITE_BUSY` contention.
    pub fn try_acquire(
        lock_path: &Path,
        busy_timeout_ms: u32,
    ) -> Result<Option<Self>, ServiceLauncherError> {
        if let Some(parent) = lock_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| ServiceLauncherError::Io {
                operation: "create_lock_parent_dir",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        try_acquire_sqlite_lock(lock_path, busy_timeout_ms)
    }

    /// Returns the filesystem path of the held lock database.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for ServiceLockGuard {
    fn drop(&mut self) {
        let _ = self.conn.execute_batch("ROLLBACK;");
    }
}

fn try_acquire_sqlite_lock(
    lock_path: &Path,
    busy_timeout_ms: u32,
) -> Result<Option<ServiceLockGuard>, ServiceLauncherError> {
    let conn = Connection::open(lock_path).map_err(|source| ServiceLauncherError::Sqlite {
        operation: "open_service_lock",
        path: lock_path.to_path_buf(),
        source,
    })?;
    let sql = format!("PRAGMA busy_timeout = {busy_timeout_ms}; BEGIN EXCLUSIVE;");
    match conn.execute_batch(&sql) {
        Ok(()) => Ok(Some(ServiceLockGuard {
            conn,
            lock_path: lock_path.to_path_buf(),
        })),
        Err(err) if is_busy_error(&err) => Ok(None),
        Err(source) => Err(ServiceLauncherError::Sqlite {
            operation: "begin_exclusive_service_lock",
            path: lock_path.to_path_buf(),
            source,
        }),
    }
}

fn is_busy_error(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(ffi_err, _)
            if ffi_err.code == ErrorCode::DatabaseBusy || (ffi_err.extended_code & 0xff) == 5
    )
}
