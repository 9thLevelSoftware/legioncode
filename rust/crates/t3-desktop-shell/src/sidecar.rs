//! Sidecar runtime discovery, child environment sanitization (`BUG-09`), DB preflight,
//! and supervised [`JobProcessSupervisor`] launch specification preparation.

use std::env;
use std::ffi::OsString;
use std::path::Path;

use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::service_preflight::{ServicePreflightResult, run_service_preflight};
use t3_process_ffi::env::SanitizedChildEnv;
use t3_process_ffi::pipes::PipeSetConfig;
use t3_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec};

use crate::sidecar_discovery::probe_runtime_candidates;
pub use crate::sidecar_discovery::{
    PreparedSidecarLaunch, RuntimeDiscoverySource, SidecarConfig, SidecarError, SidecarMode,
    SidecarRuntimeDiscovery, SupervisedSidecar,
};

/// Environment variables cleared before launching desktop-managed `t3` backend sidecars
/// (`DesktopBackendConfiguration.ts:75-85`).
pub const DESKTOP_BACKEND_STRIPPED_ENV_KEYS: &[&str] = &[
    "T3CODE_PORT",
    "T3CODE_MODE",
    "T3CODE_NO_BROWSER",
    "T3CODE_HOST",
    "T3CODE_DESKTOP_WS_URL",
    "T3CODE_DESKTOP_LAN_HOST",
    "T3CODE_DESKTOP_HTTPS_ENDPOINTS",
    "T3CODE_TAILSCALE_SERVE",
    "T3CODE_TAILSCALE_SERVE_PORT",
];

/// Coordinates sidecar preflight, runtime discovery, `BUG-09` env scrubbing, and supervised spawn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidecarController {
    config: SidecarConfig,
}

impl SidecarController {
    /// Creates a [`SidecarController`] from `config`.
    pub const fn new(config: SidecarConfig) -> Self {
        Self { config }
    }

    /// Returns the controller's configuration.
    pub const fn config(&self) -> &SidecarConfig {
        &self.config
    }

    /// Derives canonical T3 server directories and file paths for `config.base_dir`.
    pub fn derived_paths(&self) -> ServerDerivedPaths {
        ServerDerivedPaths::derive(
            &self.config.base_dir,
            self.config.mode.is_development(),
            self.config.base_dir_is_explicit,
        )
    }

    /// Runs `t3_persistence::service_preflight::run_service_preflight` against the derived SQLite DB.
    pub fn run_preflight(&self) -> ServicePreflightResult {
        let paths = self.derived_paths();
        run_service_preflight(
            &paths.db_path,
            self.config.launcher_protocol,
            &self.config.app_version,
        )
    }

