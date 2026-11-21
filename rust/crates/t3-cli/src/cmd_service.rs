//! `t3 service` lifecycle commands (`install`, `update`, `restart`, `uninstall`, `status`, `protect`).

use std::cmp::Ordering;
use std::env;
use std::fmt::Write as _;

use t3_edge::boot_service_units::format_boot_service_base_dir_option;
use t3_persistence::service_launcher::compare_exact_service_versions;

use crate::boot_service_ops::{
    BootServiceInspection, RestartOutcome, ServiceChange, UninstallOutcome,
    format_boot_service_problem, inspect_service, install_or_reconcile_service, restart_service,
    uninstall_service,
};
use crate::cli_global::fail_with_command_help;
use crate::fallback::{
    delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original, write_stdout_line,
};
use crate::invocation::{resolve_host_cli_command, resolve_install_launcher};

/// Unit identifier for `RUST_ANYTHING_UNIT_SERVICE` and fallback logs.
pub const UNIT_SERVICE: &str = "SERVICE";

struct ServiceFlags {
    base_dir: Option<String>,
    allow_downgrade: bool,
}

/// Runs `t3 service <subcommand>` natively unless `RUST_ANYTHING_UNIT_SERVICE=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_SERVICE) {
        return delegate_to_node(UNIT_SERVICE, args);
    }

    let Some(subcommand) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(&["service"], "Missing subcommand for `t3 service`.");
    };

    let rest = args.get(2..).unwrap_or(&[]);
    match subcommand {
        "install" | "update" => {
            let Some(flags) = parse_flags(rest, true) else {
                return fail_with_command_help(
                    &["service", "install"],
                    &format!("Invalid or unrecognized arguments for `t3 service {subcommand}`."),
                );
            };
            run_reconcile(subcommand == "update", &flags)
        }
        "status" => {
            let Some(flags) = parse_flags(rest, false) else {
                return fail_with_command_help(
                    &["service", "status"],
                    "Invalid or unrecognized arguments for `t3 service status`.",
                );
            };
            run_status(flags.base_dir.as_deref())
        }
        "restart" => {
            let Some(flags) = parse_flags(rest, false) else {
                return fail_with_command_help(
                    &["service", "restart"],
                    "Invalid or unrecognized arguments for `t3 service restart`.",
                );
            };
            run_restart(flags.base_dir.as_deref())
        }
        "uninstall" => {
            let Some(flags) = parse_flags(rest, false) else {
                return fail_with_command_help(
                    &["service", "uninstall"],
                    "Invalid or unrecognized arguments for `t3 service uninstall`.",
                );
            };
            run_uninstall(flags.base_dir.as_deref())
        }
        "protect" => run_protect(rest),
        other => fail_with_command_help(
            &["service"],
            &format!("Unknown subcommand \"{other}\" for \"t3 service\""),
        ),
    }
}

fn run_protect(rest: &[String]) -> i32 {
    if !parse_protect_flags(rest) {
        return fail_with_command_help(
            &["service", "protect"],
            "Invalid or unrecognized arguments for `t3 service protect`.",
        );
    }
    if !cfg!(target_os = "linux") {
        return write_stdout_line("Service protection needs Linux with systemd.", "service");
    }
    write_stdout_line(
        "Install the background service (`t3 service install`) and configure systemd protection (`loginctl enable-linger`) as your service user.",
        "service",
    )
}

fn parse_protect_flags(rest: &[String]) -> bool {
    let mut idx = 0;
    while idx < rest.len() {
        let arg = &rest[idx];
        if arg == "--undo" || arg == "--yes" {
            // Supported boolean flags on `t3 service protect`.
        } else if matches!(take_flag_value(rest, &mut idx, "base-dir"), Ok(Some(_))) {
            // Optional `--base-dir <dir>` flag.
        } else {
            return false;
        }
        idx += 1;
    }
    true
}

fn parse_flags(rest: &[String], allow_downgrade_flag: bool) -> Option<ServiceFlags> {
    let mut base_dir = None;
    let mut allow_downgrade = false;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if allow_downgrade_flag && arg == "--allow-downgrade" {
            allow_downgrade = true;
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else {
            return None;
        }
        idx += 1;
    }

    Some(ServiceFlags {
        base_dir,
        allow_downgrade,
    })
}

fn run_reconcile(is_deprecated_update: bool, flags: &ServiceFlags) -> i32 {
    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    let before = inspect_service(&base_dir);
    if is_deprecated_update {
        println!(
            "`t3 service update` is deprecated: run `t3 update` to move to a newer release, or `t3 service install` to repair the service. Repairing now."
        );
    }

    let change = match install_or_reconcile_service(&base_dir, flags.allow_downgrade, true) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };

    let after = inspect_service(&base_dir);
    let current_ver = env!("CARGO_PKG_VERSION");
    match change {
        ServiceChange::Unchanged => {
            let entry = resolve_install_launcher()
                .map_or_else(|| "t3".to_owned(), |p| p.display().to_string());
            let msg = if is_deprecated_update {
                format!("T3 Code service is already using t3@{current_ver} ({entry}).")
            } else {
                format!("T3 Code service is already installed with t3@{current_ver} ({entry}).")
            };
            write_stdout_line(&msg, "service install")
        }
        ServiceChange::Installed | ServiceChange::Updated => {
            let mut out = format_service_change(change, &after.log_path);
            if !before.installed {
                let up_cmd = host_cmd_with_base_dir("up", flags.base_dir.as_deref());
                let _ = write!(
                    out,
                    "\nRun `{up_cmd}` to choose how your other devices reach it."
                );
            }
            write_stdout_line(&out, "service install")
        }
    }
}

