//! Sidecar configuration, error types, and executable discovery across explicit, Tauri
//! `externalBin`, sibling, `T3CODE_CLI_PATH`, and `PATH` candidates.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::service_preflight::{SERVICE_LAUNCHER_PROTOCOL, ServicePreflightResult};
use t3_process_ffi::env::SanitizedChildEnv;
use t3_process_ffi::pipes::ParentPipeSet;
use t3_process_ffi::supervisor::{ProcessToken, SpawnSpec, SupervisorError};
use thiserror::Error;

/// Errors emitted while preflighting, discovering, or spawning the `t3` backend sidecar.
#[derive(Debug, Error)]
pub enum SidecarError {
    /// Database or launcher protocol compatibility preflight blocked sidecar startup.
    #[error("sidecar preflight blocked for t3@{version} at '{}': {reason}", db_path.display())]
    PreflightBlocked {
        /// Running application version.
        version: String,
        /// Human-readable blocking reason from `t3_persistence::service_preflight`.
        reason: String,
        /// Inspected SQLite database path.
        db_path: PathBuf,
    },
    /// No executable `t3` sidecar binary could be located across candidate locations.
    #[error(
        "could not locate t3 sidecar binary (checked {} candidate paths)",
        candidates.len()
    )]
    RuntimeNotFound {
        /// Candidate paths probed during discovery.
        candidates: Vec<PathBuf>,
    },
    /// Underlying OS process supervisor failed to configure or spawn the child.
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
}

/// Discovery source that resolved the `t3` sidecar executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeDiscoverySource {
    /// Caller-supplied `explicit_binary_path`.
    Explicit,
    /// Bundled Tauri `externalBin` path under `binaries/`.
    TauriExternalBin,
    /// Sibling binary beside the running desktop host executable.
    SiblingBinary,
    /// Executable named by `T3CODE_CLI_PATH`.
    CliPathEnv,
    /// Discovered via the process `PATH`.
    SystemPath,
    /// No candidate matched on disk.
    NotFound,
}

/// Result of probing candidate locations for the `t3` sidecar executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarRuntimeDiscovery {
    /// Resolved executable path when `found` is `true`, otherwise the primary expected path.
    pub binary_path: PathBuf,
    /// Source category that yielded `binary_path`.
    pub source: RuntimeDiscoverySource,
    /// Whether an existing file was found on disk.
    pub found: bool,
    /// Ordered list of candidate paths inspected.
    pub candidates_checked: Vec<PathBuf>,
}

/// Sidecar runtime mode controlling state directory (`userdata/` vs `dev/`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidecarMode {
    /// Production runtime (`userdata/`).
    #[default]
    Production,
    /// Development runtime (`dev/` when `base_dir_is_explicit == false`).
    Development,
}

impl SidecarMode {
    /// Returns `true` when running in development mode.
    pub const fn is_development(self) -> bool {
        matches!(self, Self::Development)
    }

    /// Constructs a [`SidecarMode`] from a boolean development flag.
    pub const fn from_is_development(is_development: bool) -> Self {
        if is_development {
            Self::Development
        } else {
            Self::Production
        }
    }
}

/// Configuration for [`crate::sidecar::SidecarController`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarConfig {
    /// Root T3 home directory (`T3CODE_HOME`, e.g. `~/.t3`).
    pub base_dir: PathBuf,
    /// Whether the desktop shell is running in production (`userdata/`) or development (`dev/`).
    pub mode: SidecarMode,
    /// Whether `base_dir` was explicitly configured by `--base-dir` / `T3CODE_HOME`.
    pub base_dir_is_explicit: bool,
    /// Desktop host version string passed to DB preflight checks.
    pub app_version: String,
    /// Service launcher protocol version expected by this desktop shell (`4`).
    pub launcher_protocol: i64,
    /// Bind host passed when launching `t3 serve` directly without `--bootstrap-fd`.
    pub host: String,
    /// Bind port passed when launching `t3 serve` directly without `--bootstrap-fd`.
    pub port: u16,
    /// Whether to pass `--no-browser` when launching `t3 serve`.
    pub no_browser: bool,
    /// Optional bootstrap descriptor number (`--bootstrap-fd <fd>`).
    pub bootstrap_fd: Option<u32>,
    /// Optional explicit path to the `t3` binary, bypassing search order when present.
    pub explicit_binary_path: Option<PathBuf>,
    /// Additional CLI arguments appended after the primary subcommand or `--bootstrap-fd`.
    pub extra_args: Vec<String>,
    /// Additional child environment variables applied after sanitization.
    pub extra_env: BTreeMap<String, String>,
    /// Sets `CREATE_NO_WINDOW` when launching console sidecars on Windows.
    pub windows_no_window: bool,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            base_dir: default_t3_home(),
            mode: SidecarMode::Production,
            base_dir_is_explicit: false,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            launcher_protocol: SERVICE_LAUNCHER_PROTOCOL,
            host: "127.0.0.1".to_owned(),
            port: 0,
            no_browser: true,
            bootstrap_fd: Some(3),
            explicit_binary_path: None,
            extra_args: Vec::new(),
            extra_env: BTreeMap::new(),
            windows_no_window: true,
        }
    }
}

