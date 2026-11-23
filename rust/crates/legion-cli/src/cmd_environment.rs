//! Full native `legioncode environment link|list|unlink` CLI parser and dispatcher.

use legion_contracts::orchestration::McpClientAccess;

use crate::cli_global::fail_with_command_help;
use crate::cmd_environment_ops::{EnvLinkArgs, run_link, run_list, run_unlink};
use crate::fallback::{delegate_to_node, take_flag_value, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_ENVIRONMENT` and fallback logs.
pub const UNIT_ENVIRONMENT: &str = "ENVIRONMENT";

/// Runs `legioncode environment link|list|unlink` natively unless `RUST_ANYTHING_UNIT_ENVIRONMENT=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_ENVIRONMENT) {
        return delegate_to_node(UNIT_ENVIRONMENT, args);
    }

    let Some(sub) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(
            &["environment"],
            "Missing subcommand for `legioncode environment`.",
        );
    };
    let tail = args.get(2..).unwrap_or(&[]);

    match sub {
        "link" => match parse_link_args(tail) {
            Ok(parsed) => run_link(&parsed),
            Err(code) => code,
        },
        "list" => match parse_list_flags(tail) {
            Ok((base_dir, dev_url, json)) => {
                run_list(base_dir.as_deref(), dev_url.as_deref(), json)
            }
            Err(code) => code,
        },
        "unlink" => match parse_unlink_args(tail) {
            Ok((base_dir, dev_url, env_id)) => {
                run_unlink(base_dir.as_deref(), dev_url.as_deref(), &env_id)
            }
            Err(code) => code,
        },
        other => fail_with_command_help(
            &["environment"],
            &format!("Unknown subcommand \"{other}\" for \"legioncode environment\""),
        ),
    }
}

fn parse_link_args(tail: &[String]) -> Result<EnvLinkArgs, i32> {
    let path = &["environment", "link"];
    let mut base_dir = None;
    let mut dev_url = None;
    let mut alternate_urls = Vec::new();
    let mut pairing_code = None;
    let mut access = McpClientAccess::ApprovalRequired;
    let mut url_pos = None;
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if let Some(val) = take_env_opt(path, tail, &mut idx, "base-dir")? {
            base_dir = Some(val);
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "dev-url")? {
            validate_http_url(path, "--dev-url", &val)?;
            dev_url = Some(val);
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "url")? {
            alternate_urls.push(val);
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "pairing-code")? {
            pairing_code = Some(val);
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "access")? {
            access = parse_access_choice(path, &val)?;
        } else if arg.starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unrecognized flag: {arg} in command legioncode environment link"),
            ));
        } else if url_pos.is_some() {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected argument: {arg}"),
            ));
        } else {
            url_pos = Some(arg.clone());
        }
        idx += 1;
    }

    let Some(url) = url_pos else {
        return Err(fail_with_command_help(
            path,
            "Missing required argument: url",
        ));
    };
    let Some(pairing_code) = pairing_code else {
        return Err(fail_with_command_help(
            path,
            "Missing required flag: --pairing-code",
        ));
    };
    Ok(EnvLinkArgs {
        base_dir,
        dev_url,
        url,
        alternate_urls,
        pairing_code,
        access,
    })
}

fn parse_list_flags(tail: &[String]) -> Result<(Option<String>, Option<String>, bool), i32> {
    let path = &["environment", "list"];
    let mut base_dir = None;
    let mut dev_url = None;
    let mut json = false;
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if arg == "--json" {
            json = true;
        } else if arg == "--live" {
            // Live probing is enabled by default.
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "base-dir")? {
            base_dir = Some(val);
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "dev-url")? {
            validate_http_url(path, "--dev-url", &val)?;
            dev_url = Some(val);
        } else if arg.starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unrecognized flag: {arg} in command legioncode environment list"),
            ));
        } else {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected argument: {arg}"),
            ));
        }
        idx += 1;
    }
    Ok((base_dir, dev_url, json))
}

fn parse_unlink_args(tail: &[String]) -> Result<(Option<String>, Option<String>, String), i32> {
    let path = &["environment", "unlink"];
    let mut base_dir = None;
    let mut dev_url = None;
    let mut env_id = None;
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if let Some(val) = take_env_opt(path, tail, &mut idx, "base-dir")? {
            base_dir = Some(val);
        } else if let Some(val) = take_env_opt(path, tail, &mut idx, "dev-url")? {
            validate_http_url(path, "--dev-url", &val)?;
            dev_url = Some(val);
        } else if arg.starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unrecognized flag: {arg} in command legioncode environment unlink"),
            ));
        } else if env_id.is_some() {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected argument: {arg}"),
            ));
        } else {
            env_id = Some(arg.clone());
        }
        idx += 1;
    }

    let Some(environment_id) = env_id else {
        return Err(fail_with_command_help(
            path,
            "Missing required argument: environment-id",
        ));
    };
    if environment_id.trim().is_empty() {
        return Err(fail_with_command_help(
            path,
            "Environment id cannot be empty.",
        ));
    }
    Ok((base_dir, dev_url, environment_id))
}

fn take_env_opt(
    path: &[&str],
    tail: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    take_flag_value(tail, idx, name)
        .map_err(|()| fail_with_command_help(path, &format!("Missing value for flag --{name}")))
}

fn validate_http_url(path: &[&str], flag: &str, raw: &str) -> Result<(), i32> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        Ok(())
    } else {
        Err(fail_with_command_help(
            path,
            &format!(
                "Invalid value for flag {flag}: \"{raw}\". Expected an http:// or https:// URL."
            ),
        ))
    }
}

fn parse_access_choice(path: &[&str], raw: &str) -> Result<McpClientAccess, i32> {
    match raw {
        "read-only" => Ok(McpClientAccess::ReadOnly),
        "approval-required" => Ok(McpClientAccess::ApprovalRequired),
        "auto-accept-edits" => Ok(McpClientAccess::AutoAcceptEdits),
        "auto" => Ok(McpClientAccess::Auto),
        "full-access" => Ok(McpClientAccess::FullAccess),
        other => Err(fail_with_command_help(
            path,
            &format!(
                "Invalid value for flag --access: \"{other}\". Expected: \"read-only\" | \"approval-required\" | \"auto-accept-edits\" | \"auto\" | \"full-access\""
            ),
        )),
    }
}
