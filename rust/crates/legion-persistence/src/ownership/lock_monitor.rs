//! SQLite ownership lock acquisition, inode identity tracking, and monitor spawning.

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags};
use tokio::sync::{Notify, watch};

use super::OwnershipError;

const LOCK_BUSY_TIMEOUT_SQL: &str = "PRAGMA busy_timeout = 250; BEGIN EXCLUSIVE;";

/// Cooperative cancellation fence triggered if the ownership lock file is deleted or replaced.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() {
/// use legion_persistence::ownership::CancellationToken;
///
/// let token = CancellationToken::new();
/// assert!(!token.is_cancelled());
/// let clone = token.clone();
/// token.cancel();
/// clone.cancelled().await;
/// assert!(clone.is_cancelled());
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct CancellationToken {
    tx: Arc<watch::Sender<bool>>,
    rx: watch::Receiver<bool>,
}

impl CancellationToken {
    /// Creates an active, uncancelled fence token.
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            tx: Arc::new(tx),
            rx,
        }
    }

    /// Cancels this token and wakes all awaiting tasks.
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }

    /// Returns `true` once [`CancellationToken::cancel`] has been called.
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolves immediately if already cancelled, or awaits cancellation without polling.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut rx = self.rx.clone();
        let _ = rx.wait_for(|cancelled| *cancelled).await;
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
    #[cfg(windows)]
    file_size: u64,
}

impl FileIdentity {
    pub(super) fn from_metadata(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                dev: meta.dev(),
                ino: meta.ino(),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            Self {
                creation_time: meta.creation_time(),
                last_write_time: meta.last_write_time(),
                file_size: meta.file_size(),
            }
        }
    }
}

pub(super) enum MonitorHandle {
    Tokio {
        stop: Arc<Notify>,
        stopped: Arc<AtomicBool>,
    },
    Thread {
        stop_pair: Arc<(Mutex<bool>, Condvar)>,
        join: Option<JoinHandle<()>>,
    },
}

impl MonitorHandle {
    pub(super) fn stop(&mut self) {
        match self {
            Self::Tokio { stop, stopped } => {
                stopped.store(true, Ordering::Release);
                stop.notify_waiters();
            }
            Self::Thread { stop_pair, join } => {
                let (lock, cvar) = &**stop_pair;
                if let Ok(mut guard) = lock.lock() {
                    *guard = true;
                    cvar.notify_all();
                }
                if let Some(handle) = join.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

pub(super) fn resolve_paths(input: &Path) -> (PathBuf, PathBuf) {
    if input.is_dir() {
        return (input.to_path_buf(), input.join("server-runtime.json"));
    }
    if input.extension().and_then(|ext| ext.to_str()) == Some("json") {
        let dir = match input.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        return (dir, input.to_path_buf());
    }
    (input.to_path_buf(), input.join("server-runtime.json"))
}

pub(super) fn state_dir_owned_error(state_dir: &Path) -> OwnershipError {
    OwnershipError::StateDirOwned {
        state_dir: state_dir.to_path_buf(),
        detail: format!(
            "A LegionCode server already owns {}. Stop that server, or use a separate LegionCode home and pair with it, then retry. No server was stopped. server-owner.sqlite is the ownership lock; do not delete it while a server is running.",
            state_dir.display()
        ),
    }
}

pub(super) fn open_ownership_lock(
    state_dir: &Path,
) -> Result<(Connection, PathBuf, FileIdentity, bool), OwnershipError> {
    let lock_path = state_dir.join("server-owner.sqlite");
    fs::create_dir_all(state_dir).map_err(|source| OwnershipError::Io {
        operation: "create_dir_all",
        path: state_dir.to_path_buf(),
        source,
    })?;

    let before = match fs::metadata(&lock_path) {
        Ok(meta) => FileIdentity::from_metadata(&meta),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            // Close the pre-created handle before opening SQLite so POSIX fcntl locks never drop.
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&lock_path)
                .map_err(|source| OwnershipError::Io {
                    operation: "create_lock_file",
                    path: lock_path.clone(),
                    source,
                })?;
            drop(file);
            let meta = fs::metadata(&lock_path).map_err(|source| OwnershipError::Io {
                operation: "stat_lock_file",
                path: lock_path.clone(),
                source,
            })?;
            FileIdentity::from_metadata(&meta)
        }
        Err(source) => {
            return Err(OwnershipError::Io {
                operation: "stat_lock_file",
                path: lock_path,
                source,
            });
        }
    };

    let conn = Connection::open_with_flags(
        &lock_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|source| map_lock_sqlite_error(state_dir, &lock_path, source))?;

    if let Err(err) = conn.execute_batch(LOCK_BUSY_TIMEOUT_SQL) {
        drop(conn);
        return Err(map_lock_sqlite_error(state_dir, &lock_path, err));
    }

    let after_meta = fs::metadata(&lock_path).map_err(|source| OwnershipError::Io {
        operation: "stat_lock_file",
        path: lock_path.clone(),
        source,
    })?;
    let identity = FileIdentity::from_metadata(&after_meta);
    let replaced_during_acquire = before != identity;

    Ok((conn, lock_path, identity, replaced_during_acquire))
}

fn map_lock_sqlite_error(
    state_dir: &Path,
    lock_path: &Path,
    source: rusqlite::Error,
) -> OwnershipError {
    if is_sqlite_busy(&source) {
        state_dir_owned_error(state_dir)
    } else {
        OwnershipError::Sqlite {
            path: lock_path.to_path_buf(),
            source,
        }
    }
}

fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(ffi_err, _)
            if ffi_err.code == ErrorCode::DatabaseBusy || (ffi_err.extended_code & 0xff) == 5
    )
}

