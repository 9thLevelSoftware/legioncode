//! High-level boot service inspection, installation, restart, and uninstallation (`bootService.ts`).

use std::cmp::Ordering;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use legion_edge::boot_service_units::{
    BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_TASK_FILE, BOOT_SERVICE_UNIT_FILE, BootServicePlan,
    decode_service_unit_bytes, extract_service_unit_base_dir,
    extract_service_unit_environment_value, format_boot_service_base_dir_option,
};
use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::service_launcher::{
    SERVICE_LAUNCHER_PROTOCOL, SERVICE_RESTART_PENDING_FILE, SERVICE_UNIT_ENV, ServiceState,
    compare_exact_service_versions, parse_service_state, read_service_state, runtime_entry_paths,
    service_state_path, write_service_state,
};

use crate::boot_service_mgr::{
    activate_os_service, is_supported_os, probe_service_problems, render_unit_bytes,
    stop_and_remove_os_service,
};
use crate::cmd_status::{boot_service_unit_path, paths_match};
use crate::fallback::user_home_dir;
use crate::invocation::non_empty_env;

/// Snapshot of installed boot service state for a target `base_dir`.
pub(crate) struct BootServiceInspection {
    pub(crate) supported: bool,
    pub(crate) installed: bool,
    pub(crate) current: bool,
    pub(crate) installed_version: Option<String>,
    pub(crate) installed_base_dir: Option<String>,
    pub(crate) other_base_dir: Option<String>,
    pub(crate) problems: Vec<String>,
    pub(crate) unit_path: Option<PathBuf>,
    pub(crate) log_path: PathBuf,
}

/// Outcome returned by [`install_or_reconcile_service`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceChange {
    Installed,
    Updated,
    Unchanged,
}

/// Outcome returned by [`restart_service`].
pub(crate) enum RestartOutcome {
    Restarted(Option<String>),
    Queued(Option<String>),
    OtherBaseDir(String),
    NotInstalled,
}

/// Outcome returned by [`uninstall_service`].
pub(crate) enum UninstallOutcome {
    Removed,
    OtherBaseDir(String),
    NotInstalled,
}

pub(crate) fn service_log_path(base_dir: &Path) -> PathBuf {
    ServerDerivedPaths::derive(base_dir, false, true)
        .logs_dir
        .join("boot-service.log")
}

pub(crate) fn running_inside_service() -> bool {
    matches!(
        non_empty_env(SERVICE_UNIT_ENV).as_deref(),
        Some(BOOT_SERVICE_UNIT_FILE | BOOT_SERVICE_TASK_FILE | BOOT_SERVICE_PLIST_FILE)
    )
}

/// Read-only inspection of the boot service unit, `service-state.json`, and OS manager state.
pub(crate) fn inspect_service(base_dir: &Path) -> BootServiceInspection {
    let log_path = service_log_path(base_dir);
    let Some(unit_path) = boot_service_unit_path() else {
        return BootServiceInspection {
            supported: false,
            installed: false,
            current: false,
            installed_version: None,
            installed_base_dir: None,
            other_base_dir: None,
            problems: Vec::new(),
            unit_path: None,
            log_path,
        };
    };

    let Ok(unit_bytes) = fs::read(&unit_path) else {
        return BootServiceInspection {
            supported: true,
            installed: false,
            current: false,
            installed_version: None,
            installed_base_dir: None,
            other_base_dir: None,
            problems: Vec::new(),
            unit_path: Some(unit_path),
            log_path,
        };
    };

    let unit_text = decode_service_unit_bytes(&unit_bytes);
    let installed_base_dir = extract_service_unit_base_dir(&unit_text);
    let other_base_dir = installed_base_dir
        .as_deref()
        .filter(|installed| !paths_match(Path::new(installed), base_dir))
        .map(str::to_owned);
    let state_base = installed_base_dir.as_deref().map_or(base_dir, Path::new);
    let state_file = service_state_path(state_base);
    let parsed_state = read_service_state(&state_file).ok();
    let installed_version = parsed_state
        .as_ref()
        .map(|s| s.active_version.clone())
        .filter(|v| !v.is_empty());

    let mut problems = if is_supported_os() {
        probe_service_problems()
    } else {
        Vec::new()
    };
    let restart_pending = base_dir
        .join("runtime")
        .join(SERVICE_RESTART_PENDING_FILE)
        .exists();
    if restart_pending {
        problems.push("restart-pending".to_owned());
    }

    let current_ver = env!("CARGO_PKG_VERSION");
    let current = other_base_dir.is_none()
        && problems.is_empty()
        && installed_version.as_deref() == Some(current_ver)
        && parsed_state.as_ref().is_some_and(|s| s.update.is_none());

    BootServiceInspection {
        supported: true,
        installed: true,
        current,
        installed_version,
        installed_base_dir,
        other_base_dir,
        problems,
        unit_path: Some(unit_path),
        log_path,
    }
}

