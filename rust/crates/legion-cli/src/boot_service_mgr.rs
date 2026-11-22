//! OS-level boot service manager execution (`systemctl`, `launchctl`, `schtasks`) and unit rendering.

use std::path::Path;
use std::process::Command;

#[cfg(target_os = "macos")]
use legion_edge::boot_service_units::BOOT_SERVICE_LAUNCHD_LABEL;
#[cfg(target_os = "linux")]
use legion_edge::boot_service_units::BOOT_SERVICE_UNIT_FILE;
use legion_edge::boot_service_units::BootServicePlan;
#[cfg(target_os = "macos")]
use legion_edge::boot_service_units::render_boot_service_launchd_plist;
#[cfg(target_os = "linux")]
use legion_edge::boot_service_units::render_boot_service_systemd_unit;
#[cfg(target_os = "windows")]
use legion_edge::boot_service_units::{
    encode_windows_task_utf16le_bom, render_boot_service_windows_task_xml, service_task_is_disabled,
};

#[cfg(target_os = "windows")]
use crate::invocation::non_empty_env;

pub(crate) const fn is_supported_os() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_task_name() -> String {
    let domain = non_empty_env("USERDOMAIN").unwrap_or_default();
    let user = non_empty_env("USERNAME").unwrap_or_default();
    format!("LegionCode ({domain}\\{user})")
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_user_id() -> String {
    let domain = non_empty_env("USERDOMAIN").unwrap_or_default();
    let user = non_empty_env("USERNAME").unwrap_or_default();
    format!("{domain}\\{user}")
}

pub(crate) fn render_unit_bytes(
    plan: &BootServicePlan,
    home_dir: &str,
    env_path: &str,
) -> Result<Vec<u8>, String> {
    if plan.base_dir.trim().is_empty() {
        return Err("Missing base directory for boot service unit.".to_owned());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (home_dir, env_path);
        render_boot_service_systemd_unit(plan)
            .map(String::into_bytes)
            .map_err(|err| format!("{err}"))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(render_boot_service_launchd_plist(plan, home_dir, env_path).into_bytes())
    }
    #[cfg(target_os = "windows")]
    {
        let _ = env_path;
        let sys_root = non_empty_env("SystemRoot").unwrap_or_else(|| "C:\\Windows".to_owned());
        let conhost = format!("{sys_root}\\System32\\conhost.exe");
        let xml =
            render_boot_service_windows_task_xml(plan, &windows_user_id(), home_dir, &conhost);
        Ok(encode_windows_task_utf16le_bom(&xml))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (plan, home_dir, env_path);
        Err("Unsupported operating system.".to_owned())
    }
}

pub(crate) fn probe_service_problems() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        probe_systemd_problems()
    }
    #[cfg(target_os = "macos")]
    {
        probe_launchd_problems()
    }
    #[cfg(target_os = "windows")]
    {
        probe_schtasks_problems()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn probe_systemd_problems() -> Vec<String> {
    let mut problems = Vec::new();
    let mgr_ok = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !mgr_ok {
        problems.push("user-manager-unavailable".to_owned());
        return problems;
    }
    let enabled_out = Command::new("systemctl")
        .args(["--user", "is-enabled", BOOT_SERVICE_UNIT_FILE])
        .output();
    if !enabled_out
        .as_ref()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "enabled")
    {
        problems.push("service-disabled".to_owned());
    }
    let active_ok = Command::new("systemctl")
        .args(["--user", "is-active", BOOT_SERVICE_UNIT_FILE])
        .output()
        .is_ok_and(|o| o.status.success());
    if !active_ok {
        problems.push("service-stopped".to_owned());
    }
    problems
}

#[cfg(target_os = "macos")]
fn probe_launchd_problems() -> Vec<String> {
    let Some(uid) = crate::pipe_ipc::current_unix_uid() else {
        return Vec::new();
    };
    let target = format!("gui/{uid}/{BOOT_SERVICE_LAUNCHD_LABEL}");
    let loaded = Command::new("launchctl")
        .args(["print", &target])
        .output()
        .is_ok_and(|o| o.status.success());
    if loaded {
        Vec::new()
    } else {
        vec!["service-stopped".to_owned()]
    }
}

