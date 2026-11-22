//! `t3 thread start` parser and runner (`#28597` `--generate-title`, `#38874` `--output-schema`).

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use t3_contracts::orchestration::validate_run_output_schema_size;
use t3_remote::acp_mcp_bridge::McpHttpClient;
use t3_remote::thread_client::{
    ThreadClientError, ThreadLaunchRequest, ThreadMcpClient, format_title_from_message,
};

use super::ops::{finish_wait, print_json_value, read_message_text};
use super::resolve_project_id;
use crate::cli_global::fail_with_command_help;
use crate::cmd_thread_args::{
    ParsedThreadCli, ThreadAction, ThreadCommonOpts, bad_flag, take_common_opt, take_thr_opt,
};
use crate::fallback::write_stdout_line;

/// Parsed CLI options for `t3 thread start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadStartOpts {
    pub(crate) base_dir: Option<String>,
    pub(crate) project: String,
    pub(crate) message: String,
    pub(crate) title: Option<String>,
    pub(crate) generate_title: bool,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) options: Vec<(String, String)>,
    pub(crate) worktree: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) output_schema: Option<String>,
    pub(crate) wait: bool,
    pub(crate) json: bool,
}

/// Parses `--title`, `--generate-title`, `--provider`, `--model`, `--option`, `--worktree`,
/// `--branch`, `--output-schema`, `--wait`, `--json`, `<project>`, `<message>`.
pub(crate) fn parse_start(tail: &[String]) -> Result<ParsedThreadCli, i32> {
    let path = &["thread", "start"];
    let mut common = ThreadCommonOpts::default();
    let mut title = None;
    let mut generate_title = false;
    let mut provider = None;
    let mut model = None;
    let mut options = Vec::new();
    let mut worktree = None;
    let mut branch = None;
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
        } else if arg == "--generate-title" {
            generate_title = true;
        } else if take_common_opt(path, tail, &mut idx, &mut common)? {
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "title")? {
            title = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "provider")? {
            provider = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "model")? {
            model = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "option")? {
            let Some((k, v)) = val.split_once('=') else {
                return Err(fail_with_command_help(
                    path,
                    "Invalid --option value; expected id=value.",
                ));
            };
            options.push((k.trim().to_owned(), v.trim().to_owned()));
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "worktree")? {
            worktree = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "branch")? {
            branch = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "output-schema")? {
            output_schema = Some(val);
        } else if arg.starts_with('-') && arg != "-" {
            return Err(bad_flag(path, arg, "start"));
        } else {
            pos.push(arg.clone());
        }
        idx += 1;
    }

    if pos.len() < 2 {
        return Err(fail_with_command_help(
            path,
            "Missing required arguments: <project> <message>",
        ));
    }
    if pos.len() > 2 {
        return Err(fail_with_command_help(
            path,
            &format!("Unexpected argument: {}", pos[2]),
        ));
    }
    let message = pos.swap_remove(1);
    let project = pos.swap_remove(0);

    Ok(ParsedThreadCli {
        remote_url: common.remote_url,
        token: common.token,
        action: ThreadAction::Start(ThreadStartOpts {
            base_dir: common.base_dir,
            project,
            message,
            title,
            generate_title,
            provider,
            model,
            options,
            worktree,
            branch,
            output_schema,
            wait,
            json,
        }),
    })
}

/// Reads and validates a `--output-schema <file>` JSON Schema object file.
pub(crate) fn read_output_schema_file(
    raw_path: Option<&str>,
) -> Result<Option<Map<String, Value>>, ThreadClientError> {
    let Some(path_str) = raw_path else {
        return Ok(None);
    };
    let content =
        fs::read_to_string(Path::new(path_str)).map_err(|_| ThreadClientError::Usage {
            detail: format!("{path_str} does not hold a JSON Schema object."),
        })?;
    let Value::Object(map) =
        serde_json::from_str::<Value>(&content).map_err(|_| ThreadClientError::Usage {
            detail: format!("{path_str} does not hold a JSON Schema object."),
        })?
    else {
        return Err(ThreadClientError::Usage {
            detail: format!("{path_str} does not hold a JSON Schema object."),
        });
    };
    validate_run_output_schema_size(&map).map_err(|err| ThreadClientError::Usage {
        detail: err.to_string(),
    })?;
    Ok(Some(map))
}

/// Executes `t3 thread start`.
pub(crate) async fn run_start(
    mcp: &McpHttpClient,
    client: &ThreadMcpClient,
    opts: ThreadStartOpts,
) -> Result<i32, ThreadClientError> {
    let message =
        read_message_text(&opts.message).map_err(|detail| ThreadClientError::Usage { detail })?;
    let schema = read_output_schema_file(opts.output_schema.as_deref())?;
    let project_id = resolve_project_id(mcp, &opts.project).await?;
    let title = opts
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format_title_from_message(&message), str::to_owned);

    let model_selection = if let (Some(provider), Some(model)) = (&opts.provider, &opts.model) {
        let mut sel = json!({ "provider": provider, "model": model });
        if !opts.options.is_empty()
            && let Some(obj) = sel.as_object_mut()
        {
            let pairs: Map<String, Value> = opts
                .options
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            obj.insert("options".to_owned(), Value::Object(pairs));
        }
        Some(sel)
    } else {
        None
    };

    let workspace_strategy = opts.worktree.as_ref().map(|base_ref| {
        let mut map = Map::new();
        map.insert("type".to_owned(), Value::String("worktree".to_owned()));
        map.insert("baseRef".to_owned(), Value::String(base_ref.clone()));
        if let Some(ref b) = opts.branch {
            map.insert("branch".to_owned(), Value::String(b.clone()));
        }
        Value::Object(map)
    });

    let has_schema = schema.is_some();
    let launched = client
        .launch_thread(&ThreadLaunchRequest {
            project_id,
            title: title.clone(),
            generate_title: opts.generate_title.then_some(true),
            message: Some(message),
            model_selection,
            output_schema: schema,
            workspace_strategy,
        })
        .await?;

    if !opts.wait {
        return Ok(if opts.json {
            print_json_value(&json!({
                "threadId": launched.thread_id,
                "runId": launched.run_id,
                "title": title,
            }))
        } else {
            write_stdout_line(&launched.thread_id, "thread start")
        });
    }

    let Some(run_id) = launched.run_id else {
        return Ok(if opts.json {
            print_json_value(&json!({
                "threadId": launched.thread_id,
                "status": "idle",
            }))
        } else {
            write_stdout_line(&launched.thread_id, "thread start")
        });
    };

    finish_wait(
        client,
        &launched.thread_id,
        &run_id,
        None,
        opts.json,
        has_schema,
    )
    .await
}
