//! `legioncode browser setup` host diagnostics and Linux setup implementation (`cli/browser.ts`).

use crate::cli_global::fail_with_command_help;
#[cfg(not(target_os = "linux"))]
use crate::fallback::write_stdout_line;
use crate::fallback::{delegate_to_node, take_flag_value, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_BROWSER` and fallback logs.
pub const UNIT_BROWSER: &str = "BROWSER";

/// Runs `legioncode browser setup [--base-dir <dir>]` natively unless `RUST_ANYTHING_UNIT_BROWSER=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_BROWSER) {
        return delegate_to_node(UNIT_BROWSER, args);
    }

    let Some(subcommand) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(
            &["browser"],
            "Missing subcommand for `legioncode browser`.",
        );
    };
    if subcommand != "setup" {
        return fail_with_command_help(
            &["browser"],
            &format!("Unknown subcommand \"{subcommand}\" for \"legioncode browser\""),
        );
    }

    let Ok(base_dir) = parse_setup_base_dir(args.get(2..).unwrap_or(&[])) else {
        return fail_with_command_help(
            &["browser", "setup"],
            "Invalid arguments for `legioncode browser setup`.",
        );
    };

    run_browser_setup(base_dir.as_deref())
}

fn parse_setup_base_dir(rest: &[String]) -> Result<Option<String>, ()> {
    let mut base_dir = None;
    let mut idx = 0;
    while idx < rest.len() {
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else {
            return Err(());
        }
        idx += 1;
    }
    Ok(base_dir)
}

#[cfg(not(target_os = "linux"))]
fn run_browser_setup(_base_dir: Option<&str>) -> i32 {
    write_stdout_line(
        "Nothing to set up: LegionCode's browser runs as is on this system.",
        "browser setup",
    )
}

#[cfg(target_os = "linux")]
fn run_browser_setup(explicit_base_dir: Option<&str>) -> i32 {
    crate::cmd_browser_linux::run_linux_browser_setup(explicit_base_dir)
}
