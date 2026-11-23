//! Signal, no-pos, and key-value store subcommand parsers for `legioncode workflow`.

use serde_json::Value;

use crate::cli_global::fail_with_command_help;

use super::{CommonFlags, WorkflowAction, take_common, take_opt_val};

pub(super) fn parse_signal(tail: &[String]) -> Result<(CommonFlags, WorkflowAction), i32> {
    let path = &["workflow", "signal"];
    let mut common = CommonFlags::default();
    let mut run_id = None;
    let mut wait = None;
    let mut workflow = None;
    let mut channel = None;
    let mut signal_id = None;
    let mut pos = Vec::new();
    let mut i = 0;
    while i < tail.len() {
        if take_common(path, tail, &mut i, &mut common)? {
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "run")? {
            run_id = Some(v);
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "wait")? {
            wait = Some(v);
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "workflow")? {
            workflow = Some(v);
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "channel")? {
            channel = Some(v);
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "id")? {
            signal_id = Some(v);
        } else if tail[i].starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unknown flag `{}`.", tail[i]),
            ));
        } else {
            pos.push(tail[i].clone());
        }
        i += 1;
    }
    if (run_id.is_some() || wait.is_some()) && (workflow.is_some() || channel.is_some()) {
        return Err(fail_with_command_help(
            path,
            "Address a signal with --run and --wait, or with --workflow and --channel, not both.",
        ));
    }
    let Some(raw_val) = pos.into_iter().next() else {
        return Err(fail_with_command_help(path, "Missing signal JSON value."));
    };
    let value: Value = serde_json::from_str(&raw_val)
        .map_err(|_| fail_with_command_help(path, "The value is not valid JSON."))?;
    Ok((
        common,
        WorkflowAction::Signal {
            value,
            run_id,
            wait,
            workflow,
            channel,
            signal_id,
        },
    ))
}

pub(super) fn parse_no_pos(
    path: &[&str],
    tail: &[String],
    action: WorkflowAction,
) -> Result<(CommonFlags, WorkflowAction), i32> {
    let mut common = CommonFlags::default();
    let mut i = 0;
    while i < tail.len() {
        if !take_common(path, tail, &mut i, &mut common)? {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected `{}`.", tail[i]),
            ));
        }
        i += 1;
    }
    Ok((common, action))
}

pub(super) fn parse_store(tail: &[String]) -> Result<(CommonFlags, WorkflowAction), i32> {
    let path = &["workflow", "store"];
    let Some(sub) = tail.first().map(String::as_str) else {
        return Err(fail_with_command_help(
            path,
            "Missing store subcommand (`list|set|delete`).",
        ));
    };
    let rest = &tail[1..];
    let mut common = CommonFlags::default();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if take_common(path, rest, &mut i, &mut common)? {
        } else if rest[i].starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unknown flag `{}`.", rest[i]),
            ));
        } else {
            pos.push(rest[i].clone());
        }
        i += 1;
    }
    let action = match sub {
        "list" => WorkflowAction::StoreList {
            prefix: pos.into_iter().next().unwrap_or_default(),
        },
        "set" => {
            if pos.len() < 2 {
                return Err(fail_with_command_help(
                    path,
                    "Expected `store set <key> <value>`.",
                ));
            }
            let value: Value = serde_json::from_str(&pos[1])
                .map_err(|_| fail_with_command_help(path, "The value is not valid JSON."))?;
            WorkflowAction::StoreSet {
                key: pos[0].clone(),
                value,
            }
        }
        "delete" => {
            let Some(key) = pos.into_iter().next() else {
                return Err(fail_with_command_help(
                    path,
                    "Expected `store delete <key>`.",
                ));
            };
            WorkflowAction::StoreDelete { key }
        }
        other => {
            return Err(fail_with_command_help(
                path,
                &format!("Unknown store subcommand `{other}`."),
            ));
        }
    };
    Ok((common, action))
}
