//! Argument parsing for `t3 thread list|read|show|send|wait`.

use t3_remote::thread_client::ThreadSendMode;

use crate::cli_global::fail_with_command_help;
use crate::cmd_thread::start::{ThreadStartOpts, parse_start};
use crate::fallback::take_flag_value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedThreadCli {
    pub(crate) remote_url: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) action: ThreadAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThreadAction {
    List {
        base_dir: Option<String>,
        project: Option<String>,
        all: bool,
        json: bool,
    },
    Start(ThreadStartOpts),
    Read {
        base_dir: Option<String>,
        thread_id: String,
        view: Option<String>,
        limit: Option<u32>,
        after_position: Option<i64>,
        json: bool,
    },
    Send {
        base_dir: Option<String>,
        thread_id: String,
        message: String,
        mode: ThreadSendMode,
        output_schema: Option<String>,
        wait: bool,
        json: bool,
    },
    Wait {
        base_dir: Option<String>,
        thread_id: String,
        run_id: Option<String>,
        timeout_ms: Option<u64>,
        json: bool,
    },
}

impl ThreadAction {
    pub(crate) fn base_dir(&self) -> Option<&str> {
        match self {
            Self::Start(opts) => opts.base_dir.as_deref(),
            Self::List { base_dir, .. }
            | Self::Read { base_dir, .. }
            | Self::Send { base_dir, .. }
            | Self::Wait { base_dir, .. } => base_dir.as_deref(),
        }
    }
}

pub(crate) fn parse_thread_cli(rest: &[String]) -> Result<ParsedThreadCli, i32> {
    let Some(subcmd) = rest.first().map(String::as_str) else {
        return Err(fail_with_command_help(
            &["thread"],
            "Missing subcommand for `t3 thread`.",
        ));
    };
    let tail = rest.get(1..).unwrap_or(&[]);

    match subcmd {
        "list" => parse_list(tail),
        "start" => parse_start(tail),
        "read" | "show" => parse_read(tail, subcmd),
        "send" => parse_send(tail),
        "wait" => crate::cmd_thread::parse_wait(tail),
        other => Err(fail_with_command_help(
            &["thread"],
            &format!("Unknown subcommand \"{other}\" for \"t3 thread\""),
        )),
    }
}

#[derive(Default)]
pub(crate) struct ThreadCommonOpts {
    pub(crate) base_dir: Option<String>,
    pub(crate) remote_url: Option<String>,
    pub(crate) token: Option<String>,
}

pub(crate) fn take_common_opt(
    path: &[&str],
    tail: &[String],
    idx: &mut usize,
    common: &mut ThreadCommonOpts,
) -> Result<bool, i32> {
    if let Some(val) = take_thr_opt(path, tail, idx, "base-dir")? {
        common.base_dir = Some(val);
        Ok(true)
    } else if let Some(val) = take_thr_opt(path, tail, idx, "remote-url")? {
        if !val.starts_with("http://") && !val.starts_with("https://") {
            return Err(fail_with_command_help(
                path,
                &format!(
                    "Invalid value for flag --remote-url: \"{val}\". Expected an http:// or https:// URL."
                ),
            ));
        }
        common.remote_url = Some(val);
        Ok(true)
    } else if let Some(val) = take_thr_opt(path, tail, idx, "token")? {
        common.token = Some(val);
        Ok(true)
    } else if let Some(val) = take_thr_opt(path, tail, idx, "dev-url")? {
        if !val.starts_with("http://") && !val.starts_with("https://") {
            return Err(fail_with_command_help(
                path,
                &format!(
                    "Invalid value for flag --dev-url: \"{val}\". Expected an http:// or https:// URL."
                ),
            ));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(crate) fn bad_flag(path: &[&str], arg: &str, sub: &str) -> i32 {
    fail_with_command_help(
        path,
        &format!("Unrecognized flag: {arg} in command t3 thread {sub}"),
    )
}

pub(crate) fn take_thr_opt(
    path: &[&str],
    tail: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    take_flag_value(tail, idx, name)
        .map_err(|()| fail_with_command_help(path, &format!("Missing value for flag --{name}")))
}

fn parse_list(tail: &[String]) -> Result<ParsedThreadCli, i32> {
    let path = &["thread", "list"];
    let mut common = ThreadCommonOpts::default();
    let mut project = None;
    let mut all = false;
    let mut json = false;
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if arg == "--json" {
            json = true;
        } else if arg == "--all" {
            all = true;
        } else if take_common_opt(path, tail, &mut idx, &mut common)? {
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "project")? {
            project = Some(val);
        } else if arg.starts_with('-') {
            return Err(bad_flag(path, arg, "list"));
        } else {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected argument: {arg}"),
            ));
        }
        idx += 1;
    }
    Ok(ParsedThreadCli {
        remote_url: common.remote_url,
        token: common.token,
        action: ThreadAction::List {
            base_dir: common.base_dir,
            project,
            all,
            json,
        },
    })
}

