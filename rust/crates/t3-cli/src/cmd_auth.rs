//! Full native `t3 auth` CLI (`pairing`, `session`, `connect-device`) backed by `cmd_auth_ops`.

use crate::cli_global::fail_with_command_help;
use crate::cmd_auth_args::{AllowFlags, AuthOutputMode, parse_auth_flags};
use crate::cmd_auth_devices::{
    exec_connect_device_approve, exec_connect_device_list, exec_connect_device_revoke,
};
use crate::cmd_auth_ops::{
    exec_pairing_create, exec_pairing_list, exec_pairing_revoke, exec_session_issue,
    exec_session_list, exec_session_revoke,
};
use crate::fallback::{delegate_to_node, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_AUTH` and fallback logs.
pub const UNIT_AUTH: &str = "AUTH";

/// Runs `t3 auth <group> <action> ...` natively unless `RUST_ANYTHING_UNIT_AUTH=original` is set.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_AUTH) {
        return delegate_to_node(UNIT_AUTH, args);
    }

    let Some(group) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(&["auth"], "Missing subcommand for `t3 auth`.");
    };
    let rest = args.get(2..).unwrap_or(&[]);

    match group {
        "pairing" => run_pairing(rest),
        "session" => run_session(rest),
        "connect-device" => run_connect_device(rest),
        other => fail_with_command_help(
            &["auth"],
            &format!("Unknown subcommand \"{other}\" for \"t3 auth\""),
        ),
    }
}

fn run_pairing(args: &[String]) -> i32 {
    let Some(sub) = args.first().map(String::as_str) else {
        return fail_with_command_help(
            &["auth", "pairing"],
            "Missing subcommand for `t3 auth pairing`.",
        );
    };
    let tail = args.get(1..).unwrap_or(&[]);
    match sub {
        "create" => {
            let path = &["auth", "pairing", "create"];
            let flags = match parse_auth_flags(path, tail, AllowFlags::pairing_create()) {
                Ok(parsed) => parsed,
                Err(code) => return code,
            };
            if let Some(extra) = flags.positionals.first() {
                return fail_with_command_help(path, &format!("Unexpected argument: {extra}"));
            }
            exec_pairing_create(&flags)
        }
        "list" => {
            let path = &["auth", "pairing", "list"];
            let flags = match parse_auth_flags(path, tail, AllowFlags::list_only()) {
                Ok(parsed) => parsed,
                Err(code) => return code,
            };
            if let Some(extra) = flags.positionals.first() {
                return fail_with_command_help(path, &format!("Unexpected argument: {extra}"));
            }
            exec_pairing_list(
                flags.base_dir.as_deref(),
                flags.dev_url.as_deref(),
                flags.output_mode == AuthOutputMode::Json,
            )
        }
        "revoke" => {
            let path = &["auth", "pairing", "revoke"];
            match require_single_pos(path, tail, "id") {
                Ok((id, base_dir, dev_url)) => {
                    exec_pairing_revoke(base_dir.as_deref(), dev_url.as_deref(), id.trim())
                }
                Err(code) => code,
            }
        }
        other => fail_with_command_help(
            &["auth", "pairing"],
            &format!("Unknown subcommand \"{other}\" for \"t3 auth pairing\""),
        ),
    }
}

fn run_session(args: &[String]) -> i32 {
    let Some(sub) = args.first().map(String::as_str) else {
        return fail_with_command_help(
            &["auth", "session"],
            "Missing subcommand for `t3 auth session`.",
        );
    };
    let tail = args.get(1..).unwrap_or(&[]);
    match sub {
        "issue" => {
            let path = &["auth", "session", "issue"];
            let flags = match parse_auth_flags(path, tail, AllowFlags::session_issue()) {
                Ok(parsed) => parsed,
                Err(code) => return code,
            };
            if let Some(extra) = flags.positionals.first() {
                return fail_with_command_help(path, &format!("Unexpected argument: {extra}"));
            }
            exec_session_issue(&flags)
        }
        "list" => {
            let path = &["auth", "session", "list"];
            let flags = match parse_auth_flags(path, tail, AllowFlags::list_only()) {
                Ok(parsed) => parsed,
                Err(code) => return code,
            };
            if let Some(extra) = flags.positionals.first() {
                return fail_with_command_help(path, &format!("Unexpected argument: {extra}"));
            }
            exec_session_list(
                flags.base_dir.as_deref(),
                flags.dev_url.as_deref(),
                flags.output_mode == AuthOutputMode::Json,
            )
        }
        "revoke" => {
            let path = &["auth", "session", "revoke"];
            match require_single_pos(path, tail, "session-id") {
                Ok((id, base_dir, dev_url)) => {
                    exec_session_revoke(base_dir.as_deref(), dev_url.as_deref(), id.trim())
                }
                Err(code) => code,
            }
        }
        other => fail_with_command_help(
            &["auth", "session"],
            &format!("Unknown subcommand \"{other}\" for \"t3 auth session\""),
        ),
    }
}

fn run_connect_device(args: &[String]) -> i32 {
    let Some(sub) = args.first().map(String::as_str) else {
        return fail_with_command_help(
            &["auth", "connect-device"],
            "Missing subcommand for `t3 auth connect-device`.",
        );
    };
    let tail = args.get(1..).unwrap_or(&[]);
    match sub {
        "list" => {
            let path = &["auth", "connect-device", "list"];
            let flags = match parse_auth_flags(path, tail, AllowFlags::list_only()) {
                Ok(parsed) => parsed,
                Err(code) => return code,
            };
            if let Some(extra) = flags.positionals.first() {
                return fail_with_command_help(path, &format!("Unexpected argument: {extra}"));
            }
            exec_connect_device_list(
                flags.base_dir.as_deref(),
                flags.dev_url.as_deref(),
                flags.output_mode == AuthOutputMode::Json,
            )
        }
        "approve" => {
            let path = &["auth", "connect-device", "approve"];
            match require_single_pos(path, tail, "code") {
                Ok((code_arg, base_dir, dev_url)) => exec_connect_device_approve(
                    base_dir.as_deref(),
                    dev_url.as_deref(),
                    code_arg.trim(),
                ),
                Err(code) => code,
            }
        }
        "revoke" => {
            let path = &["auth", "connect-device", "revoke"];
            match require_single_pos(path, tail, "code") {
                Ok((code_arg, base_dir, dev_url)) => exec_connect_device_revoke(
                    base_dir.as_deref(),
                    dev_url.as_deref(),
                    code_arg.trim(),
                ),
                Err(code) => code,
            }
        }
        other => fail_with_command_help(
            &["auth", "connect-device"],
            &format!("Unknown subcommand \"{other}\" for \"t3 auth connect-device\""),
        ),
    }
}

fn require_single_pos(
    path: &[&str],
    tail: &[String],
    arg_name: &str,
) -> Result<(String, Option<String>, Option<String>), i32> {
    let mut flags = parse_auth_flags(path, tail, AllowFlags::location_only())?;
    if flags.positionals.is_empty() {
        return Err(fail_with_command_help(
            path,
            &format!("Missing required argument: {arg_name}"),
        ));
    }
    if flags.positionals.len() > 1 {
        return Err(fail_with_command_help(
            path,
            &format!("Unexpected argument: {}", flags.positionals[1]),
        ));
    }
    Ok((
        flags.positionals.swap_remove(0),
        flags.base_dir,
        flags.dev_url,
    ))
}
