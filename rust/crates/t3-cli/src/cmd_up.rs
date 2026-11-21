//! `t3 up` remote access and background service setup (`cli/up.ts`).

use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use t3_persistence::service_env::{parse_service_env, upsert_service_env};

use crate::boot_service_ops::{
    RestartOutcome, ServiceChange, inspect_service, install_or_reconcile_service,
    other_base_dir_error, restart_service, running_inside_service,
};
use crate::cli_global::fail_with_command_help;
use crate::cmd_service::format_service_change;
use crate::cmd_status::parse_service_env_bool;
use crate::fallback::{
    delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original, write_stdout_line,
};
use crate::invocation::{has_cloud_public_config, resolve_host_cli_command};

/// Unit identifier for `RUST_ANYTHING_UNIT_UP` and fallback logs.
pub const UNIT_UP: &str = "UP";

#[derive(Clone, Copy, PartialEq, Eq)]
enum TriToggle {
    Unset,
    On,
    Off,
    Conflict,
}

impl TriToggle {
    const fn merge(self, enable: bool) -> Self {
        match (self, enable) {
            (Self::Unset | Self::On, true) => Self::On,
            (Self::Unset | Self::Off, false) => Self::Off,
            _ => Self::Conflict,
        }
    }
}

struct UpFlags {
    base_dir: Option<String>,
    lan: TriToggle,
    connect: TriToggle,
    yes: bool,
}

/// Runs `t3 up [--lan|--no-lan] [--connect|--no-connect] [--yes] [--headless] [--base-dir <dir>]`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_UP) {
        return delegate_to_node(UNIT_UP, args);
    }

    let Some(flags) = parse_up_flags(args.get(1..).unwrap_or(&[])) else {
        return fail_with_command_help(&["up"], "Invalid or unrecognized arguments for `t3 up`.");
    };

    if flags.lan == TriToggle::Conflict {
        eprintln!("Conflicting flags: pass either --lan or --no-lan, not both.");
        return 1;
    }
    if flags.connect == TriToggle::Conflict {
        eprintln!("Conflicting flags: pass either --connect or --no-connect, not both.");
        return 1;
    }
    if flags.connect == TriToggle::On && !has_cloud_public_config() {
        eprintln!("T3 Connect is unavailable in this build.");
        return 1;
    }

    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    let before = inspect_service(&base_dir);
    if let Some(other) = before.other_base_dir {
        let msg = other_base_dir_error(&other, &base_dir);
        eprintln!("\nERROR\n  {msg}");
        return 1;
    }

    let env_path = base_dir.join("service.env");
    let raw_env = fs::read_to_string(&env_path).unwrap_or_default();
    let parsed_env = parse_service_env(&raw_env);
    let current_lan = parsed_env
        .get("T3CODE_LAN")
        .and_then(|v| parse_service_env_bool(v));
    let host_override = parsed_env
        .get("T3CODE_HOST")
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let any_choice_flag =
        flags.lan != TriToggle::Unset || flags.connect != TriToggle::Unset || flags.yes;
    if !(any_choice_flag || std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        eprintln!("No terminal. Choose access with --lan, --no-lan, --connect, or --yes.");
        return 1;
    }

    let desired_lan = match flags.lan {
        TriToggle::On => true,
        TriToggle::Off => false,
        TriToggle::Unset if !any_choice_flag => {
            let Some(choice) = prompt_lan_choice(current_lan.unwrap_or(before.installed)) else {
                return write_stdout_line("Nothing changed.", "up");
            };
            choice
        }
        TriToggle::Unset | TriToggle::Conflict => current_lan.unwrap_or(false),
    };

    let lan_changed =
        (flags.lan != TriToggle::Unset || !any_choice_flag) && current_lan != Some(desired_lan);
    if lan_changed && let Err(err) = update_service_env(&env_path, &raw_env, desired_lan) {
        eprintln!("Failed to update {}: {err}", env_path.display());
        return 1;
    }

    let needs_service = desired_lan || before.installed;
    if !needs_service {
        let cmd = resolve_base_cmd("", flags.base_dir.as_deref());
        return write_stdout_line(
            &format!("Remote access is off. Run `{cmd}` to use T3 Code here."),
            "up",
        );
    }

    run_up_service_step(
        &base_dir,
        &flags,
        &before,
        lan_changed,
        desired_lan,
        host_override.as_deref(),
    )
}

