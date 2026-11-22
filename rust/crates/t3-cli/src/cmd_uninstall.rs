//! `t3 uninstall` machine cleanup (`cli/uninstall.ts`).

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use t3_edge::boot_service_units::format_boot_service_base_dir_option;

use crate::boot_service_ops::{UninstallOutcome, inspect_service, uninstall_service};
use crate::cli_global::fail_with_command_help;
#[cfg(not(windows))]
use crate::cmd_install::extract_posix_launcher_target;
#[cfg(windows)]
use crate::cmd_install::extract_windows_shim_target;
use crate::cmd_install::{detect_standalone_runtime, is_version_executable};
use crate::fallback::{
    delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original, write_stdout_line,
};
use crate::invocation::non_empty_env;

/// Unit identifier for `RUST_ANYTHING_UNIT_UNINSTALL` and fallback logs.
pub const UNIT_UNINSTALL: &str = "UNINSTALL";

struct UninstallFlags {
    base_dir: Option<String>,
    yes: bool,
}

/// Runs `t3 uninstall [--yes|-y] [--base-dir <dir>]` natively unless `RUST_ANYTHING_UNIT_UNINSTALL=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_UNINSTALL) {
        return delegate_to_node(UNIT_UNINSTALL, args);
    }

    let Some(flags) = parse_uninstall_flags(args.get(1..).unwrap_or(&[])) else {
        return fail_with_command_help(
            &["uninstall"],
            "Invalid or unrecognized arguments for `t3 uninstall`.",
        );
    };

    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    let status = inspect_service(&base_dir);
    let serves_this_home = status.supported && status.installed && status.other_base_dir.is_none();
    let other_home_service = if serves_this_home {
        None
    } else {
        status.other_base_dir.or(status.installed_base_dir)
    };

    let versions_dir = base_dir.join("runtime").join("versions");
    let runtime_dir = base_dir.join("runtime");
    let runtime_exists = runtime_dir.exists();
    let launchers = collect_owned_launchers(&versions_dir);
    let userdata_dir = base_dir.join("userdata");

    if !serves_this_home && launchers.is_empty() && !runtime_exists {
        println!(
            "Nothing to remove: t3 is not installed for {}.",
            base_dir.display()
        );
        if detect_standalone_runtime().is_none() {
            println!(
                "  This t3 runs from a Node script, so it was installed by npm or built from source. Remove it the same way (`npm uninstall -g t3`, or delete the checkout)."
            );
        }
        print_other_home_reminder(other_home_service.as_deref());
        return 0;
    }

    println!("This will remove:");
    if serves_this_home {
        println!("  the background service (stopping it first)");
    }
    for launcher in &launchers {
        println!("  the launcher at {}", launcher.display());
    }
    if runtime_exists {
        println!("  every downloaded version under {}", runtime_dir.display());
    }
    println!(
        "Your projects, threads, and settings under {} are kept. Delete that directory yourself if you want them gone too.",
        userdata_dir.display()
    );

    if !flags.yes {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            eprintln!(
                "CliUninstallError: Not a terminal, so nothing was removed. Rerun with --yes to confirm from a script."
            );
            return 1;
        }
        if !confirm_uninstall() {
            return write_stdout_line("Left as is.", "uninstall");
        }
    }

    execute_uninstall(
        &base_dir,
        serves_this_home,
        &launchers,
        &versions_dir,
        runtime_exists.then_some(runtime_dir.as_path()),
        other_home_service.as_deref(),
    )
}

fn parse_uninstall_flags(rest: &[String]) -> Option<UninstallFlags> {
    let mut base_dir = None;
    let mut yes = false;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if arg == "--yes" || arg == "-y" {
            yes = true;
        } else if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else {
            return None;
        }
        idx += 1;
    }

    Some(UninstallFlags { base_dir, yes })
}

fn collect_owned_launchers(versions_dir: &Path) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();
    let launchers_file = versions_dir.parent().map(|rt| rt.join("launchers"));
    if let Some(file) = launchers_file
        && let Ok(contents) = fs::read_to_string(file)
    {
        for line in contents.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let path = PathBuf::from(line);
            if owns_launcher(&path, versions_dir) {
                out.insert(path);
            }
        }
    }
    out.into_iter().collect()
}

