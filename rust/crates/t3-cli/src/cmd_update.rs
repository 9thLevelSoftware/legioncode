//! `t3 update` self-updater and pinned runtime switcher (`cli/update.ts`).

use std::cmp::Ordering;
use std::io::{BufRead, IsTerminal, Write};

use t3_contracts::policy::ManagedUpdatesMode;
use t3_persistence::service_launcher::{compare_exact_service_versions, is_exact_service_version};

use crate::boot_service_ops::{
    RestartOutcome, inspect_service, install_or_reconcile_service, restart_service,
};
use crate::cli_global::fail_with_command_help;
use crate::cmd_policy::{load_policy_document, resolve_managed_policy_path};
use crate::cmd_update_net::{ReleaseChannel, install_target_version, resolve_newest_release};
use crate::fallback::{
    delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original, write_stdout_line,
};
use crate::invocation::non_empty_env;

/// Unit identifier for `RUST_ANYTHING_UNIT_UPDATE` and fallback logs.
pub const UNIT_UPDATE: &str = "UPDATE";

struct UpdateFlags {
    base_dir: Option<String>,
    channel: Option<String>,
    allow_downgrade: bool,
    yes: bool,
    requested_version: Option<String>,
}

/// Runs `t3 update [--base-dir <dir>] [--channel stable|nightly|preview] [--allow-downgrade] [--yes|-y] [version]` natively.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_UPDATE) {
        return delegate_to_node(UNIT_UPDATE, args);
    }

    let Some(flags) = parse_update_flags(args.get(1..).unwrap_or(&[])) else {
        return fail_with_command_help(
            &["update"],
            "Invalid or unrecognized arguments for `t3 update`.",
        );
    };

    let explicit_channel = match flags.channel.as_deref() {
        Some(raw) => {
            if let Some(ch) = ReleaseChannel::parse(raw) {
                Some(ch)
            } else {
                eprintln!(
                    "\nERROR\n  Invalid value for flag --channel: \"{raw}\". Expected: \"stable\" | \"nightly\" | \"preview\""
                );
                return 1;
            }
        }
        None => None,
    };

    if let Err(err) = check_managed_update_policy(explicit_channel) {
        eprintln!("{err}");
        return 1;
    }

    let current_ver = env!("CARGO_PKG_VERSION");
    let current_channel = ReleaseChannel::of_version(current_ver);
    let target_channel = explicit_channel.unwrap_or(current_channel);

    let target_version = match resolve_target_version(
        flags.requested_version.as_deref(),
        target_channel,
        current_channel,
    ) {
        Ok(ver) => ver,
        Err(msg) => {
            eprintln!("CliUpdateError: {msg}");
            return 1;
        }
    };

    if target_version == current_ver {
        return write_stdout_line(
            &format!("t3 is already up to date (t3@{current_ver})."),
            "update",
        );
    }

    let is_downgrade =
        compare_exact_service_versions(&target_version, current_ver) == Ordering::Less;
    if is_downgrade && !flags.allow_downgrade {
        eprintln!(
            "CliUpdateError: Refusing to downgrade from t3@{current_ver} to older t3@{target_version}. Run the command again with --allow-downgrade to continue."
        );
        return 1;
    }

    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    let before_service = inspect_service(&base_dir);
    let mirror = non_empty_env("T3CODE_RELEASE_BASE_URL").or_else(managed_mirror_url);

    let entry_path = match install_target_version(&base_dir, &target_version, mirror.as_deref()) {
        Ok(path) => path,
        Err(msg) => {
            eprintln!("CliUpdateError: {msg}");
            return 1;
        }
    };

    println!("Updated t3 from {current_ver} to {target_version}.");
    println!("  Installed {}", entry_path.display());

    if before_service.installed && before_service.other_base_dir.is_none() {
        let should_restart = flags.yes
            || (std::io::stdin().is_terminal()
                && std::io::stdout().is_terminal()
                && prompt_service_restart(&target_version));
        if should_restart {
            match install_or_reconcile_service(&base_dir, flags.allow_downgrade, true) {
                Ok(_) => println!("  Background service restarted on {target_version}"),
                Err(_) => match restart_service(&base_dir) {
                    Ok(RestartOutcome::Restarted(_) | RestartOutcome::Queued(_)) => {
                        println!("  Background service restarted on {target_version}");
                    }
                    Ok(_) => {}
                    Err(msg) => {
                        eprintln!("  Warning: could not restart background service: {msg}");
                    }
                },
            }
        } else if !flags.yes {
            println!("  Background service left running; run `t3 service restart` later.");
        }
    }

    0
}

fn parse_update_flags(rest: &[String]) -> Option<UpdateFlags> {
    let mut flags = UpdateFlags {
        base_dir: None,
        channel: None,
        allow_downgrade: false,
        yes: false,
        requested_version: None,
    };
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if arg == "--allow-downgrade" {
            flags.allow_downgrade = true;
        } else if arg == "--yes" || arg == "-y" {
            flags.yes = true;
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            flags.base_dir = Some(val);
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "channel") {
            flags.channel = Some(val);
        } else if arg.starts_with('-') || flags.requested_version.is_some() {
            return None;
        } else {
            flags.requested_version = Some(arg.clone());
        }
        idx += 1;
    }

    Some(flags)
}

fn check_managed_update_policy(channel: Option<ReleaseChannel>) -> Result<(), String> {
    let policy_path = resolve_managed_policy_path();
    if !std::path::Path::new(&policy_path).exists() {
        return Ok(());
    }
    let doc = load_policy_document(&policy_path)?;
    let Some(updates) = doc.limits.as_ref().and_then(|l| l.updates.as_ref()) else {
        return Ok(());
    };
    if updates.mode == ManagedUpdatesMode::Off {
        return Err(format!(
            "Managed updates are disabled by {} ({policy_path}).",
            doc.organization
        ));
    }
    if updates.allow_nightly == Some(false) && channel == Some(ReleaseChannel::Nightly) {
        return Err(format!(
            "The nightly release channel is disabled by {} ({policy_path}).",
            doc.organization
        ));
    }
    Ok(())
}

fn managed_mirror_url() -> Option<String> {
    let policy_path = resolve_managed_policy_path();
    if !std::path::Path::new(&policy_path).exists() {
        return None;
    }
    load_policy_document(&policy_path)
        .ok()
        .and_then(|d| d.limits)
        .and_then(|l| l.updates)
        .and_then(|u| u.feed_url)
}

fn resolve_target_version(
    requested: Option<&str>,
    target_channel: ReleaseChannel,
    current_channel: ReleaseChannel,
) -> Result<String, String> {
    match requested {
        Some(raw) => {
            let ver = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
            if !is_exact_service_version(ver) {
                return Err(format!("Invalid version \"{raw}\"."));
            }
            let req_ch = ReleaseChannel::of_version(ver);
            if current_channel != req_ch
                && current_channel != ReleaseChannel::Preview
                && req_ch != ReleaseChannel::Preview
                && target_channel != req_ch
            {
                return Err(format!(
                    "Version {ver} is on the {} channel. Pass --channel {} to switch tracks.",
                    req_ch.label(),
                    req_ch.label()
                ));
            }
            Ok(ver.to_owned())
        }
        None => resolve_newest_release(target_channel),
    }
}

fn prompt_service_restart(target_version: &str) -> bool {
    print!("Restart the background service onto {target_version} now? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