    /// Sanitizes caller-supplied `entries` (`BUG-09`), removes desktop override vars, sets
    /// `T3CODE_HOME`, and applies `config.extra_env` through the same sanitization boundary.
    pub fn sanitize_env_entries<I, K, V>(&self, entries: I, is_windows: bool) -> SanitizedChildEnv
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut sanitized = SanitizedChildEnv::from_entries(entries, is_windows);
        for key in DESKTOP_BACKEND_STRIPPED_ENV_KEYS {
            sanitized.remove(key);
        }
        sanitized.set(
            "T3CODE_HOME",
            self.config.base_dir.to_string_lossy().into_owned(),
        );
        for (key, value) in &self.config.extra_env {
            sanitized.set(key, value);
        }
        sanitized
    }

    /// Captures and sanitizes the current host process environment (`BUG-09`).
    pub fn sanitize_host_env(&self) -> SanitizedChildEnv {
        self.sanitize_env_entries(env::vars(), cfg!(windows))
    }

    /// Builds deterministic CLI arguments for launching the `t3` sidecar.
    pub fn launch_args(&self) -> Vec<String> {
        let mut args = if let Some(fd) = self.config.bootstrap_fd {
            vec!["--bootstrap-fd".to_owned(), fd.to_string()]
        } else {
            let mut serve_args = vec![
                "serve".to_owned(),
                "--host".to_owned(),
                self.config.host.clone(),
                "--port".to_owned(),
                self.config.port.to_string(),
                "--base-dir".to_owned(),
                self.config.base_dir.to_string_lossy().into_owned(),
            ];
            if self.config.no_browser {
                serve_args.push("--no-browser".to_owned());
            }
            serve_args
        };
        args.extend(self.config.extra_args.iter().cloned());
        args
    }

    /// Discovers the `t3` sidecar executable from explicit config, Tauri `binaries/`, sibling
    /// binaries, `T3CODE_CLI_PATH`, or `PATH`.
    pub fn discover_runtime(&self) -> SidecarRuntimeDiscovery {
        let cli_path = env::var("T3CODE_CLI_PATH").ok();
        let path_env = env::var("PATH").ok();
        let current_exe = env::current_exe().ok();
        self.discover_runtime_with_inputs(
            cli_path.as_deref(),
            path_env.as_deref(),
            current_exe.as_deref(),
        )
    }

    /// Deterministic runtime discovery using caller-provided `T3CODE_CLI_PATH`, `PATH`, and `exe`.
    pub fn discover_runtime_with_inputs(
        &self,
        cli_path_env: Option<&str>,
        path_env: Option<&str>,
        current_exe: Option<&Path>,
    ) -> SidecarRuntimeDiscovery {
        probe_runtime_candidates(
            self.config.explicit_binary_path.as_deref(),
            cli_path_env,
            path_env,
            current_exe,
        )
    }

    /// Runs DB preflight, discovers the runtime, sanitizes the host env, and builds a [`SpawnSpec`].
    pub fn prepare_spawn_spec(&self) -> Result<PreparedSidecarLaunch, SidecarError> {
        self.prepare_spawn_spec_with_env(env::vars(), cfg!(windows))
    }

    /// Runs DB preflight, discovers the runtime, sanitizes `parent_env`, and builds a [`SpawnSpec`].
    pub fn prepare_spawn_spec_with_env<I, K, V>(
        &self,
        parent_env: I,
        is_windows: bool,
    ) -> Result<PreparedSidecarLaunch, SidecarError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let paths = self.derived_paths();
        let preflight = run_service_preflight(
            &paths.db_path,
            self.config.launcher_protocol,
            &self.config.app_version,
        );
        if let ServicePreflightResult::Blocked { version, reason } = preflight {
            return Err(SidecarError::PreflightBlocked {
                version,
                reason,
                db_path: paths.db_path,
            });
        }

        let discovery = self.discover_runtime();
        if !discovery.found {
            return Err(SidecarError::RuntimeNotFound {
                candidates: discovery.candidates_checked,
            });
        }

        let launch_args = self.launch_args();
        let sanitized_env = self.sanitize_env_entries(parent_env, is_windows);
        let spawn_spec = build_spawn_spec(
            &discovery.binary_path,
            &launch_args,
            &self.config.base_dir,
            sanitized_env.clone(),
            self.config.windows_no_window,
        );

        Ok(PreparedSidecarLaunch {
            paths,
            preflight,
            discovery,
            launch_args,
            sanitized_env,
            spawn_spec,
        })
    }

    /// Prepares a verified launch plan and spawns it under `supervisor`.
    pub fn spawn_supervised(
        &self,
        supervisor: &mut JobProcessSupervisor,
    ) -> Result<SupervisedSidecar, SidecarError> {
        let prepared = self.prepare_spawn_spec()?;
        let (token, pipes) = supervisor.spawn(&prepared.spawn_spec)?;
        let pid = supervisor.pid(token);
        Ok(SupervisedSidecar {
            token,
            pid,
            pipes,
            preflight: prepared.preflight,
            launch_args: prepared.launch_args,
            binary_path: prepared.discovery.binary_path,
        })
    }
}

/// Builds a [`SpawnSpec`] with piped stdio and pre-sanitized child environment (`BUG-09`).
pub fn build_spawn_spec(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: SanitizedChildEnv,
    windows_no_window: bool,
) -> SpawnSpec {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    SpawnSpec::new(program.as_os_str())
        .args(os_args)
        .cwd(cwd)
        .env(env)
        .stdio(PipeSetConfig::piped())
        .windows_no_window(windows_no_window)
}