fn owns_launcher(launcher: &Path, versions_dir: &Path) -> bool {
    #[cfg(windows)]
    {
        fs::read_to_string(launcher)
            .ok()
            .and_then(|raw| extract_windows_shim_target(&raw).map(PathBuf::from))
            .is_some_and(|target| is_version_executable(versions_dir, &target))
    }
    #[cfg(not(windows))]
    {
        let link_owned = fs::read_link(launcher).is_ok_and(|t| {
            is_version_executable(
                versions_dir,
                &launcher.parent().unwrap_or(Path::new(".")).join(t),
            )
        });
        let real_owned =
            fs::canonicalize(launcher).is_ok_and(|t| is_version_executable(versions_dir, &t));
        let script_owned = fs::read_to_string(launcher)
            .ok()
            .and_then(|raw| extract_posix_launcher_target(&raw))
            .is_some_and(|t| is_version_executable(versions_dir, Path::new(&t)));
        link_owned || real_owned || script_owned
    }
}

fn confirm_uninstall() -> bool {
    print!("Remove t3 from this machine? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn execute_uninstall(
    base_dir: &Path,
    serves_this_home: bool,
    launchers: &[PathBuf],
    versions_dir: &Path,
    runtime_dir: Option<&Path>,
    other_home_service: Option<&str>,
) -> i32 {
    if serves_this_home {
        match uninstall_service(base_dir) {
            Ok(UninstallOutcome::Removed) => println!("Removed the background service."),
            Ok(UninstallOutcome::NotInstalled | UninstallOutcome::OtherBaseDir(_)) => {}
            Err(msg) => {
                eprintln!("{msg}");
                return 1;
            }
        }
    }

    for launcher in launchers {
        if !owns_launcher(launcher, versions_dir) {
            println!("Left {}: it no longer runs this t3.", launcher.display());
            continue;
        }
        if let Err(err) = fs::remove_file(launcher)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "CliUninstallError: Could not remove the launcher at {}: {err}",
                launcher.display()
            );
            return 1;
        }
        println!("Removed {}.", launcher.display());
    }

    if let Some(rt) = runtime_dir
        && remove_runtime_dir(rt) != 0
    {
        return 1;
    }

    println!();
    println!("t3 is uninstalled. Thanks for trying T3 Code.");
    print_other_home_reminder(other_home_service);
    0
}

fn remove_runtime_dir(runtime_dir: &Path) -> i32 {
    let running_inside_runtime =
        std::env::current_exe().is_ok_and(|exe| exe.starts_with(runtime_dir));

    if cfg!(windows) && running_inside_runtime {
        let comspec = non_empty_env("ComSpec")
            .or_else(|| non_empty_env("COMSPEC"))
            .unwrap_or_else(|| "cmd.exe".to_owned());
        match Command::new(comspec)
            .args([
                "/d",
                "/v:off",
                "/s",
                "/c",
                "ping -n 3 127.0.0.1 >nul & rmdir /s /q \"%T3_UNINSTALL_RUNTIME%\"",
            ])
            .env("T3_UNINSTALL_RUNTIME", runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => {
                println!("{} will be removed once t3 exits.", runtime_dir.display());
                0
            }
            Err(err) => {
                eprintln!(
                    "CliUninstallError: Could not schedule removal of {}: {err}. Delete it yourself once this window is closed.",
                    runtime_dir.display()
                );
                1
            }
        }
    } else if let Err(err) = fs::remove_dir_all(runtime_dir) {
        eprintln!(
            "CliUninstallError: Could not remove {}: {err}",
            runtime_dir.display()
        );
        1
    } else {
        println!("Removed {}.", runtime_dir.display());
        0
    }
}

fn print_other_home_reminder(other_home_service: Option<&str>) {
    if let Some(other) = other_home_service {
        let opt = format_boot_service_base_dir_option(other);
        println!(
            "The T3 Code service serves {other}, so it was not removed. Pass `{opt}` to remove it."
        );
    }
}
