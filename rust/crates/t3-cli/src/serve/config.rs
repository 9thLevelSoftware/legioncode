//! Configuration, flag parsing, error types, and descriptor builder for `t3 serve`.

use std::env;
use std::path::{Path, PathBuf};

use t3_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use t3_contracts::ids::{EnvironmentId, IdError};
use t3_persistence::ownership::{OwnershipError, SERVER_EXIT_CODE_STATE_DIR_OWNED};
use t3_persistence::paths::PathsError;
use t3_persistence::sqlite::SqliteStoreError;
use thiserror::Error;

use crate::fallback::{expand_home, resolve_base_dir};

/// Unit identifier for `RUST_ANYTHING_UNIT_SERVE` and fallback logs.
pub const UNIT_SERVE: &str = "SERVE";

/// Default HTTP/WebSocket listen port matching `apps/server/src/config.ts`.
pub const DEFAULT_PORT: u16 = 3773;

/// Runtime mode accepted by `t3 serve --mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServeMode {
    /// Browser or headless web mode (`web`).
    #[default]
    Web,
    /// Desktop host mode (`desktop`).
    Desktop,
}

/// Configuration for [`crate::serve::spawn_server`] and `t3 serve`.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Base data directory (`--base-dir`, `T3CODE_HOME`, or `~/.t3`).
    pub base_dir: PathBuf,
    /// Whether `base_dir` was explicitly configured (selecting `userdata` over `dev`).
    pub base_dir_is_explicit: bool,
    /// TCP port to bind (`0` binds an ephemeral port).
    pub port: u16,
    /// Optional bind host (`127.0.0.1` by default, or `0.0.0.0` with `lan`).
    pub host: Option<String>,
    /// Server runtime mode (`web` or `desktop`).
    pub mode: ServeMode,
    /// Whether automatic browser launch is suppressed.
    pub no_browser: bool,
    /// Bind on all IPv4 interfaces (`0.0.0.0`) when `host` is unset.
    pub lan: bool,
    /// Optional built web bundle directory served via `static_assets_router`.
    pub static_dir: Option<PathBuf>,
    /// Working directory used for environment metadata labeling.
    pub cwd: PathBuf,
}

impl Default for ServeConfig {
    fn default() -> Self {
        let (base_dir, base_dir_is_explicit) = resolve_base_dir(None);
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let static_dir = resolve_static_dir_env().or_else(|| resolve_default_static_dir(&cwd));
        Self {
            base_dir,
            base_dir_is_explicit,
            port: DEFAULT_PORT,
            host: None,
            mode: ServeMode::Web,
            no_browser: false,
            lan: false,
            static_dir,
            cwd,
        }
    }
}

/// Errors produced when initializing, locking, binding, or running the server.
#[derive(Debug, Error)]
pub enum ServeError {
    /// Directory derivation, creation, or environment ID initialization failed.
    #[error(transparent)]
    Paths(#[from] PathsError),
    /// State directory ownership lock or runtime state publication failed.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// SQLite store open or schema initialization failed.
    #[error(transparent)]
    Sqlite(#[from] SqliteStoreError),
    /// Persisted `environment-id` did not satisfy [`EnvironmentId`] validation.
    #[error("invalid environment ID: {0}")]
    InvalidEnvironmentId(#[from] IdError),
    /// TCP listener bind or address inspection failed.
    #[error("failed to bind TCP listener on {addr}: {source}")]
    Bind {
        /// Requested bind address (`host:port`).
        addr: String,
        /// Underlying OS socket error.
        #[source]
        source: std::io::Error,
    },
    /// Background HTTP server task terminated unexpectedly.
    #[error("server task failed: {0}")]
    ServerTask(String),
}

impl ServeError {
    /// Returns [`SERVER_EXIT_CODE_STATE_DIR_OWNED`] (`78`) when another server owns the state dir.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Ownership(OwnershipError::StateDirOwned { .. }) => {
                SERVER_EXIT_CODE_STATE_DIR_OWNED
            }
            Self::Paths(_)
            | Self::Ownership(_)
            | Self::Sqlite(_)
            | Self::InvalidEnvironmentId(_)
            | Self::Bind { .. }
            | Self::ServerTask(_) => 1,
        }
    }
}

pub(crate) fn resolve_static_dir_env() -> Option<PathBuf> {
    t3_contracts::brand::ACTIVE_BRAND
        .read_env("STATIC_DIR")
        .map(|v| expand_home(v.trim()))
}

/// Auto-discovers the first built web bundle directory containing `index.html` among
/// `<current_exe_dir>/client`, `<current_exe_dir>/../web-app`, and `<cwd>/client`.
pub fn resolve_default_static_dir(cwd: &Path) -> Option<PathBuf> {
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    resolve_default_static_dir_from(exe_dir.as_deref(), cwd)
}

/// Testable helper backing [`resolve_default_static_dir`] with an explicit executable directory.
pub fn resolve_default_static_dir_from(exe_dir: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::with_capacity(3);
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("client"));
        let sibling_web_app = dir.parent().map_or_else(
            || dir.join("..").join("web-app"),
            |parent| parent.join("web-app"),
        );
        candidates.push(sibling_web_app);
    }
    candidates.push(cwd.join("client"));
    candidates
        .into_iter()
        .find(|dir| dir.join("index.html").is_file())
}

pub(crate) fn format_bind_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub(crate) fn build_descriptor(
    environment_id: EnvironmentId,
    cwd: &Path,
    database_migration: Option<u32>,
) -> ExecutionEnvironmentDescriptor {
    let label = resolve_environment_label(cwd);
    let platform = PlatformInfo::new(current_platform_os(), current_platform_arch());
    let mut builder = ExecutionEnvironmentDescriptor::builder(
        environment_id,
        label,
        platform,
        env!("CARGO_PKG_VERSION"),
    );
    if let Some(migration) = database_migration {
        builder = builder.database_migration(migration);
    }
    builder.build()
}

fn resolve_environment_label(cwd: &Path) -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(val) = env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    if let Some(name) = cwd.file_name().and_then(|s| s.to_str()) {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    "T3 environment".to_owned()
}

const fn current_platform_os() -> PlatformOs {
    if cfg!(target_os = "macos") {
        PlatformOs::Darwin
    } else if cfg!(target_os = "linux") {
        PlatformOs::Linux
    } else if cfg!(target_os = "windows") {
        PlatformOs::Windows
    } else {
        PlatformOs::Unknown
    }
}

const fn current_platform_arch() -> PlatformArch {
    if cfg!(target_arch = "aarch64") {
        PlatformArch::Arm64
    } else if cfg!(target_arch = "x86_64") {
        PlatformArch::X64
    } else {
        PlatformArch::Other
    }
}
