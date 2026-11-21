//! Cross-platform OS boot service unit renderers, parsers, diagnostics, and Linux memory/OOM
//! protection drop-in generation.
//!
//! Mirrors `apps/server/src/cloud/{bootService.ts,serviceProtocol.ts,serviceProtection.ts}`
//! while remaining 100% `wasm32-unknown-unknown`-compatible and pure Safe Rust.
//!
//! # Examples
//!
//! ```
//! use t3_edge::boot_service_units::{
//!     BootServicePlan, BootServiceProblem, ProtectionMode, REMOTE_ACCESS_UNITS,
//!     build_service_protection_plan, decode_service_unit_bytes, encode_windows_task_utf16le_bom,
//!     extract_service_unit_base_dir, format_boot_service_base_dir_option,
//!     format_boot_service_problem, render_boot_service_launchd_plist,
//!     render_boot_service_systemd_unit, render_boot_service_windows_task_xml,
//!     render_protection_privileged_script,
//! };
//!
//! let plan = BootServicePlan::new(
//!     vec!["/home/theo/.t3/runtime/t3".to_owned(), "__service-launcher".to_owned()],
//!     "/home/theo/.t3",
//!     "/home/theo/.t3/userdata/logs/boot-service.log",
//!     "/home/theo/.config/systemd/user/t3code.service",
//! );
//! let unit = render_boot_service_systemd_unit(&plan)?;
//! assert_eq!(extract_service_unit_base_dir(&unit).as_deref(), Some("/home/theo/.t3"));
//!
//! let plist = render_boot_service_launchd_plist(&plan, "/Users/theo", "/usr/bin:/bin");
//! assert_eq!(extract_service_unit_base_dir(&plist).as_deref(), Some("/home/theo/.t3"));
//!
//! let task_xml = render_boot_service_windows_task_xml(
//!     &plan,
//!     "PC\\theo",
//!     "C:\\Users\\theo",
//!     "C:\\Windows\\System32\\conhost.exe",
//! );
//! let bom_bytes = encode_windows_task_utf16le_bom(&task_xml);
//! assert_eq!(decode_service_unit_bytes(&bom_bytes), task_xml);
//! assert_eq!(extract_service_unit_base_dir(&task_xml).as_deref(), Some("/home/theo/.t3"));
//!
//! assert_eq!(
//!     format_boot_service_base_dir_option("/tmp/o'reilly"),
//!     "--base-dir '/tmp/o'\"'\"'reilly'"
//! );
//! assert!(
//!     format_boot_service_problem(
//!         BootServiceProblem::ServiceStopped,
//!         "t3 service install",
//!         "t3 service restart",
//!     )
//!     .contains("t3 service install")
//! );
//!
//! let protection = build_service_protection_plan(
//!     1000,
//!     "/home/theo/.config/systemd/user",
//!     REMOTE_ACCESS_UNITS,
//!     None,
//! );
//! let script = render_protection_privileged_script(&protection, ProtectionMode::Apply);
//! assert!(script.contains("OOMScoreAdjust=-900"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::boot_service_units_parse::{
    decode_service_unit_bytes, disable_service_task, extract_service_unit_base_dir,
    extract_service_unit_environment_value, extract_service_unit_log_path,
    extract_service_unit_program, service_task_is_disabled, systemd_unit_directory_of,
};
pub use crate::boot_service_units_protection::{
    PROTECT_DROP_IN, ProtectionMode, REMOTE_ACCESS_UNITS, SYSTEM_UNIT_DIR, ServiceProtectionPlan,
    SystemdDropIn, build_service_protection_plan, parse_loaded_remote_access_units,
    render_protection_privileged_script,
};
pub use crate::boot_service_units_render::{
    encode_windows_task_utf16le_bom, render_boot_service_launchd_plist,
    render_boot_service_systemd_unit, render_boot_service_windows_task_xml,
};

/// Canonical service base name (`bootService.ts:59`).
pub const BOOT_SERVICE_NAME: &str = "t3code";

/// Linux systemd user unit file name (`bootService.ts:60`).
pub const BOOT_SERVICE_UNIT_FILE: &str = "t3code.service";

/// macOS launchd bundle/label (`bootService.ts:63`).
pub const BOOT_SERVICE_LAUNCHD_LABEL: &str = "com.t3tools.t3code.service";

/// macOS launchd plist file name (`bootService.ts:64`).
pub const BOOT_SERVICE_PLIST_FILE: &str = "com.t3tools.t3code.service.plist";

/// Default boot service log filename (`bootService.ts:65`).
pub const BOOT_SERVICE_LOG_FILE: &str = "boot-service.log";

/// Windows Task Scheduler XML copy filename (`bootService.ts:69`).
pub const BOOT_SERVICE_TASK_FILE: &str = "t3code-task.xml";

/// Environment variable naming the boot-service definition file (`serviceProtocol.ts:19`).
pub const SERVICE_UNIT_ENV: &str = "T3_BOOT_SERVICE_UNIT";

