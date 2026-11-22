//! Sidecar runtime discovery, child environment sanitization (`BUG-09`), DB preflight,
//! and supervised [`JobProcessSupervisor`] launch specification preparation.

use std::env;
use std::ffi::OsString;
use std::path::Path;

use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::service_preflight::{ServicePreflightResult, run_service_preflight};
use legion_process_ffi::env::SanitizedChildEnv;
use legion_process_ffi::pipes::PipeSetConfig;
use legion_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec};

pub use crate::sidecar_discovery::{
    PreparedSidecarLaunch, RuntimeDiscoverySource, SidecarConfig, SidecarError, SidecarMode,
    SidecarRuntimeDiscovery, SupervisedSidecar,
};
use crate::sidecar_discovery::{probe_runtime_candidates, probe_runtime_candidates_with_rust_bin};
pub use crate::sidecar_process::{
    ManagedDesktopSidecar, desktop_serve_launch_args, discover_static_web_dir,
    generate_ephemeral_hex_token, mint_desktop_pairing_token, pick_unused_loopback_port,
    poll_environment_ready, start_managed_desktop_sidecar,
};

/// Environment variables cleared before launching desktop-managed `legioncode` backend sidecars
/// (`DesktopBackendConfiguration.ts:75-85`).
pub const DESKTOP_BACKEND_STRIPPED_ENV_KEYS: &[&str] = &[
    "LEGIONCODE_PORT",
    "LEGIONCODE_MODE",
    "LEGIONCODE_NO_BROWSER",
    "LEGIONCODE_HOST",
    "LEGIONCODE_DESKTOP_WS_URL",
    "LEGIONCODE_DESKTOP_LAN_HOST",
    "LEGIONCODE_DESKTOP_HTTPS_ENDPOINTS",
    "LEGIONCODE_TAILSCALE_SERVE",
    "LEGIONCODE_TAILSCALE_SERVE_PORT",
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

    /// Derives canonical `LegionCode` server directories and file paths for `config.base_dir`.
    pub fn derived_paths(&self) -> ServerDerivedPaths {
        ServerDerivedPaths::derive(
            &self.config.base_dir,
            self.config.mode.is_development(),
            self.config.base_dir_is_explicit,
        )
    }

    /// Runs `legion_persistence::service_preflight::run_service_preflight` against the derived SQLite DB.
    pub fn run_preflight(&self) -> ServicePreflightResult {
        let paths = self.derived_paths();
        run_service_preflight(
            &paths.db_path,
            self.config.launcher_protocol,
            &self.config.app_version,
        )
    }

    /// Sanitizes caller-supplied `entries` (`BUG-09`), removes desktop override vars, sets
    /// `LEGIONCODE_HOME`, and applies `config.extra_env` through the same sanitization boundary.
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
        for suffix in [
            "PORT",
            "MODE",
            "NO_BROWSER",
            "HOST",
            "DESKTOP_WS_URL",
            "DESKTOP_LAN_HOST",
            "DESKTOP_HTTPS_ENDPOINTS",
            "TAILSCALE_SERVE",
            "TAILSCALE_SERVE_PORT",
        ] {
            for candidate in legion_contracts::brand::ACTIVE_BRAND.env_var_candidates(suffix) {
                sanitized.remove(&candidate);
            }
        }
        sanitized.set(
            format!(
                "{}HOME",
                legion_contracts::brand::ACTIVE_BRAND.primary_env_prefix
            ),
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

    /// Builds deterministic CLI arguments for launching the `legioncode` sidecar.
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

    /// Discovers the `legioncode` sidecar executable from explicit config, `LEGIONCODE_RUST_BIN`,
    /// Tauri `binaries/`, sibling `../bin/`, workspace targets, `LEGIONCODE_CLI_PATH`, or `PATH`.
    pub fn discover_runtime(&self) -> SidecarRuntimeDiscovery {
        let rust_bin = legion_contracts::brand::ACTIVE_BRAND.read_env("RUST_BIN");
        let cli_path = legion_contracts::brand::ACTIVE_BRAND.read_env("CLI_PATH");
        let path_env = env::var("PATH").ok();
        let current_exe = env::current_exe().ok();
        self.discover_runtime_with_env_inputs(
            rust_bin.as_deref(),
            cli_path.as_deref(),
            path_env.as_deref(),
            current_exe.as_deref(),
            true,
        )
    }

    /// Deterministic runtime discovery using caller-provided `LEGIONCODE_CLI_PATH`, `PATH`, and `exe`.
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

    /// Deterministic runtime discovery including optional `LEGIONCODE_RUST_BIN` and workspace probing.
    pub fn discover_runtime_with_env_inputs(
        &self,
        rust_bin_env: Option<&str>,
        cli_path_env: Option<&str>,
        path_env: Option<&str>,
        current_exe: Option<&Path>,
        include_workspace_targets: bool,
    ) -> SidecarRuntimeDiscovery {
        probe_runtime_candidates_with_rust_bin(
            self.config.explicit_binary_path.as_deref(),
            rust_bin_env,
            cli_path_env,
            path_env,
            current_exe,
            include_workspace_targets,
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
