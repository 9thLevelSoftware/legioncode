//! Server directory derivation, directory initialization, and atomic environment ID persistence.
//!
//! Mirrors `apps/server/src/config.ts` (`deriveServerPaths`, `ensureServerDirectories`) and
//! `apps/server/src/environment/ServerEnvironment.ts` (`ensure_environment_id`).

mod attachments;
mod home;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use attachments::{
    TempFileGuard, recovery_path_for, restrict_state_dir_permissions, write_and_sync,
};
use serde::{Deserialize, Serialize};
use t3_contracts::brand::BrandConfig;
use thiserror::Error;
use uuid::Uuid;

pub use attachments::sweep_stale_pending_attachments;
pub use home::{expand_home_dir, os_user_home_dir, resolve_base_dir, resolve_base_dir_for_brand};

/// Errors returned while deriving, creating, or initializing server paths and identity files.
#[derive(Debug, Error)]
pub enum PathsError {
    /// Failed to create or inspect a directory under the server home.
    #[error("failed directory operation '{operation}' at '{}': {source}", path.display())]
    Directory {
        /// Operation that failed.
        operation: &'static str,
        /// Target directory path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Failed while reading, checking, or writing `environment-id`.
    #[error("server environment ID {operation} failed at '{}': {source}", environment_id_path.display())]
    EnvironmentIdIo {
        /// Operation that failed (`check`, `read`, or `write`).
        operation: &'static str,
        /// Path to `environment-id`.
        environment_id_path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `environment-id` remained missing or empty after atomic initialization and recovery.
    #[error("server environment ID file is missing or empty after initialization at '{}'", .0.display())]
    EnvironmentIdMissingAfterInit(PathBuf),
}

/// All derived directory and file paths rooted under a T3 Code `base_dir`.
///
/// Matches `ServerDerivedPaths` in `apps/server/src/config.ts:38-58`.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use t3_persistence::paths::ServerDerivedPaths;
///
/// let prod = ServerDerivedPaths::derive("/var/lib/t3", false, false);
/// assert_eq!(prod.state_dir, Path::new("/var/lib/t3").join("userdata"));
/// assert_eq!(prod.db_path, prod.state_dir.join("statev2.sqlite"));
///
/// let dev = ServerDerivedPaths::derive("/var/lib/t3", true, false);
/// assert_eq!(dev.state_dir, Path::new("/var/lib/t3").join("dev"));
///
/// let explicit_dev = ServerDerivedPaths::derive("/var/lib/t3", true, true);
/// assert_eq!(explicit_dev.state_dir, Path::new("/var/lib/t3").join("userdata"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDerivedPaths {
    /// Active state directory (`<base_dir>/dev` for implicit dev mode, else `<base_dir>/userdata`).
    pub state_dir: PathBuf,
    /// Primary SQLite V2 database file (`<state_dir>/statev2.sqlite`).
    pub db_path: PathBuf,
    /// User keybindings file (`<state_dir>/keybindings.json`).
    pub keybindings_config_path: PathBuf,
    /// Persisted server settings file (`<state_dir>/settings.json`).
    pub settings_path: PathBuf,
    /// Directory containing environment theme definitions (`<state_dir>/themes`).
    pub environment_themes_dir: PathBuf,
    /// Directory caching provider statuses (`<base_dir>/caches`).
    pub provider_status_cache_dir: PathBuf,
    /// Directory holding managed git worktrees (`<base_dir>/worktrees`).
    pub worktrees_dir: PathBuf,
    /// Directory caching worktree checkouts (`<provider_status_cache_dir>/worktree-checkouts`).
    pub worktree_checkout_cache_dir: PathBuf,
    /// Directory storing message attachments (`<state_dir>/attachments`).
    pub attachments_dir: PathBuf,
    /// Directory storing server-side browser artifacts (`<state_dir>/browser-artifacts`).
    pub browser_artifacts_dir: PathBuf,
    /// Root logs directory (`<state_dir>/logs`).
    pub logs_dir: PathBuf,
    /// Structured server trace NDJSON log (`<logs_dir>/server.trace.ndjson`).
    pub server_trace_path: PathBuf,
    /// Provider log directory (`<logs_dir>/provider`).
    pub provider_logs_dir: PathBuf,
    /// Provider event log file (`<provider_logs_dir>/events.log`).
    pub provider_event_log_path: PathBuf,
    /// Terminal PTY log directory (`<logs_dir>/terminals`).
    pub terminal_logs_dir: PathBuf,
    /// Anonymous telemetry identifier path (`<state_dir>/anonymous-id`).
    pub anonymous_id_path: PathBuf,
    /// Environment identifier path (`<state_dir>/environment-id`).
    pub environment_id_path: PathBuf,
    /// Persisted server runtime state JSON path (`<state_dir>/server-runtime.json`).
    pub server_runtime_state_path: PathBuf,
    /// Secrets directory (`<state_dir>/secrets`).
    pub secrets_dir: PathBuf,
}

impl ServerDerivedPaths {
    /// Derives all server paths from `base_dir`, matching `config.ts:141-177`.
    pub fn derive(
        base_dir: impl AsRef<Path>,
        dev_url_present: bool,
        base_dir_is_explicit: bool,
    ) -> Self {
        let base_dir = base_dir.as_ref();
        let state_dir = base_dir.join(if dev_url_present && !base_dir_is_explicit {
            "dev"
        } else {
            "userdata"
        });
        let db_path = state_dir.join("statev2.sqlite");
        let attachments_dir = state_dir.join("attachments");
        let logs_dir = state_dir.join("logs");
        let provider_logs_dir = logs_dir.join("provider");
        let provider_status_cache_dir = base_dir.join("caches");

        Self {
            keybindings_config_path: state_dir.join("keybindings.json"),
            settings_path: state_dir.join("settings.json"),
            environment_themes_dir: state_dir.join("themes"),
            worktrees_dir: base_dir.join("worktrees"),
            worktree_checkout_cache_dir: provider_status_cache_dir.join("worktree-checkouts"),
            browser_artifacts_dir: state_dir.join("browser-artifacts"),
            server_trace_path: logs_dir.join("server.trace.ndjson"),
            provider_event_log_path: provider_logs_dir.join("events.log"),
            terminal_logs_dir: logs_dir.join("terminals"),
            anonymous_id_path: state_dir.join("anonymous-id"),
            environment_id_path: state_dir.join("environment-id"),
            server_runtime_state_path: state_dir.join("server-runtime.json"),
            secrets_dir: state_dir.join("secrets"),
            state_dir,
            db_path,
            provider_status_cache_dir,
            attachments_dir,
            logs_dir,
            provider_logs_dir,
        }
    }

