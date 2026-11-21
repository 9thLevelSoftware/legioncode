//! Standalone `t3 __ssh-helper` command group backed by `t3_remote::ssh_helper`.

use std::io::{self, Write};
use std::path::Path;

use t3_remote::ssh_helper::{
    format_picked_port, format_remaining_update_wait, format_runtime_port, is_home_owned,
    keep_server, pick_port, remaining_update_wait_ms, runtime_port, wait_ready,
};

use crate::cli_global::fail_with_command_help;
use crate::cmd_trace::current_epoch_ms;
use crate::fallback::{delegate_to_node, expand_home, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_SSH_HELPER` and fallback logs.
pub const UNIT_SSH_HELPER: &str = "SSH_HELPER";

/// Runs `t3 __ssh-helper <subcommand> ...` natively unless `RUST_ANYTHING_UNIT_SSH_HELPER=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_SSH_HELPER) {
        return delegate_to_node(UNIT_SSH_HELPER, args);
    }

    let Some(sub) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(
            &["__ssh-helper"],
            "Missing subcommand for `t3 __ssh-helper`.",
        );
    };
    let rest = args.get(2..).unwrap_or(&[]);

    match sub {
        "pick-port" => run_pick_port(rest),
        "wait-ready" => run_wait_ready(rest),
        "runtime-port" => run_runtime_port(rest),
        "keep-server" => run_keep_server(rest),
        "update-remaining" => run_update_remaining(rest),
        "home-owned" => run_home_owned(rest),
        other => fail_with_command_help(
            &["__ssh-helper"],
            &format!("Unknown subcommand \"{other}\" for \"t3 __ssh-helper\""),
        ),
    }
}

fn run_pick_port(rest: &[String]) -> i32 {
    if rest.len() != 3 {
        return 1;
    }
    let Ok(default_port) = rest[1].trim().parse::<u16>() else {
        return 1;
    };
    let Ok(scan_window) = rest[2].trim().parse::<u16>() else {
        return 1;
    };
    match pick_port(Path::new(&rest[0]), default_port, scan_window) {
        Some(port) => write_raw_stdout(&format_picked_port(port)),
        None => 1,
    }
}

fn run_wait_ready(rest: &[String]) -> i32 {
    if rest.len() != 3 {
        return 1;
    }
    let Ok(port) = rest[0].trim().parse::<u16>() else {
        return 1;
    };
    let Ok(timeout_ms) = rest[1].trim().parse::<u64>() else {
        return 1;
    };
    let Ok(probe_timeout_ms) = rest[2].trim().parse::<u64>() else {
        return 1;
    };
    i32::from(!wait_ready(port, timeout_ms, probe_timeout_ms))
}

fn run_runtime_port(rest: &[String]) -> i32 {
    if rest.len() != 1 {
        return 1;
    }
    match runtime_port(Path::new(&rest[0])) {
        Some((pid, port)) => write_raw_stdout(&format_runtime_port(pid, port)),
        None => 1,
    }
}

fn run_keep_server(rest: &[String]) -> i32 {
    if rest.len() != 2 {
        return 1;
    }
    let Ok(pid) = rest[1].trim().parse::<u32>() else {
        return 1;
    };
    i32::from(!keep_server(
        Path::new(&rest[0]),
        pid,
        env!("CARGO_PKG_VERSION"),
    ))
}

fn run_update_remaining(rest: &[String]) -> i32 {
    if rest.len() != 1 {
        return 1;
    }
    let remaining = remaining_update_wait_ms(Path::new(&rest[0]), current_epoch_ms());
    write_raw_stdout(&format_remaining_update_wait(remaining))
}

fn run_home_owned(rest: &[String]) -> i32 {
    if rest.len() != 1 {
        return 1;
    }
    let base_dir = expand_home(&rest[0]);
    i32::from(!is_home_owned(&base_dir))
}

fn write_raw_stdout(text: &str) -> i32 {
    let mut out = io::stdout().lock();
    if out.write_all(text.as_bytes()).is_err() || out.flush().is_err() {
        return 1;
    }
    0
}