/// Complete verified launch plan ready to pass to [`t3_process_ffi::supervisor::JobProcessSupervisor::spawn`].
#[derive(Debug, Clone)]
pub struct PreparedSidecarLaunch {
    /// Derived server paths under `base_dir`.
    pub paths: ServerDerivedPaths,
    /// Ready preflight outcome from `t3_persistence::service_preflight`.
    pub preflight: ServicePreflightResult,
    /// Runtime discovery record.
    pub discovery: SidecarRuntimeDiscovery,
    /// Ordered command-line arguments passed to the sidecar binary.
    pub launch_args: Vec<String>,
    /// Sanitized environment map with `BUG-09` watch/debugger flags stripped.
    pub sanitized_env: SanitizedChildEnv,
    /// Ready [`SpawnSpec`] for `JobProcessSupervisor`.
    pub spawn_spec: SpawnSpec,
}

/// Active sidecar handle returned after [`crate::sidecar::SidecarController::spawn_supervised`].
#[derive(Debug)]
pub struct SupervisedSidecar {
    /// Supervisor slot token for polling or termination.
    pub token: ProcessToken,
    /// OS process ID while running.
    pub pid: Option<u32>,
    /// Parent-side stdio pipe endpoints.
    pub pipes: ParentPipeSet,
    /// Preflight result that authorized this spawn.
    pub preflight: ServicePreflightResult,
    /// Arguments passed to the sidecar binary.
    pub launch_args: Vec<String>,
    /// Binary path invoked.
    pub binary_path: PathBuf,
}

pub(crate) fn probe_runtime_candidates(
    explicit_binary_path: Option<&Path>,
    cli_path_env: Option<&str>,
    path_env: Option<&str>,
    current_exe: Option<&Path>,
) -> SidecarRuntimeDiscovery {
    let mut candidates = Vec::new();
    if let Some(explicit) = explicit_binary_path {
        candidates.push((RuntimeDiscoverySource::Explicit, explicit.to_path_buf()));
    }
    if let Some(exe_dir) = current_exe.and_then(Path::parent) {
        for name in sidecar_binary_names() {
            candidates.push((
                RuntimeDiscoverySource::TauriExternalBin,
                exe_dir.join("binaries").join(&name),
            ));
            candidates.push((
                RuntimeDiscoverySource::TauriExternalBin,
                exe_dir.join("resources").join("binaries").join(&name),
            ));
            candidates.push((RuntimeDiscoverySource::SiblingBinary, exe_dir.join(&name)));
        }
    }
    if let Some(cli_path) = cli_path_env.map(str::trim).filter(|s| !s.is_empty()) {
        candidates.push((RuntimeDiscoverySource::CliPathEnv, PathBuf::from(cli_path)));
    }
    if let Some(path_val) = path_env {
        for dir in env::split_paths(path_val) {
            for name in path_binary_names() {
                candidates.push((RuntimeDiscoverySource::SystemPath, dir.join(name)));
            }
        }
    }

    let mut checked = Vec::with_capacity(candidates.len());
    for (source, path) in candidates {
        checked.push(path.clone());
        if path.is_file() {
            return SidecarRuntimeDiscovery {
                binary_path: path,
                source,
                found: true,
                candidates_checked: checked,
            };
        }
    }

    let fallback = checked
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(default_sidecar_filename()));
    SidecarRuntimeDiscovery {
        binary_path: fallback,
        source: RuntimeDiscoverySource::NotFound,
        found: false,
        candidates_checked: checked,
    }
}

fn default_t3_home() -> PathBuf {
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".t3")
}

fn default_sidecar_filename() -> &'static str {
    if cfg!(windows) { "t3.exe" } else { "t3" }
}

fn sidecar_binary_names() -> Vec<String> {
    let mut names = vec![default_sidecar_filename().to_owned()];
    if let Some(triple) = current_target_triple() {
        let suffixed = if cfg!(windows) {
            format!("t3-{triple}.exe")
        } else {
            format!("t3-{triple}")
        };
        names.push(suffixed);
    }
    names
}

fn path_binary_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["t3.exe", "t3.cmd", "t3.bat", "t3"]
    } else {
        &["t3"]
    }
}

const fn current_target_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Some("aarch64-pc-windows-msvc")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-gnu")
    } else {
        None
    }
}