    /// Resolves `base_dir` via `brand` env vars or home dotdir and derives server paths.
    #[must_use]
    pub fn resolve_for_brand(
        brand: &BrandConfig,
        cli_base_dir: Option<&Path>,
        home_dir: Option<&Path>,
        dev_url_present: bool,
        env_lookup: impl FnMut(&str) -> Option<String>,
    ) -> Self {
        let (base_dir, explicit) =
            resolve_base_dir_for_brand(brand, cli_base_dir, home_dir, env_lookup);
        Self::derive(base_dir, dev_url_present, explicit)
    }

    /// Derives paths from CLI `--base-dir`, active-brand environment variables, or home dotdir.
    #[must_use]
    pub fn derive_from_env(cli_base_dir: Option<&Path>, dev_url_present: bool) -> Self {
        let (base_dir, explicit) = resolve_base_dir(cli_base_dir);
        Self::derive(base_dir, dev_url_present, explicit)
    }
}

/// Ensures required server directories exist, restricts `state_dir` permissions on Unix, and
/// sweeps expired pending attachment uploads, matching `config.ts:179-220`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let paths = t3_persistence::ServerDerivedPaths::derive(dir.path(), false, false);
/// t3_persistence::ensure_server_directories(&paths)?;
/// assert!(paths.state_dir.is_dir());
/// assert!(paths.logs_dir.is_dir());
/// assert!(paths.attachments_dir.is_dir());
/// assert!(paths.worktrees_dir.is_dir());
/// # Ok(())
/// # }
/// ```
pub fn ensure_server_directories(derived_paths: &ServerDerivedPaths) -> Result<(), PathsError> {
    create_dir_all(&derived_paths.state_dir)?;
    if !derived_paths.db_path.try_exists().unwrap_or(true) {
        attachments::mark_no_copy_on_write_best_effort(&derived_paths.state_dir);
    }

    let dirs = [
        Some(derived_paths.logs_dir.as_path()),
        Some(derived_paths.provider_logs_dir.as_path()),
        Some(derived_paths.terminal_logs_dir.as_path()),
        Some(derived_paths.attachments_dir.as_path()),
        Some(derived_paths.worktrees_dir.as_path()),
        derived_paths.keybindings_config_path.parent(),
        derived_paths.settings_path.parent(),
        Some(derived_paths.provider_status_cache_dir.as_path()),
        derived_paths.anonymous_id_path.parent(),
        derived_paths.server_runtime_state_path.parent(),
    ];
    for dir in dirs.into_iter().flatten() {
        create_dir_all(dir)?;
    }

    restrict_state_dir_permissions(&derived_paths.state_dir);
    let _ = sweep_stale_pending_attachments(&derived_paths.attachments_dir, SystemTime::now());
    Ok(())
}

/// Atomically reads or creates the server environment ID at `environment_id_path`.
///
/// Uses a temporary file + `hard_link` (ignoring `AlreadyExists`) so concurrent processes agree on
/// one winner, falling back to `<environment_id_path>.recovery` + copy + rename if an empty
/// target blocked creation. Matches `ServerEnvironment.ts:91-177`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let paths = t3_persistence::ServerDerivedPaths::derive(dir.path(), false, false);
/// t3_persistence::ensure_server_directories(&paths)?;
/// let first = t3_persistence::ensure_environment_id(&paths.environment_id_path)?;
/// let second = t3_persistence::ensure_environment_id(&paths.environment_id_path)?;
/// assert_eq!(first, second);
/// # Ok(())
/// # }
/// ```
pub fn ensure_environment_id(environment_id_path: impl AsRef<Path>) -> Result<String, PathsError> {
    let environment_id_path = environment_id_path.as_ref();
    if let Some(existing) = read_persisted_environment_id(environment_id_path)? {
        return Ok(existing);
    }

    if let Some(parent) = environment_id_path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all(parent)?;
    }

