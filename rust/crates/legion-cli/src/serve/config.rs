//! Configuration, flag parsing, error types, and descriptor builder for `legioncode serve`.

use std::env;
use std::path::{Path, PathBuf};

use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::{EnvironmentId, IdError};
use legion_persistence::ownership::{OwnershipError, SERVER_EXIT_CODE_STATE_DIR_OWNED};
use legion_persistence::paths::PathsError;
use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStoreError};
use thiserror::Error;

use crate::fallback::{expand_home, resolve_base_dir};

/// Unit identifier for `RUST_ANYTHING_UNIT_SERVE` and fallback logs.
pub const UNIT_SERVE: &str = "SERVE";

/// Default HTTP/WebSocket listen port matching `apps/server/src/config.ts`.
pub const DEFAULT_PORT: u16 = 3773;

/// Runtime mode accepted by `legioncode serve --mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServeMode {
    /// Browser or headless web mode (`web`).
    #[default]
    Web,
    /// Desktop host mode (`desktop`).
    Desktop,
}

/// Configuration for [`crate::serve::spawn_server`] and `legioncode serve`.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Base data directory (`--base-dir`, `LEGIONCODE_HOME`, or `~/.legioncode`).
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
    legion_contracts::brand::ACTIVE_BRAND
        .read_env("STATIC_DIR")
        .map(|v| expand_home(v.trim()))
}

/// Auto-discovers the first built web bundle directory containing `index.html` among:
/// 1. `<current_exe_dir>/client`
/// 2. `<current_exe_dir>/../web-app`
/// 3. `<cwd>/apps/web/dist`
/// 4. `<cwd>/client`
/// 5. `<repo_root>/apps/web/dist` or `<repo_root>/apps/server/dist/client` when running from `rust/target/{debug,release}`.
pub fn resolve_default_static_dir(cwd: &Path) -> Option<PathBuf> {
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    resolve_default_static_dir_from(exe_dir.as_deref(), cwd)
}

fn resolve_repo_root_from_rust_target(exe_dir: &Path) -> Option<&Path> {
    let mut current = Some(exe_dir);
    let mut saw_profile = false;
    while let Some(dir) = current {
        let name = dir.file_name().and_then(|n| n.to_str());
        if name == Some("target") {
            let rust_dir = dir.parent()?;
            if saw_profile && rust_dir.file_name().and_then(|n| n.to_str()) == Some("rust") {
                return rust_dir.parent();
            }
            return None;
        }
        if matches!(name, Some("debug" | "release")) {
            saw_profile = true;
        }
        current = dir.parent();
    }
    None
}

/// Testable helper backing [`resolve_default_static_dir`] with an explicit executable directory.
pub fn resolve_default_static_dir_from(exe_dir: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::with_capacity(6);
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("client"));
        let sibling_web_app = dir.parent().map_or_else(
            || dir.join("..").join("web-app"),
            |parent| parent.join("web-app"),
        );
        candidates.push(sibling_web_app);
    }
    candidates.push(cwd.join("apps").join("web").join("dist"));
    candidates.push(cwd.join("client"));
    if let Some(repo_root) = exe_dir.and_then(resolve_repo_root_from_rust_target) {
        candidates.push(repo_root.join("apps").join("web").join("dist"));
        candidates.push(
            repo_root
                .join("apps")
                .join("server")
                .join("dist")
                .join("client"),
        );
    }
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
    let voice_supported = !legion_speech::discover_speech_devices().is_empty();
    ExecutionEnvironmentDescriptor::builder(
        environment_id,
        label,
        platform,
        env!("CARGO_PKG_VERSION"),
    )
    .database_migration(
        database_migration.unwrap_or_else(|| u32::try_from(BUILD_MIGRATION_HEAD).unwrap_or(114)),
    )
    .voice_transcription(voice_supported)
    .build()
}

fn resolve_environment_label(cwd: &Path) -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .filter_map(|k| env::var(k).ok())
        .map(|v| v.trim().to_owned())
        .find(|v| !v.is_empty())
        .or_else(|| {
            cwd.file_name()
                .and_then(|s| s.to_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "LegionCode environment".to_owned())
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::resolve_default_static_dir_from;

    fn write_index(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(dir)?;
        fs::write(dir.join("index.html"), "<!doctype html><html></html>")?;
        Ok(())
    }

    #[test]
    fn resolve_default_static_dir_prefers_priority_order_and_target_repo_walkup()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        let target_debug = repo.join("rust").join("target").join("debug");
        let target_release = repo
            .join("rust")
            .join("target")
            .join("x86_64")
            .join("release");
        fs::create_dir_all(&target_debug)?;
        fs::create_dir_all(&target_release)?;

        let outside_cwd = temp.path().join("outside");
        fs::create_dir_all(&outside_cwd)?;
        let pick = |e: &Path| resolve_default_static_dir_from(Some(e), &outside_cwd);

        // (5b) repo apps/server/dist/client fallback from rust/target/{debug,release}
        let repo_server_client = repo.join("apps").join("server").join("dist").join("client");
        write_index(&repo_server_client)?;
        assert_eq!(pick(&target_debug), Some(repo_server_client));

        // (5a) repo apps/web/dist beats apps/server/dist/client
        let repo_web_dist = repo.join("apps").join("web").join("dist");
        write_index(&repo_web_dist)?;
        assert_eq!(pick(&target_release), Some(repo_web_dist));

        // Non-target exe_dir does not walk up to repo root
        let non_target_bin = repo.join("bundle").join("bin");
        fs::create_dir_all(&non_target_bin)?;
        assert_eq!(pick(&non_target_bin), None);

        // (4) <cwd>/client beats (5), (3) <cwd>/apps/web/dist beats (4), (2) ../web-app beats (3), (1) exe/client beats (2)
        for dir in [
            outside_cwd.join("client"),
            outside_cwd.join("apps").join("web").join("dist"),
            repo.join("rust").join("target").join("web-app"),
            target_debug.join("client"),
        ] {
            write_index(&dir)?;
            assert_eq!(pick(&target_debug), Some(dir));
        }
        Ok(())
    }

    #[test]
    fn build_descriptor_emits_protocol_v2_and_capabilities()
    -> Result<(), Box<dyn std::error::Error>> {
        let env_id = legion_contracts::ids::EnvironmentId::new("env-test-1")?;
        let v = serde_json::to_value(super::build_descriptor(env_id, Path::new("."), None))?;
        assert_eq!(v["orchestrationProtocolVersion"], 2);
        assert_eq!(v["databaseMigration"], super::BUILD_MIGRATION_HEAD);
        for key in "shellThreadFamilies shellOlderSettled connectionProbe attachmentUploads vcsBranchStatus inlineMessageContext threadSettlement projectOrganization ignoredFileListing ignoredFileSearch environmentThemes commandOutputStreaming projectWorkflowBoard".split(' ') {
            assert_eq!(v["capabilities"][key], true, "missing capability {key}");
        }
        assert!(v["capabilities"]["fileAttachments"]["maxUploadBytes"].is_number());
        Ok(())
    }
}