/// Launcher configuration passed to OS boot-service renderers (`bootService.ts:86-94`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootServicePlan {
    /// Command and arguments invoked by the OS service manager.
    pub program: Vec<String>,
    /// Served T3 home directory (`T3CODE_HOME` / `--base-dir`).
    pub base_dir: String,
    /// Log file appended by the service manager or launcher.
    pub log_path: String,
    /// Destination unit/plist/task XML path on disk.
    pub unit_path: String,
    /// Optional binary directory preserved for `T3CODE_INSTALL_BIN_DIR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_bin_dir: Option<String>,
}

impl BootServicePlan {
    /// Constructs a [`BootServicePlan`] without `install_bin_dir`.
    pub fn new(
        program: Vec<String>,
        base_dir: impl Into<String>,
        log_path: impl Into<String>,
        unit_path: impl Into<String>,
    ) -> Self {
        Self {
            program,
            base_dir: base_dir.into(),
            log_path: log_path.into(),
            unit_path: unit_path.into(),
            install_bin_dir: None,
        }
    }

    /// Attaches an explicit `T3CODE_INSTALL_BIN_DIR` value to this plan.
    #[must_use]
    pub fn with_install_bin_dir(mut self, install_bin_dir: impl Into<String>) -> Self {
        self.install_bin_dir = Some(install_bin_dir.into());
        self
    }
}

/// Errors returned when a [`BootServicePlan`] cannot be represented safely in a unit file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BootServiceRenderError {
    /// `systemd` rejects executables containing control characters, quotes, or backslashes.
    #[error(
        "systemd cannot run T3 Code from {base_dir:?}: the path contains a quote, backslash or control character. Choose another T3 home with --base-dir."
    )]
    UnsupportedBaseDir {
        /// Unsupported `base_dir` path.
        base_dir: String,
    },
}

/// Diagnosed OS service prerequisite or health problem (`bootService.ts:647-655`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootServiceProblem {
    /// `systemctl --user` cannot connect to the user systemd manager.
    UserManagerUnavailable,
    /// `loginctl show-user` failed to report linger status.
    LingerUnavailable,
    /// `loginctl` reports `Linger=no`, so user services stop after logout.
    LingerDisabled,
    /// The installed unit/task exists but is disabled.
    ServiceDisabled,
    /// The installed unit/task is enabled but not currently running.
    ServiceStopped,
    /// A newer runtime version is staged on disk while the previous one still runs.
    RestartPending,
}

impl BootServiceProblem {
    /// Kebab-case wire tag matching `BootServiceProblem` in `bootService.ts:647-654`.
    pub fn code(self) -> &'static str {
        match self {
            Self::UserManagerUnavailable => "user-manager-unavailable",
            Self::LingerUnavailable => "linger-unavailable",
            Self::LingerDisabled => "linger-disabled",
            Self::ServiceDisabled => "service-disabled",
            Self::ServiceStopped => "service-stopped",
            Self::RestartPending => "restart-pending",
        }
    }

    /// Formats remediation guidance using the default `t3 service install` / `t3 service restart` commands.
    pub fn format_default(self) -> String {
        format_boot_service_problem(self, "t3 service install", "t3 service restart")
    }
}

/// Formats actionable remediation text for `problem` matching `bootService.ts:657-678`.
pub fn format_boot_service_problem(
    problem: BootServiceProblem,
    install_cmd: &str,
    restart_cmd: &str,
) -> String {
    match problem {
        BootServiceProblem::UserManagerUnavailable => {
            "Cannot reach the systemd user manager. Run `systemctl --user status` in a login session for the service user. Install your distribution's systemd user-session support if it is missing; do not run T3 with sudo.".to_owned()
        }
        BootServiceProblem::LingerUnavailable => {
            "Cannot check whether this user can run services after logout. Run `loginctl show-user \"$(id -un)\" --property=Linger` and check that systemd-logind is available.".to_owned()
        }
        BootServiceProblem::LingerDisabled => {
            "Lingering is disabled. T3 Code will stop when your last login session ends and will not start at boot. Run `sudo loginctl enable-linger \"$(id -un)\"` on this machine, then retry the service command as your normal user.".to_owned()
        }
        BootServiceProblem::ServiceDisabled => {
            format!("The service is not enabled to start automatically. Run `{install_cmd}` to repair it.")
        }
        BootServiceProblem::ServiceStopped => {
            format!("The service is not running. Check the service log (on Linux, also `systemctl --user status t3code.service`), then run `{install_cmd}`.")
        }
        BootServiceProblem::RestartPending => {
            format!("A newer version is installed but the service is still running the previous one. Run `{restart_cmd}` to switch.")
        }
    }
}

/// Formats `--base-dir '...'` POSIX shell argument escaping matching `bootService.ts:713-714` byte-for-byte.
pub fn format_boot_service_base_dir_option(base_dir: &str) -> String {
    format!("--base-dir '{}'", base_dir.replace('\'', "'\"'\"'"))
}
