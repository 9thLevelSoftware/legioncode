//! Service launcher argument parsing, lock probes, `ServiceState` codec, update markers,
//! and startup planning.
//!
//! Mirrors `apps/server/src/serviceLauncher.ts`, `apps/server/src/cli/serviceLauncher.ts`,
//! `apps/server/src/cloud/serviceProtocol.ts`, and `apps/server/src/desktopUpdate/DesktopUpdateMarker.ts`.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use legion_persistence::service_launcher::{
//!     LauncherOptions, ServiceState, parse_service_launcher_args, publish_update_marker,
//!     read_service_state, remaining_update_wait_ms, write_service_state,
//! };
//!
//! let args = vec!["--base-dir".to_owned(), "/tmp/legioncode".to_owned()];
//! assert_eq!(
//!     parse_service_launcher_args(&args),
//!     Some(LauncherOptions {
//!         base_dir: Some("/tmp/legioncode".to_owned()),
//!         log_file: None,
//!         service_unit: None,
//!     }),
//! );
//!
//! let dir = tempfile::tempdir()?;
//! let state_path = dir.path().join("service-state.json");
//! write_service_state(
//!     &state_path,
//!     &ServiceState {
//!         protocol: 4,
//!         active_version: "1.2.3".to_owned(),
//!         update: None,
//!     },
//! )?;
//! assert_eq!(read_service_state(&state_path)?.active_version, "1.2.3");
//!
//! let marker = dir.path().join("update-in-progress.json");
//! publish_update_marker(&marker, 10_000)?;
//! assert_eq!(remaining_update_wait_ms(&marker, 15_000), Some(55_000));
//! # Ok(())
//! # }
//! ```

mod locks;
mod plan;
mod state;
mod update_marker;
mod version;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::service_preflight::SERVICE_LAUNCHER_PROTOCOL;

pub use locks::{
    ServiceLockGuard, service_launcher_is_running, service_launcher_is_serving,
    service_launcher_lock_path, service_serving_lock_path, service_state_path, state_dir_owned,
};
pub use plan::{
    LauncherChildPlan, LauncherChildRole, LauncherStartupDecision, plan_launcher_startup,
    runtime_entry_paths, runtime_exists,
};
pub use state::{
    ServiceLauncherContext, ServiceState, ServiceUpdateRecord, decode_service_launcher_context,
    parse_legacy_service_state, parse_service_state, read_service_state, write_service_state,
};
pub use update_marker::{DesktopUpdateMarker, publish_update_marker, remaining_update_wait_ms};
pub use version::{compare_exact_service_versions, is_exact_service_version};

/// Desktop update wait-marker TTL in milliseconds (`packages/contracts/src/desktopBootstrap.ts:42`).
pub const DESKTOP_UPDATE_IN_PROGRESS_TTL_MS: u64 = 60_000;

/// Environment variable passing serialized [`ServiceLauncherContext`] to child servers.
pub const SERVICE_LAUNCHER_CONTEXT_ENV: &str = "LEGIONCODE_SERVICE_LAUNCHER_CONTEXT";

/// Optional environment overrides file stored inside `<base_dir>/runtime`.
pub const SERVICE_ENV_FILE: &str = "service.env";

/// Durable service launcher selection state filename (`<base_dir>/runtime/service-state.json`).
pub const SERVICE_STATE_FILE: &str = "service-state.json";

/// Environment variable identifying the boot service unit/task definition.
pub const SERVICE_UNIT_ENV: &str = "LEGIONCODE_BOOT_SERVICE_UNIT";

/// Marker written prior to an explicit launcher stop.
pub const SERVICE_STOP_MARKER_FILE: &str = ".service-stopping";

/// Marker cleared once the launcher boots an awaited runtime version.
pub const SERVICE_RESTART_PENDING_FILE: &str = ".restart-pending";

/// Exclusive SQLite lock filename held by a running service launcher.
pub const SERVICE_LAUNCHER_LOCK_FILE: &str = "service-launcher.lock";

/// Exclusive SQLite lock filename held while a launcher serves or is starting a server.
pub const SERVICE_SERVING_LOCK_FILE: &str = "service-serving.lock";

/// Errors returned by [`service_launcher`](self) operations.
#[derive(Debug, Error)]
pub enum ServiceLauncherError {
    /// Filesystem I/O failure.
    #[error("service launcher I/O failure during '{operation}' on '{}': {source}", path.display())]
    Io {
        /// Operation label.
        operation: &'static str,
        /// Target filesystem path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// JSON serialization failure.
    #[error("service launcher JSON error during '{operation}' for '{}': {source}", path.display())]
    Json {
        /// Operation label.
        operation: &'static str,
        /// Target filesystem path.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// SQLite lock failure.
    #[error("service launcher SQLite error during '{operation}' on '{}': {source}", path.display())]
    Sqlite {
        /// Operation label.
        operation: &'static str,
        /// Target lock file path.
        path: PathBuf,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
    /// `service-state.json` failed protocol/SemVer/update invariant checks.
    #[error("Service state at '{}' is invalid or unsupported.", path.display())]
    InvalidServiceState {
        /// Target state file path.
        path: PathBuf,
    },
}

/// CLI options parsed by [`parse_service_launcher_args`] (`cli/serviceLauncher.ts:4-18`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherOptions {
    /// Explicit `LegionCode` base directory (`--base-dir`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
    /// Log output file path passed by Windows Scheduled Task (`--log-file`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    /// Service unit or scheduled task name (`--service-unit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_unit: Option<String>,
}

/// Parses `--base-dir`, `--log-file`, and `--service-unit` flag pairs matching
/// `apps/server/src/cli/serviceLauncher.ts:4-18`.
///
/// Returns `None` on odd argument counts or unknown flags.
pub fn parse_service_launcher_args(args: &[String]) -> Option<LauncherOptions> {
    if !args.len().is_multiple_of(2) {
        return None;
    }
    let mut options = LauncherOptions::default();
    for pair in args.as_chunks::<2>().0 {
        let flag = pair[0].as_str();
        let value = pair[1].clone();
        match flag {
            "--base-dir" => options.base_dir = Some(value),
            "--log-file" => options.log_file = Some(value),
            "--service-unit" => options.service_unit = Some(value),
            _ => return None,
        }
    }
    Some(options)
}