/// Installs or updates the boot service unit and activates it via the OS service manager.
pub(crate) fn install_or_reconcile_service(
    base_dir: &Path,
    allow_downgrade: bool,
    start: bool,
) -> Result<ServiceChange, String> {
    let Some(unit_path) = is_supported_os().then(boot_service_unit_path).flatten() else {
        return Err(unsupported_platform_msg());
    };

    let existing_bytes = fs::read(&unit_path).ok();
    let existing_text = existing_bytes.as_deref().map(decode_service_unit_bytes);
    if let Some(ref text) = existing_text
        && let Some(installed_base) = extract_service_unit_base_dir(text)
        && !paths_match(Path::new(&installed_base), base_dir)
    {
        return Err(other_base_dir_error(&installed_base, base_dir));
    }

    let current_ver = env!("CARGO_PKG_VERSION");
    let state_file = service_state_path(base_dir);
    let prev_state = fs::read_to_string(&state_file)
        .ok()
        .and_then(|raw| parse_service_state(&raw));

    if let Some(ref st) = prev_state
        && !allow_downgrade
        && compare_exact_service_versions(current_ver, &st.active_version) == Ordering::Less
    {
        return Err(format!(
            "Refusing to replace legioncode@{} with older legioncode@{current_ver}. Run the command again with --allow-downgrade to continue.",
            st.active_version
        ));
    }

    let previously_installed = existing_bytes.is_some();
    if previously_installed && start && !running_inside_service() {
        stop_and_remove_os_service(false)?;
    }

    let plan = build_service_plan(base_dir, &unit_path, existing_text.as_deref());
    let home_str = user_home_dir().to_string_lossy().into_owned();
    let env_path = env::var("PATH").unwrap_or_default();
    let rendered = render_unit_bytes(&plan, &home_str, &env_path)?;

    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Could not create service unit directory: {err}"))?;
    }
    if let Some(log_parent) = Path::new(&plan.log_path).parent() {
        let _ = fs::create_dir_all(log_parent);
    }

    write_service_state(
        &state_file,
        &ServiceState {
            protocol: SERVICE_LAUNCHER_PROTOCOL,
            active_version: current_ver.to_owned(),
            update: None,
        },
    )
    .map_err(|err| format!("Could not write service state: {err}"))?;

    let unchanged_unit = existing_bytes.as_deref() == Some(rendered.as_slice())
        && prev_state
            .as_ref()
            .is_some_and(|s| s.active_version == current_ver && s.update.is_none());

    fs::write(&unit_path, &rendered).map_err(|err| {
        format!(
            "Could not write service unit {}: {err}",
            unit_path.display()
        )
    })?;

    let restart_pending = base_dir.join("runtime").join(SERVICE_RESTART_PENDING_FILE);
    if start {
        activate_os_service(&unit_path)?;
        let _ = fs::remove_file(&restart_pending);
    } else {
        let _ = fs::write(&restart_pending, format!("{current_ver}\n"));
    }

    if !previously_installed {
        Ok(ServiceChange::Installed)
    } else if unchanged_unit {
        Ok(ServiceChange::Unchanged)
    } else {
        Ok(ServiceChange::Updated)
    }
}

/// Restarts a same-home installed boot service via the OS service manager.
pub(crate) fn restart_service(base_dir: &Path) -> Result<RestartOutcome, String> {
    let Some(unit_path) = is_supported_os().then(boot_service_unit_path).flatten() else {
        return Err(unsupported_platform_msg());
    };
    let status = inspect_service(base_dir);
    if !status.installed {
        return Ok(RestartOutcome::NotInstalled);
    }
    if let Some(other) = status.other_base_dir {
        return Ok(RestartOutcome::OtherBaseDir(other));
    }

    if running_inside_service() {
        return Ok(RestartOutcome::Queued(status.installed_version));
    }

    stop_and_remove_os_service(false)?;
    activate_os_service(&unit_path)?;
    let _ = fs::remove_file(base_dir.join("runtime").join(SERVICE_RESTART_PENDING_FILE));
    Ok(RestartOutcome::Restarted(status.installed_version))
}