pub(crate) fn take_single_thread(path: &[&str], mut pos: Vec<String>) -> Result<String, i32> {
    if pos.is_empty() {
        return Err(fail_with_command_help(
            path,
            "Missing required argument: thread",
        ));
    }
    if pos.len() > 1 {
        return Err(fail_with_command_help(
            path,
            &format!("Unexpected argument: {}", pos[1]),
        ));
    }
    Ok(pos.swap_remove(0))
}

fn parse_read(tail: &[String], sub: &str) -> Result<ParsedThreadCli, i32> {
    let path: &[&str] = if sub == "show" {
        &["thread", "show"]
    } else {
        &["thread"]
    };
    let mut common = ThreadCommonOpts::default();
    let mut view = None;
    let mut limit = None;
    let mut after_position = None;
    let mut json = false;
    let mut pos = Vec::new();
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if arg == "--json" {
            json = true;
        } else if take_common_opt(path, tail, &mut idx, &mut common)? {
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "view")? {
            view = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "limit")?
            .or(take_thr_opt(path, tail, &mut idx, "messages")?)
        {
            limit = Some(
                val.trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (1..=100).contains(n))
                    .ok_or_else(|| {
                        fail_with_command_help(path, "Invalid --limit value; expected 1..=100.")
                    })?,
            );
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "after")? {
            after_position = Some(
                val.trim()
                    .parse::<i64>()
                    .map_err(|_| fail_with_command_help(path, "Invalid --after position."))?,
            );
        } else if arg.starts_with('-') {
            return Err(bad_flag(path, arg, sub));
        } else {
            pos.push(arg.clone());
        }
        idx += 1;
    }

    Ok(ParsedThreadCli {
        remote_url: common.remote_url,
        token: common.token,
        action: ThreadAction::Read {
            base_dir: common.base_dir,
            thread_id: take_single_thread(path, pos)?,
            view,
            limit,
            after_position,
            json,
        },
    })
}

fn parse_send(tail: &[String]) -> Result<ParsedThreadCli, i32> {
    let path = &["thread", "send"];
    let mut common = ThreadCommonOpts::default();
    let mut mode = ThreadSendMode::Queue;
    let mut output_schema = None;
    let mut wait = false;
    let mut json = false;
    let mut pos = Vec::new();
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if arg == "--json" {
            json = true;
        } else if arg == "--wait" {
            wait = true;
        } else if take_common_opt(path, tail, &mut idx, &mut common)? {
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "output-schema")? {
            output_schema = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "mode")? {
            mode = match val.trim() {
                "auto" => ThreadSendMode::Auto,
                "queue" => ThreadSendMode::Queue,
                "steer" => ThreadSendMode::Steer,
                "restart" => ThreadSendMode::Restart,
                other => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unsupported send mode `{other}`."),
                    ));
                }
            };
        } else if arg.starts_with('-') && arg != "-" {
            return Err(bad_flag(path, arg, "send"));
        } else {
            pos.push(arg.clone());
        }
        idx += 1;
    }

    if pos.len() < 2 {
        return Err(fail_with_command_help(
            path,
            "Missing required arguments: <thread> <message>",
        ));
    }
    if pos.len() > 2 {
        return Err(fail_with_command_help(
            path,
            &format!("Unexpected argument: {}", pos[2]),
        ));
    }
    let message = pos.swap_remove(1);
    let thread_id = pos.swap_remove(0);
    Ok(ParsedThreadCli {
        remote_url: common.remote_url,
        token: common.token,
        action: ThreadAction::Send {
            base_dir: common.base_dir,
            thread_id,
            message,
            mode,
            output_schema,
            wait,
            json,
        },
    })
}