#[cfg(target_os = "windows")]
fn probe_schtasks_problems() -> Vec<String> {
    let task = windows_task_name();
    let Ok(out) = Command::new("schtasks")
        .args(["/Query", "/TN", &task, "/XML"])
        .output()
    else {
        return vec!["service-disabled".to_owned(), "service-stopped".to_owned()];
    };
    if !out.status.success() {
        return vec!["service-disabled".to_owned(), "service-stopped".to_owned()];
    }
    let mut problems = Vec::new();
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !stdout.trim().is_empty() && service_task_is_disabled(&stdout) {
        problems.push("service-disabled".to_owned());
    }
    let list_out = Command::new("schtasks")
        .args(["/Query", "/TN", &task, "/FO", "LIST"])
        .output();
    let running = list_out.as_ref().is_ok_and(|list| {
        list.status.success()
            && !list.stdout.is_empty()
            && String::from_utf8_lossy(&list.stdout)
                .lines()
                .any(|line| line.contains("Running") || line.contains("Queued"))
    });
    if !running {
        problems.push("service-stopped".to_owned());
    }
    problems
}

pub(crate) fn activate_os_service(unit_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let _ = unit_path;
        run_step(
            "reloading systemd user units",
            "systemctl",
            &["--user", "daemon-reload"],
        )?;
        run_step(
            "enabling the service",
            "systemctl",
            &["--user", "enable", BOOT_SERVICE_UNIT_FILE],
        )?;
        run_step_optional(
            "systemctl",
            &["--user", "reset-failed", BOOT_SERVICE_UNIT_FILE],
        );
        run_step(
            "starting the service",
            "systemctl",
            &["--user", "restart", BOOT_SERVICE_UNIT_FILE],
        )
    }
    #[cfg(target_os = "macos")]
    {
        let uid = crate::pipe_ipc::current_unix_uid()
            .ok_or_else(|| "Could not determine user id for launchd.".to_owned())?;
        let domain = format!("gui/{uid}");
        let target = format!("{domain}/{BOOT_SERVICE_LAUNCHD_LABEL}");
        run_step_optional("launchctl", &["enable", &target]);
        let unit_str = unit_path.to_string_lossy();
        run_step(
            "starting the service",
            "launchctl",
            &["bootstrap", &domain, &unit_str],
        )
    }
    #[cfg(target_os = "windows")]
    {
        let task = windows_task_name();
        let unit_str = unit_path.to_string_lossy();
        run_step(
            "registering the scheduled task",
            "schtasks",
            &["/Create", "/XML", &unit_str, "/TN", &task, "/F"],
        )?;
        run_step("starting the service", "schtasks", &["/Run", "/TN", &task])
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = unit_path;
        Err("Unsupported operating system.".to_owned())
    }
}

pub(crate) fn stop_and_remove_os_service(remove_registration: bool) -> Result<(), String> {
    if !is_supported_os() {
        return Err("Unsupported operating system.".to_owned());
    }
    #[cfg(target_os = "linux")]
    {
        if remove_registration {
            run_step(
                "stopping the service",
                "systemctl",
                &["--user", "disable", "--now", BOOT_SERVICE_UNIT_FILE],
            )?;
            run_step_optional("systemctl", &["--user", "daemon-reload"]);
        } else {
            run_step(
                "stopping the installed service",
                "systemctl",
                &["--user", "stop", BOOT_SERVICE_UNIT_FILE],
            )?;
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = remove_registration;
        let uid = crate::pipe_ipc::current_unix_uid()
            .ok_or_else(|| "Could not determine user id for launchd.".to_owned())?;
        let target = format!("gui/{uid}/{BOOT_SERVICE_LAUNCHD_LABEL}");
        run_step_optional("launchctl", &["bootout", "--wait", &target]);
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let task = windows_task_name();
        run_step_optional("schtasks", &["/Change", "/TN", &task, "/DISABLE"]);
        run_step_optional("schtasks", &["/End", "/TN", &task]);
        if remove_registration {
            run_step_optional("schtasks", &["/Delete", "/TN", &task, "/F"]);
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = remove_registration;
        Err("Unsupported operating system.".to_owned())
    }
}

fn run_step(step: &str, command: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(command)
        .args(args)
        .output()
        .map_err(|err| format!("Background setup failed while {step}: {err}"))?;
    if out.status.success() {
        return Ok(());
    }
    let code = out.status.code().unwrap_or(1);
    Err(format!(
        "Background setup failed while {step} (exit code {code})."
    ))
}

fn run_step_optional(command: &str, args: &[&str]) {
    let _ = Command::new(command).args(args).output();
}
