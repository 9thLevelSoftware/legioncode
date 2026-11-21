//! Single-FD SQLite state-directory ownership lock with active lock-loss fencing (BUG-05 fix).
//!
//! Mirrors `apps/server/src/serverOwnership.ts` and `apps/server/src/serverRuntimeState.ts`:
//! holds a single non-cloned `rusqlite::Connection` with `PRAGMA busy_timeout = 250; BEGIN EXCLUSIVE;`
//! on `<state_dir>/server-owner.sqlite` and monitors the lock file via path-only `stat` so any
//! deletion or inode replacement immediately cancels the active [`CancellationToken`] fence.

mod liveness;
mod lock_monitor;
mod runtime_state;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::Connection;
use thiserror::Error;
use uuid::Uuid;

pub use liveness::{
    format_epoch_millis_utc, format_iso8601_utc, is_process_alive, legacy_owner_is_live,
    parse_iso8601_utc_millis,
};
pub use lock_monitor::CancellationToken;
use lock_monitor::{
    FileIdentity, MonitorHandle, check_lock_lost, open_ownership_lock, resolve_paths,
    spawn_lock_monitor, state_dir_owned_error,
};
pub use runtime_state::{
    ManagedPolicyRecord, ManagedPolicyState, PersistedServerRuntimeState,
    clear_persisted_server_runtime_state, persist_server_runtime_state,
    read_live_server_runtime_state, read_persisted_server_runtime_state, runtime_origin_for_host,
};

/// Process exit code signaling that another server or legacy service owns the state directory.
///
/// Matches `SERVER_EXIT_CODE_STATE_DIR_OWNED` in `packages/contracts/src/desktopBootstrap.ts:6`.
pub const SERVER_EXIT_CODE_STATE_DIR_OWNED: i32 = 78;

const DEFAULT_LOCK_MONITOR_INTERVAL: Duration = Duration::from_secs(5);