    let generated = Uuid::new_v4().to_string();
    persist_environment_id(environment_id_path, &generated, PersistMode::Create)?;
    if let Some(winner) = read_persisted_environment_id(environment_id_path)? {
        return Ok(winner);
    }

    persist_environment_id(environment_id_path, &generated, PersistMode::Recover)?;
    if let Some(winner) = read_persisted_environment_id(environment_id_path)? {
        return Ok(winner);
    }

    Err(PathsError::EnvironmentIdMissingAfterInit(
        environment_id_path.to_path_buf(),
    ))
}

#[derive(Clone, Copy)]
enum PersistMode {
    Create,
    Recover,
}

fn create_dir_all(path: &Path) -> Result<(), PathsError> {
    fs::create_dir_all(path).map_err(|source| PathsError::Directory {
        operation: "create",
        path: path.to_path_buf(),
        source,
    })
}

fn read_persisted_environment_id(path: &Path) -> Result<Option<String>, PathsError> {
    match path.try_exists() {
        Ok(false) => return Ok(None),
        Ok(true) => {}
        Err(source) => {
            return Err(PathsError::EnvironmentIdIo {
                operation: "check",
                environment_id_path: path.to_path_buf(),
                source,
            });
        }
    }

    match fs::read_to_string(path) {
        Ok(contents) => {
            let trimmed = contents.trim();
            Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PathsError::EnvironmentIdIo {
            operation: "read",
            environment_id_path: path.to_path_buf(),
            source,
        }),
    }
}

fn persist_environment_id(
    environment_id_path: &Path,
    value: &str,
    mode: PersistMode,
) -> Result<(), PathsError> {
    let state_dir = match environment_id_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let destination = match mode {
        PersistMode::Create => environment_id_path.to_path_buf(),
        PersistMode::Recover => recovery_path_for(environment_id_path),
    };

    let guard = TempFileGuard::create(state_dir, ".environment-id-")
        .map_err(|source| write_err(environment_id_path, source))?;

    write_and_sync(&guard.path, value).map_err(|source| write_err(environment_id_path, source))?;

    match fs::hard_link(&guard.path, &destination) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
        Err(source) => return Err(write_err(environment_id_path, source)),
    }

    if matches!(mode, PersistMode::Recover) {
        // Keep `.recovery` intact so concurrent late initializers publish the same winner.
        let _ = fs::remove_file(&guard.path);
        fs::copy(&destination, &guard.path)
            .map_err(|source| write_err(environment_id_path, source))?;
        fs::rename(&guard.path, environment_id_path)
            .map_err(|source| write_err(environment_id_path, source))?;
    }

    Ok(())
}

fn write_err(environment_id_path: &Path, source: std::io::Error) -> PathsError {
    PathsError::EnvironmentIdIo {
        operation: "write",
        environment_id_path: environment_id_path.to_path_buf(),
        source,
    }
}
