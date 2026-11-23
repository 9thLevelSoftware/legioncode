//! Argument parsing helpers for `legioncode workflow`.

use std::path::PathBuf;

use serde_json::Value;

use crate::cli_global::fail_with_command_help;

#[path = "cmd_workflow_args_signal_store.rs"]
mod signal_store;
use signal_store::{parse_no_pos, parse_signal, parse_store};

#[derive(Debug, Clone, Default)]
pub(super) struct CommonFlags {
    pub(super) base_dir: Option<PathBuf>,
    pub(super) remote_url: Option<String>,
    pub(super) token: Option<String>,
    pub(super) json: bool,
}

#[derive(Debug, Clone)]
pub(super) enum WorkflowAction {
    Start {
        workflow: String,
        key: String,
        input: Value,
    },
    List {
        status: Option<String>,
    },
    Show {
        run_id: String,
    },
    Resume {
        run_id: String,
    },
    Cancel {
        run_id: String,
        reason: Option<String>,
    },
    Signal {
        value: Value,
        run_id: Option<String>,
        wait: Option<String>,
        workflow: Option<String>,
        channel: Option<String>,
        signal_id: Option<String>,
    },
    Questions,
    Hooks,
    StoreList {
        prefix: String,
    },
    StoreSet {
        key: String,
        value: Value,
    },
    StoreDelete {
        key: String,
    },
}

pub(super) fn parse_workflow_cli(args: &[String]) -> Result<(CommonFlags, WorkflowAction), i32> {
    let path = &["workflow"];
    let Some(sub) = args.first().map(String::as_str) else {
        return Err(fail_with_command_help(path, "Missing workflow subcommand."));
    };
    let tail = &args[1..];
    match sub {
        "start" => parse_start(tail),
        "list" => parse_list(tail),
        "show" | "read" => parse_run_only(path, tail, |run_id| WorkflowAction::Show { run_id }),
        "resume" => parse_run_only(path, tail, |run_id| WorkflowAction::Resume { run_id }),
        "cancel" => parse_cancel(tail),
        "signal" => parse_signal(tail),
        "questions" => parse_no_pos(path, tail, WorkflowAction::Questions),
        "hooks" => parse_no_pos(path, tail, WorkflowAction::Hooks),
        "store" => parse_store(tail),
        other => Err(fail_with_command_help(
            path,
            &format!("Unknown workflow subcommand `{other}`."),
        )),
    }
}

fn take_opt_val(
    path: &[&str],
    args: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    let arg = &args[*idx];
    let flag = format!("--{name}");
    if arg == &flag {
        *idx += 1;
        let Some(val) = args.get(*idx) else {
            return Err(fail_with_command_help(
                path,
                &format!("Missing value for --{name}."),
            ));
        };
        return Ok(Some(val.clone()));
    }
    let prefix = format!("{flag}=");
    Ok(arg.strip_prefix(&prefix).map(str::to_owned))
}

fn take_common(
    path: &[&str],
    args: &[String],
    idx: &mut usize,
    common: &mut CommonFlags,
) -> Result<bool, i32> {
    if args[*idx] == "--json" {
        common.json = true;
        return Ok(true);
    }
    if let Some(v) = take_opt_val(path, args, idx, "base-dir")? {
        common.base_dir = Some(PathBuf::from(v));
        return Ok(true);
    }
    if let Some(v) = take_opt_val(path, args, idx, "remote-url")? {
        common.remote_url = Some(v);
        return Ok(true);
    }
    if let Some(v) = take_opt_val(path, args, idx, "token")? {
        common.token = Some(v);
        return Ok(true);
    }
    Ok(false)
}

fn parse_start(tail: &[String]) -> Result<(CommonFlags, WorkflowAction), i32> {
    let path = &["workflow", "start"];
    let mut common = CommonFlags::default();
    let mut key: Option<String> = None;
    let mut input_raw = "{}".to_owned();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < tail.len() {
        if take_common(path, tail, &mut i, &mut common)? {
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "key")? {
            key = Some(v);
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "input")? {
            input_raw = v;
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
    let Some(workflow) = pos.into_iter().next() else {
        return Err(fail_with_command_help(path, "Missing workflow name."));
    };
    let Some(key_val) = key.filter(|k| !k.trim().is_empty()) else {
        return Err(fail_with_command_help(path, "--key cannot be blank."));
    };
    let input: Value = serde_json::from_str(&input_raw)
        .map_err(|_| fail_with_command_help(path, "--input is not valid JSON."))?;
    Ok((
        common,
        WorkflowAction::Start {
            workflow,
            key: key_val,
            input,
        },
    ))
}

fn parse_list(tail: &[String]) -> Result<(CommonFlags, WorkflowAction), i32> {
    let path = &["workflow", "list"];
    let mut common = CommonFlags::default();
    let mut status = None;
    let mut i = 0;
    while i < tail.len() {
        if take_common(path, tail, &mut i, &mut common)? {
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "status")? {
            status = Some(v);
        } else {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected `{}`.", tail[i]),
            ));
        }
        i += 1;
    }
    Ok((common, WorkflowAction::List { status }))
}

fn parse_run_only<F>(
    path: &[&str],
    tail: &[String],
    build: F,
) -> Result<(CommonFlags, WorkflowAction), i32>
where
    F: FnOnce(String) -> WorkflowAction,
{
    let mut common = CommonFlags::default();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < tail.len() {
        if take_common(path, tail, &mut i, &mut common)? {
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
    let Some(run_id) = pos.into_iter().next() else {
        return Err(fail_with_command_help(path, "Missing workflow run id."));
    };
    Ok((common, build(run_id)))
}

fn parse_cancel(tail: &[String]) -> Result<(CommonFlags, WorkflowAction), i32> {
    let path = &["workflow", "cancel"];
    let mut common = CommonFlags::default();
    let mut reason = None;
    let mut pos = Vec::new();
    let mut i = 0;
    while i < tail.len() {
        if take_common(path, tail, &mut i, &mut common)? {
        } else if let Some(v) = take_opt_val(path, tail, &mut i, "reason")? {
            reason = Some(v);
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
    let Some(run_id) = pos.into_iter().next() else {
        return Err(fail_with_command_help(path, "Missing workflow run id."));
    };
    Ok((common, WorkflowAction::Cancel { run_id, reason }))
}