/// Errors produced when acquiring, publishing, verifying, or releasing server directory ownership.
#[derive(Debug, Error)]
pub enum OwnershipError {
    /// Another running server or pre-lock process owns the target state directory (exit code `78`).
    #[error("{detail}")]
    StateDirOwned {
        /// Contended state directory.
        state_dir: PathBuf,
        /// Human-readable diagnostic matching `serverOwnership.ts`.
        detail: String,
    },
    /// Filesystem failure while preparing or verifying ownership state.
    #[error("ownership filesystem {operation} failed at '{}': {source}", path.display())]
    Io {
        /// Operation name.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// SQLite failure while opening or locking `server-owner.sqlite`.
    #[error("could not acquire server ownership lock at '{}': {source}", path.display())]
    Sqlite {
        /// Lock database path.
        path: PathBuf,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// Failed to persist, decode, or clear `server-runtime.json`.
    #[error("failed to {operation} server runtime state at '{}': {message}", path.display())]
    RuntimeState {
        /// Operation (`persist`, `read`, `decode`, or `clear`).
        operation: &'static str,
        /// Runtime state JSON path.
        path: PathBuf,
        /// Failure details.
        message: String,
    },
    /// Ownership lock file was deleted or replaced after acquisition.
    #[error("server ownership lock at '{}' was deleted or replaced", lock_path.display())]
    LockLost {
        /// Path to `server-owner.sqlite`.
        lock_path: PathBuf,
    },
}

impl OwnershipError {
    /// Returns [`SERVER_EXIT_CODE_STATE_DIR_OWNED`] (`78`) when the state directory is already owned.
    pub const fn exit_code(&self) -> Option<i32> {
        match self {
            Self::StateDirOwned { .. } => Some(SERVER_EXIT_CODE_STATE_DIR_OWNED),
            Self::Io { .. }
            | Self::Sqlite { .. }
            | Self::RuntimeState { .. }
            | Self::LockLost { .. } => None,
        }
    }
}

/// Exclusive state-directory ownership lock backed by a single non-cloned SQLite connection.
///
/// Releasing or dropping the lock clears `server-runtime.json` only when its `ownerId` matches
/// this lock instance (for [`ServerOwnerLock::acquire`]), then closes `server-owner.sqlite`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::ownership::{OwnershipError, ServerOwnerLock};
///
/// let dir = tempfile::tempdir()?;
/// let runtime_path = dir.path().join("server-runtime.json");
/// let owner = ServerOwnerLock::acquire(&runtime_path)?;
/// let err = ServerOwnerLock::acquire(&runtime_path).unwrap_err();
/// assert_eq!(err.exit_code(), Some(78));
/// assert!(matches!(err, OwnershipError::StateDirOwned { .. }));
/// owner.release()?;
/// # Ok(())
/// # }
/// ```
pub struct ServerOwnerLock {
    state_dir: PathBuf,
    lock_path: PathBuf,
    runtime_state_path: PathBuf,
    owner_id: Option<String>,
    identity: FileIdentity,
    fence: CancellationToken,
    conn: Mutex<Option<Connection>>,
    monitor: Option<MonitorHandle>,
}

impl std::fmt::Debug for ServerOwnerLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerOwnerLock")
            .field("state_dir", &self.state_dir)
            .field("lock_path", &self.lock_path)
            .field("owner_id", &self.owner_id)
            .field("fenced", &self.fence.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl ServerOwnerLock {
    /// Acquires exclusive ownership of the state directory and monitors lock integrity every 5s.
    ///
    /// Accepts either `<state_dir>` or `<state_dir>/server-runtime.json`.
    pub fn acquire(state_dir_or_runtime_path: impl AsRef<Path>) -> Result<Self, OwnershipError> {
        Self::acquire_with_poll_interval(state_dir_or_runtime_path, DEFAULT_LOCK_MONITOR_INTERVAL)
    }

    /// Acquires exclusive ownership using a custom lock-file monitor `poll_interval`.
    pub fn acquire_with_poll_interval(
        state_dir_or_runtime_path: impl AsRef<Path>,
        poll_interval: Duration,
    ) -> Result<Self, OwnershipError> {
        let (state_dir, runtime_state_path) = resolve_paths(state_dir_or_runtime_path.as_ref());
        let owner_id = Uuid::new_v4().to_string();
        let (conn, lock_path, identity, replaced_during_acquire) = open_ownership_lock(&state_dir)?;

        if let Ok(Some(previous)) = read_persisted_server_runtime_state(&runtime_state_path)
            && previous.owner_id.is_none()
            && legacy_owner_is_live(&previous)
        {
            drop(conn);
            return Err(state_dir_owned_error(&state_dir));
        }

        let fence = CancellationToken::new();
        if replaced_during_acquire {
            fence.cancel();
        }
        let monitor = spawn_lock_monitor(
            lock_path.clone(),
            identity,
            fence.clone(),
            poll_interval,
            replaced_during_acquire,
        );

        Ok(Self {
            state_dir,
            lock_path,
            runtime_state_path,
            owner_id: Some(owner_id),
            identity,
            fence,
            conn: Mutex::new(Some(conn)),
            monitor: Some(monitor),
        })
    }

    /// Holds only `<state_dir>/server-owner.sqlite` without reading or clearing `server-runtime.json`.
    ///
    /// Matches `holdServerOwnershipLock` in `apps/server/src/serverOwnership.ts:211-217`.
    pub fn hold_lock_only(
        state_dir_or_runtime_path: impl AsRef<Path>,
    ) -> Result<Self, OwnershipError> {
        let (state_dir, runtime_state_path) = resolve_paths(state_dir_or_runtime_path.as_ref());
        let (conn, lock_path, identity, replaced_during_acquire) = open_ownership_lock(&state_dir)?;
        let fence = CancellationToken::new();
        if replaced_during_acquire {
            fence.cancel();
        }
        let monitor = spawn_lock_monitor(
            lock_path.clone(),
            identity,
            fence.clone(),
            DEFAULT_LOCK_MONITOR_INTERVAL,
            replaced_during_acquire,
        );

        Ok(Self {
            state_dir,
            lock_path,
            runtime_state_path,
            owner_id: None,
            identity,
            fence,
            conn: Mutex::new(Some(conn)),
            monitor: Some(monitor),
        })
    }

    /// Returns the unique `ownerId` assigned by [`ServerOwnerLock::acquire`], or `None` for
    /// [`ServerOwnerLock::hold_lock_only`].
    pub fn owner_id(&self) -> Option<&str> {
        self.owner_id.as_deref()
    }

    /// Returns the owned state directory path.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Returns the path to `server-owner.sqlite`.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Returns the path to `server-runtime.json`.
    pub fn runtime_state_path(&self) -> &Path {
        &self.runtime_state_path
    }

    /// Returns a cloneable [`CancellationToken`] that cancels immediately on lock/inode loss.
    pub fn fence_token(&self) -> CancellationToken {
        self.fence.clone()
    }

    /// Returns `true` if lock/inode loss has tripped the ownership fence.
    pub fn is_fenced(&self) -> bool {
        self.fence.is_cancelled()
    }

    /// Synchronously verifies that `server-owner.sqlite` still exists with its acquired inode.
    ///
    /// Trips [`ServerOwnerLock::fence_token`] and returns [`OwnershipError::LockLost`] on mismatch.
    pub fn verify_ownership(&self) -> Result<(), OwnershipError> {
        if self.fence.is_cancelled() || check_lock_lost(&self.lock_path, self.identity) {
            self.fence.cancel();
            return Err(OwnershipError::LockLost {
                lock_path: self.lock_path.clone(),
            });
        }
        Ok(())
    }

    /// Publishes `server-runtime.json` stamped with this lock's `ownerId`.
    ///
    /// Fails with [`OwnershipError::LockLost`] if the ownership fence has tripped.
    pub fn publish(
        &self,
        mut state: PersistedServerRuntimeState,
    ) -> Result<PersistedServerRuntimeState, OwnershipError> {
        self.verify_ownership()?;
        if let Some(owner_id) = &self.owner_id {
            state.owner_id = Some(owner_id.clone());
        }
        persist_server_runtime_state(&self.runtime_state_path, &state)?;
        Ok(state)
    }

    /// Releases the lock, conditionally clearing `server-runtime.json` if still owned by this lock.
    pub fn release(mut self) -> Result<(), OwnershipError> {
        self.cleanup_inner();
        Ok(())
    }

    fn cleanup_inner(&mut self) {
        if let Some(mut monitor) = self.monitor.take() {
            monitor.stop();
        }
        if let Some(owner_id) = &self.owner_id
            && let Ok(Some(state)) = read_persisted_server_runtime_state(&self.runtime_state_path)
            && state.owner_id.as_deref() == Some(owner_id.as_str())
        {
            let _ = clear_persisted_server_runtime_state(&self.runtime_state_path);
        }
        if let Ok(mut guard) = self.conn.lock() {
            let _ = guard.take();
        }
    }
}

impl Drop for ServerOwnerLock {
    fn drop(&mut self) {
        self.cleanup_inner();
    }
}