fn parse_up_flags(rest: &[String]) -> Option<UpFlags> {
    let mut flags = UpFlags {
        base_dir: None,
        lan: TriToggle::Unset,
        connect: TriToggle::Unset,
        yes: false,
    };
    let mut idx = 0;

    while idx < rest.len() {
        match rest[idx].as_str() {
            "--lan" => flags.lan = flags.lan.merge(true),
            "--no-lan" => flags.lan = flags.lan.merge(false),
            "--connect" => flags.connect = flags.connect.merge(true),
            "--no-connect" => flags.connect = flags.connect.merge(false),
            "--yes" | "-y" => flags.yes = true,
            "--headless" => {}
            _ => {
                if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
                    flags.base_dir = Some(val);
                } else {
                    return None;
                }
            }
        }
        idx += 1;
    }

    Some(flags)
}

fn update_service_env(env_path: &Path, existing: &str, lan_enabled: bool) -> std::io::Result<()> {
    if let Some(parent) = env_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let updated = upsert_service_env(
        existing,
        &[(
            "T3CODE_LAN",
            Some(if lan_enabled { "true" } else { "false" }),
        )],
    );
    fs::write(env_path, updated)
}

fn prompt_lan_choice(default_on: bool) -> Option<bool> {
    let default_hint = if default_on { "Y/n" } else { "y/N" };
    print!("Allow LAN/tailnet pairing-code access? [{default_hint}] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    let trimmed = line.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "" => Some(default_on),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

fn run_up_service_step(
    base_dir: &Path,
    flags: &UpFlags,
    before: &crate::boot_service_ops::BootServiceInspection,
    lan_changed: bool,
    desired_lan: bool,
    host_override: Option<&str>,
) -> i32 {
    if !before.installed || !before.current {
        match install_or_reconcile_service(base_dir, false, true) {
            Ok(change @ (ServiceChange::Installed | ServiceChange::Updated)) => {
                println!("{}", format_service_change(change, &before.log_path));
                println!();
            }
            Ok(ServiceChange::Unchanged) => {}
            Err(msg) => {
                eprintln!("{msg}");
                return 1;
            }
        }
    } else if lan_changed {
        if running_inside_service() {
            match restart_service(base_dir) {
                Ok(RestartOutcome::Queued(_)) => {
                    println!("Restart queued. This session will disconnect.");
                }
                Ok(_) => {}
                Err(msg) => {
                    eprintln!("{msg}");
                    return 1;
                }
            }
        } else {
            println!("Restarting T3 Code. Running agent turns will stop.");
            if let Err(msg) = restart_service(base_dir) {
                eprintln!("{msg}");
                return 1;
            }
        }
    }

    let ver = env!("CARGO_PKG_VERSION");
    let lan_label = if let Some(host) = host_override {
        format!("overridden \u{00B7} T3CODE_HOST={host}")
    } else if desired_lan {
        "on \u{00B7} pairing code only".to_owned()
    } else {
        "off".to_owned()
    };

    let mut summary = format!(
        "This machine\n  {:<15}installed \u{00B7} t3@{ver}\n  {:<15}{lan_label}",
        "Service", "Local network"
    );
    if desired_lan && host_override.is_none() {
        let pair_cmd = resolve_base_cmd("pair", flags.base_dir.as_deref());
        let _ = write!(
            summary,
            "\n\nAnyone on your LAN or tailnet can reach the port. A pairing code is the only lock.\n\nRun `{pair_cmd}`. Use the code on your other device."
        );
    }
    write_stdout_line(&summary, "up summary")
}

fn resolve_base_cmd(subcommand: &str, raw_base_dir: Option<&str>) -> String {
    let args = raw_base_dir
        .map(|d| vec!["--base-dir".to_owned(), d.to_owned()])
        .unwrap_or_default();
    resolve_host_cli_command(subcommand, &args)
}
