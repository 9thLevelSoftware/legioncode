//! Native `legioncode --help`, `-h`, `help`, and subcommand `--help` renderer matching Effect CLI.

use crate::cmd_help_catalog::{lookup_help, resolve_command_path};
use crate::fallback::{delegate_to_node, unit_forced_original, write_stdout_raw};

/// Unit identifier for `RUST_ANYTHING_UNIT_HELP` and fallback logs.
pub const UNIT_HELP: &str = "HELP";

/// Renders root or subcommand help when `--help`, `-h`, or `legioncode help [cmd...]` is requested.
///
/// Honors `RUST_ANYTHING_UNIT_HELP=original` for root help and never falls back to Node for
/// subcommand `--help`.
pub fn run(args: &[String]) -> Option<i32> {
    run_with_raw(args, args)
}

/// Same as [`run`], delegating `raw_args` untouched when `RUST_ANYTHING_UNIT_HELP=original`.
pub(crate) fn run_with_raw(args: &[String], raw_args: &[String]) -> Option<i32> {
    if args.is_empty() {
        return None;
    }

    if args.first().map(String::as_str) == Some("help") {
        if unit_forced_original(UNIT_HELP) {
            return Some(delegate_to_node(UNIT_HELP, raw_args));
        }
        if args
            .iter()
            .skip(1)
            .any(|arg| arg == "--help" || arg == "-h")
        {
            let text = lookup_help(&["help"])?;
            return Some(write_stdout_raw(text, "help output"));
        }
        let path = resolve_command_path(&args[1..]);
        let text = lookup_help(&path)?;
        return Some(write_stdout_raw(text, "help output"));
    }

    if !has_help_flag(args) {
        return None;
    }

    if unit_forced_original(UNIT_HELP) {
        return Some(delegate_to_node(UNIT_HELP, raw_args));
    }

    let path = resolve_command_path(args);
    let text = lookup_help(&path).or_else(|| lookup_help(&[]))?;
    Some(write_stdout_raw(text, "help output"))
}

/// Prints help for `path` (best matching prefix) to stdout when available.
pub(crate) fn print_help_for_args(args: &[String]) {
    let path = resolve_command_path(args);
    if let Some(text) = lookup_help(&path) {
        let _ = write_stdout_raw(text, "command help");
    }
}

/// Prints help for an explicit command path (`&["status"]`, `&["auth", "pairing"]`, etc.).
pub(crate) fn print_help_for_path(path: &[&str]) {
    if let Some(text) = lookup_help(path) {
        let _ = write_stdout_raw(text, "command help");
    }
}

/// Returns `true` when `--help` or `-h` appears before any `--` end-of-options marker.
pub(crate) fn has_help_flag(args: &[String]) -> bool {
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--help" || arg == "-h" {
            return true;
        }
    }
    false
}