/// Stops, deactivates, and deletes a same-home installed boot service unit.
pub(crate) fn uninstall_service(base_dir: &Path) -> Result<UninstallOutcome, String> {
    let Some(unit_path) = is_supported_os().then(boot_service_unit_path).flatten() else {
        return Err(unsupported_platform_msg());
    };
    let status = inspect_service(base_dir);
    if !status.installed {
        return Ok(UninstallOutcome::NotInstalled);
    }
    if let Some(other) = status.other_base_dir {
        return Ok(UninstallOutcome::OtherBaseDir(other));
    }

    stop_and_remove_os_service(true)?;
    if unit_path.exists() {
        fs::remove_file(&unit_path)
            .map_err(|err| format!("Could not remove {}: {err}", unit_path.display()))?;
    }
    let _ = fs::remove_file(base_dir.join("runtime").join(SERVICE_RESTART_PENDING_FILE));
    Ok(UninstallOutcome::Removed)
}

pub(crate) fn format_boot_service_problem(
    problem: &str,
    install_cmd: &str,
    restart_cmd: &str,
) -> String {
    match problem {
        "user-manager-unavailable" => "Cannot reach the systemd user manager. Run `systemctl --user status` in a login session for the service user. Install your distribution's systemd user-session support if it is missing; do not run LegionCode with sudo.".to_owned(),
        "linger-unavailable" => "Cannot check whether this user can run services after logout. Run `loginctl show-user \"$(id -un)\" --property=Linger` and check that systemd-logind is available.".to_owned(),
        "linger-disabled" => "Lingering is disabled. LegionCode will stop when your last login session ends and will not start at boot. Run `sudo loginctl enable-linger \"$(id -un)\"` on this machine, then retry the service command as your normal user.".to_owned(),
        "service-disabled" => format!("The service is not enabled to start automatically. Run `{install_cmd}` to repair it."),
        "service-stopped" => format!("The service is not running. Check the service log (on Linux, also `systemctl --user status legioncode.service`), then run `{install_cmd}`."),
        "restart-pending" => format!("A newer version is installed but the service is still running the previous one. Run `{restart_cmd}` to switch."),
        other => format!("Service problem: {other}."),
    }
}

fn build_service_plan(
    base_dir: &Path,
    unit_path: &Path,
    existing_unit: Option<&str>,
) -> BootServicePlan {
    let (entry_path, _) = runtime_entry_paths(base_dir, env!("CARGO_PKG_VERSION"));
    let exe_path = if entry_path.exists() {
        entry_path
    } else {
        env::current_exe().unwrap_or(entry_path)
    };
    let mut plan = BootServicePlan::new(
        vec![
            exe_path.to_string_lossy().into_owned(),
            "__service-launcher".to_owned(),
        ],
        base_dir.to_string_lossy().into_owned(),
        service_log_path(base_dir).to_string_lossy().into_owned(),
        unit_path.to_string_lossy().into_owned(),
    );
    let bin_dir = non_empty_env("LEGIONCODE_INSTALL_BIN_DIR").or_else(|| {
        existing_unit
            .and_then(|u| extract_service_unit_environment_value(u, "LEGIONCODE_INSTALL_BIN_DIR"))
    });
    if let Some(dir) = bin_dir {
        plan = plan.with_install_bin_dir(dir);
    }
    plan
}

fn unsupported_platform_msg() -> String {
    format!(
        "Background setup supports Linux with systemd, macOS with launchd, and Windows with Task Scheduler; this machine reports '{}'.",
        env::consts::OS
    )
}

pub(crate) fn other_base_dir_error(installed_base_dir: &str, base_dir: &Path) -> String {
    let opt = format_boot_service_base_dir_option(installed_base_dir);
    format!(
        "The LegionCode service on this machine serves {installed_base_dir}, not {}. Pass {opt} to manage it, or run `legioncode service uninstall {opt}` first to move it; both data directories are kept.",
        base_dir.display()
    )
}