/// Returns whether a running server currently holds `server-owner.sqlite` (`#28256`).
///
/// Read-only so probing never creates the lock file when absent.
pub fn is_server_ownership_held(path: impl AsRef<Path>) -> bool {
    let (state_dir, _) = resolve_paths(path.as_ref());
    let lock_path = state_dir.join("server-owner.sqlite");
    let Ok(conn) = Connection::open_with_flags(
        &lock_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    let _ = conn.busy_timeout(Duration::ZERO);
    let mut stmt = match conn.prepare("SELECT 1 FROM sqlite_schema") {
        Ok(stmt) => stmt,
        Err(err) => return is_sqlite_busy(&err),
    };
    match stmt.exists([]) {
        Ok(_) => false,
        Err(err) => is_sqlite_busy(&err),
    }
}

pub(super) fn check_lock_lost(lock_path: &Path, expected: FileIdentity) -> bool {
    match fs::metadata(lock_path) {
        Ok(meta) => FileIdentity::from_metadata(&meta) != expected,
        Err(err) if err.kind() == ErrorKind::NotFound => true,
        // Keep watching on transient permission/stat errors just like serverOwnership.ts:269-277.
        Err(_) => false,
    }
}

pub(super) fn spawn_lock_monitor(
    lock_path: PathBuf,
    identity: FileIdentity,
    fence: CancellationToken,
    poll_interval: Duration,
    already_lost: bool,
) -> MonitorHandle {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let stop = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let stopped_clone = Arc::clone(&stopped);
        handle.spawn(async move {
            if already_lost {
                fence.cancel();
                return;
            }
            while !stopped_clone.load(Ordering::Acquire) {
                tokio::select! {
                    () = tokio::time::sleep(poll_interval) => {}
                    () = stop_clone.notified() => break,
                }
                if stopped_clone.load(Ordering::Acquire) {
                    break;
                }
                if check_lock_lost(&lock_path, identity) {
                    fence.cancel();
                    break;
                }
            }
        });
        MonitorHandle::Tokio { stop, stopped }
    } else {
        let stop_pair = Arc::new((Mutex::new(false), Condvar::new()));
        let pair_clone = Arc::clone(&stop_pair);
        let join = std::thread::spawn(move || {
            if already_lost {
                fence.cancel();
                return;
            }
            let (lock, cvar) = &*pair_clone;
            if let Ok(mut stopped) = lock.lock() {
                while !*stopped {
                    let Ok((guard, _)) = cvar.wait_timeout(stopped, poll_interval) else {
                        break;
                    };
                    stopped = guard;
                    if *stopped {
                        break;
                    }
                    if check_lock_lost(&lock_path, identity) {
                        fence.cancel();
                        break;
                    }
                }
            }
        });
        MonitorHandle::Thread {
            stop_pair,
            join: Some(join),
        }
    }
}