pub(crate) fn format_service_change(change: ServiceChange, log_path: &std::path::Path) -> String {
    let ver = env!("CARGO_PKG_VERSION");
    let verb = match change {
        ServiceChange::Installed => "Service installed",
        ServiceChange::Updated | ServiceChange::Unchanged => "Service updated",
    };
    format!("{verb} \u{00B7} t3@{ver}\nLog: {}", log_path.display())
}

fn run_status(raw_base_dir: Option<&str>) -> i32 {
    let (base_dir, _) = resolve_base_dir(raw_base_dir);
    let status = inspect_service(&base_dir);
    let install_cmd = host_cmd_with_base_dir("service install", raw_base_dir);
    let restart_cmd = host_cmd_with_base_dir("service restart", raw_base_dir);
    let rendered = format_service_status(&status, &install_cmd, &restart_cmd);
    write_stdout_line(&rendered, "service status")
}

fn format_service_status(
    status: &BootServiceInspection,
    install_cmd: &str,
    restart_cmd: &str,
) -> String {
    if !status.supported {
        return "T3 Code service\n  Status: unavailable on this machine\n  Supported on: Linux with systemd, macOS with launchd, Windows with Task Scheduler".to_owned();
    }
    if !status.installed {
        return format!("T3 Code service\n  Status: not installed\n  Next: Run `{install_cmd}`.");
    }
    if let Some(ref other) = status.other_base_dir {
        let opt = format_boot_service_base_dir_option(other);
        return format!(
            "T3 Code service\n  Status: installed for another T3 home\n  Base dir: {other}\n  Next: Pass `{opt}` to manage it."
        );
    }

    let cli_ver = env!("CARGO_PKG_VERSION");
    let installed_ver = status.installed_version.as_deref().unwrap_or(cli_ver);
    if status
        .installed_version
        .as_deref()
        .is_some_and(|v| compare_exact_service_versions(v, cli_ver) == Ordering::Greater)
    {
        return format!(
            "T3 Code service\n  Status: installed \u{00B7} t3@{installed_ver} (newer than this t3@{cli_ver} CLI)"
        );
    }

    let status_line = if status.current {
        format!("installed \u{00B7} t3@{installed_ver}")
    } else {
        "needs an update or repair".to_owned()
    };
    let mut lines = vec![
        "T3 Code service".to_owned(),
        format!("  Status: {status_line}"),
        format!(
            "  Unit: {}",
            status
                .unit_path
                .as_ref()
                .map_or_else(String::new, |p| p.display().to_string())
        ),
        format!("  Logs: {}", status.log_path.display()),
    ];
    for problem in &status.problems {
        lines.push(format!(
            "  [{problem}] {}",
            format_boot_service_problem(problem, install_cmd, restart_cmd)
        ));
    }
    if !status.current {
        if status.problems.as_slice() == ["restart-pending"] {
            lines.push(format!("  Next: Run `{restart_cmd}` to switch."));
        } else {
            lines.push(format!("  Next: Run `{install_cmd}` to repair it."));
        }
    }
    lines.join("\n")
}

fn run_restart(raw_base_dir: Option<&str>) -> i32 {
    let (base_dir, _) = resolve_base_dir(raw_base_dir);
    let status = inspect_service(&base_dir);
    if !status.supported {
        return write_stdout_line("T3 Code service is not installed.", "service restart");
    }
    match restart_service(&base_dir) {
        Ok(RestartOutcome::NotInstalled) => {
            write_stdout_line("T3 Code service is not installed.", "service restart")
        }
        Ok(RestartOutcome::OtherBaseDir(other)) => {
            let opt = format_boot_service_base_dir_option(&other);
            write_stdout_line(
                &format!(
                    "The T3 Code service serves {other}, so it was not restarted. Pass `{opt}` to restart it."
                ),
                "service restart",
            )
        }
        Ok(RestartOutcome::Restarted(ver)) => {
            let suffix = ver.map_or_else(String::new, |v| format!(" on t3@{v}"));
            write_stdout_line(
                &format!("Restarted the T3 Code service{suffix}."),
                "service restart",
            )
        }
        Ok(RestartOutcome::Queued(ver)) => {
            let suffix = ver.map_or_else(String::new, |v| format!(" on t3@{v}"));
            write_stdout_line(
                &format!(
                    "Queued a restart of the T3 Code service{suffix}. This session runs inside the service and disconnects when it goes down.\nLogs: {}",
                    status.log_path.display()
                ),
                "service restart",
            )
        }
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    }
}

fn run_uninstall(raw_base_dir: Option<&str>) -> i32 {
    let (base_dir, _) = resolve_base_dir(raw_base_dir);
    let status = inspect_service(&base_dir);
    if !status.supported {
        return write_stdout_line("T3 Code service is not installed.", "service uninstall");
    }
    match uninstall_service(&base_dir) {
        Ok(UninstallOutcome::NotInstalled) => {
            write_stdout_line("T3 Code service is not installed.", "service uninstall")
        }
        Ok(UninstallOutcome::OtherBaseDir(other)) => {
            let opt = format_boot_service_base_dir_option(&other);
            write_stdout_line(
                &format!(
                    "The T3 Code service serves {other}, so it was not removed. Pass `{opt}` to remove it."
                ),
                "service uninstall",
            )
        }
        Ok(UninstallOutcome::Removed) => {
            write_stdout_line("Removed the T3 Code service.", "service uninstall")
        }
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    }
}

fn host_cmd_with_base_dir(subcommand: &str, raw_base_dir: Option<&str>) -> String {
    let extra = raw_base_dir
        .map(|d| vec!["--base-dir".to_owned(), d.to_owned()])
        .unwrap_or_default();
    resolve_host_cli_command(subcommand, &extra)
}
