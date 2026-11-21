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
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use thiserror::Error;

pub use crate::boot_service_units_parse::{
    decode_service_unit_bytes, disable_service_task, extract_service_unit_base_dir,
    extract_service_unit_base_dir_for_brand, extract_service_unit_environment_value,
    extract_service_unit_log_path, extract_service_unit_program, service_task_is_disabled,
    systemd_unit_directory_of,
};
pub use crate::boot_service_units_protection::{
    PROTECT_DROP_IN, ProtectionMode, REMOTE_ACCESS_UNITS, SYSTEM_UNIT_DIR, ServiceProtectionPlan,
    SystemdDropIn, build_service_protection_plan, build_service_protection_plan_for_brand,
    parse_loaded_remote_access_units, render_protection_privileged_script,
    render_protection_privileged_script_for_brand,
};
pub use crate::boot_service_units_render::{
    encode_windows_task_utf16le_bom, render_boot_service_launchd_plist,
    render_boot_service_launchd_plist_for_brand, render_boot_service_systemd_unit,
    render_boot_service_systemd_unit_for_brand, render_boot_service_windows_task_xml,
    render_boot_service_windows_task_xml_for_brand,
};

const fn str_eq(left: &str, right: &str) -> bool {
    let a = left.as_bytes();
    let b = right.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut idx = 0;
    while idx < a.len() {
        if a[idx] != b[idx] {
            return false;
        }
        idx += 1;
    }
    true
}

/// Canonical service base name (`bootService.ts:59`).
pub const BOOT_SERVICE_NAME: &str = ACTIVE_BRAND.windows_task_name;

/// Linux systemd user unit file name (`bootService.ts:60`).
pub const BOOT_SERVICE_UNIT_FILE: &str = ACTIVE_BRAND.systemd_unit_name;

/// macOS launchd bundle/label (`bootService.ts:63`).
pub const BOOT_SERVICE_LAUNCHD_LABEL: &str = ACTIVE_BRAND.launchd_label;

/// macOS launchd plist file name (`bootService.ts:64`).
pub const BOOT_SERVICE_PLIST_FILE: &str = if str_eq(ACTIVE_BRAND.url_scheme, "legioncode") {
    "dev.legioncode.service.plist"
} else {
    "com.t3tools.t3code.service.plist"
};

/// Default boot service log filename (`bootService.ts:65`).
pub const BOOT_SERVICE_LOG_FILE: &str = "boot-service.log";

/// Windows Task Scheduler XML copy filename (`bootService.ts:69`).
pub const BOOT_SERVICE_TASK_FILE: &str = if str_eq(ACTIVE_BRAND.url_scheme, "legioncode") {
    "legioncode-task.xml"
} else {
    "t3code-task.xml"
};

/// Environment variable naming the boot-service definition file (`serviceProtocol.ts:19`).
pub const SERVICE_UNIT_ENV: &str = if str_eq(ACTIVE_BRAND.url_scheme, "legioncode") {
    "LEGION_BOOT_SERVICE_UNIT"
} else {
    "T3_BOOT_SERVICE_UNIT"
};

pub(crate) fn launchd_plist_file_for_brand(brand: &BrandConfig) -> String {
    format!("{}.plist", brand.launchd_label)
}

pub(crate) fn windows_task_file_for_brand(brand: &BrandConfig) -> String {
    format!("{}-task.xml", brand.windows_task_name)
}

pub(crate) fn service_unit_env_for_brand(brand: &BrandConfig) -> String {
    if brand.url_scheme == "t3code" {
        format!("{}BOOT_SERVICE_UNIT", brand.fallback_env_prefix)
    } else {
        format!("{}BOOT_SERVICE_UNIT", brand.primary_env_prefix)
    }
}

pub(crate) fn protect_drop_in_for_brand(brand: &BrandConfig) -> String {
    format!("90-{}-protect.conf", brand.url_scheme)
}

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
        "systemd cannot run {} from {base_dir:?}: the path contains a quote, backslash or control character. Choose another {} home with --base-dir.",
        ACTIVE_BRAND.product_name,
        ACTIVE_BRAND.product_name
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

    /// Formats remediation guidance using the active brand's default `<cli> service install` /
    /// `<cli> service restart` commands.
    pub fn format_default(self) -> String {
        let install_cmd = format!("{} service install", ACTIVE_BRAND.cli_bin_name);
        let restart_cmd = format!("{} service restart", ACTIVE_BRAND.cli_bin_name);
        format_boot_service_problem(self, &install_cmd, &restart_cmd)
    }
}

/// Formats actionable remediation text for `problem` using [`ACTIVE_BRAND`].
pub fn format_boot_service_problem(
    problem: BootServiceProblem,
    install_cmd: &str,
    restart_cmd: &str,
) -> String {
    format_boot_service_problem_for_brand(problem, install_cmd, restart_cmd, &ACTIVE_BRAND)
}

/// Formats actionable remediation text for `problem` parameterized by `brand`.
pub fn format_boot_service_problem_for_brand(
    problem: BootServiceProblem,
    install_cmd: &str,
    restart_cmd: &str,
    brand: &BrandConfig,
) -> String {
    let product = brand.product_name;
    let short = brand.cli_bin_name.to_ascii_uppercase();
    let unit = brand.systemd_unit_name;
    match problem {
        BootServiceProblem::UserManagerUnavailable => {
            format!(
                "Cannot reach the systemd user manager. Run `systemctl --user status` in a login session for the service user. Install your distribution's systemd user-session support if it is missing; do not run {short} with sudo."
            )
        }
        BootServiceProblem::LingerUnavailable => {
            "Cannot check whether this user can run services after logout. Run `loginctl show-user \"$(id -un)\" --property=Linger` and check that systemd-logind is available.".to_owned()
        }
        BootServiceProblem::LingerDisabled => {
            format!(
                "Lingering is disabled. {product} will stop when your last login session ends and will not start at boot. Run `sudo loginctl enable-linger \"$(id -un)\"` on this machine, then retry the service command as your normal user."
            )
        }
        BootServiceProblem::ServiceDisabled => {
            format!("The service is not enabled to start automatically. Run `{install_cmd}` to repair it.")
        }
        BootServiceProblem::ServiceStopped => {
            format!(
                "The service is not running. Check the service log (on Linux, also `systemctl --user status {unit}`), then run `{install_cmd}`."
            )
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
