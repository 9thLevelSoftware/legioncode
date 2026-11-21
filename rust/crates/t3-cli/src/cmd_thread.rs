//! Standalone `t3 thread list|read|send|wait` commands backed by `t3_remote::thread_client`.

#[path = "cmd_thread_ops.rs"]
mod ops;
#[path = "cmd_thread_start.rs"]
pub(crate) mod start;

use std::path::Path;

use serde::Deserialize;
use serde_json::json;
use t3_contracts::brand::ACTIVE_BRAND;
use t3_remote::acp_mcp_bridge::McpHttpClient;
use t3_remote::thread_client::{
    ThreadClientError, ThreadListRequest, ThreadMcpClient, discover_live_server_runtime,
    format_thread_list_line,
};

use crate::cli_global::fail_with_command_help;
use crate::cmd_project_args::normalize_workspace_root;
use crate::cmd_thread_args::{
    ParsedThreadCli, ThreadAction, ThreadCommonOpts, bad_flag, parse_thread_cli, take_common_opt,
    take_single_thread, take_thr_opt,
};
use crate::cmd_trace::parse_duration_ms;
use crate::fallback::{
    delegate_to_node, resolve_base_dir, unit_forced_original, write_stdout_line,
};
use ops::{
    acquire_local_thread_auth, print_json_value, revoke_fallback_thread_session, run_read,
    run_send, run_wait,
};

/// Unit identifier for `RUST_ANYTHING_UNIT_THREAD` and fallback logs.
pub const UNIT_THREAD: &str = "THREAD";

/// Runs `t3 thread` (`list|read|show|send|wait`) natively unless `RUST_ANYTHING_UNIT_THREAD=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_THREAD) {
        return delegate_to_node(UNIT_THREAD, args);
    }

    let parsed = match parse_thread_cli(args.get(1..).unwrap_or(&[])) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `t3 thread`.");
        return 1;
    };
    rt.block_on(execute(parsed))
}

type McpTarget = (String, String, String, Option<(std::path::PathBuf, String)>);

async fn resolve_mcp_target(parsed: &ParsedThreadCli) -> Result<McpTarget, i32> {
    if let Some(ref remote_url) = parsed.remote_url {
        let base = remote_url.trim_end_matches('/').to_owned();
        let tok = parsed
            .token
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                ACTIVE_BRAND
                    .read_env("TOKEN")
                    .or_else(|| ACTIVE_BRAND.read_env("BEARER_TOKEN"))
                    .map(|s| s.trim().to_owned())
            });
        let Some(tok) = tok else {
            eprintln!(
                "--remote-url requires --token <bearer> (or {}TOKEN).",
                ACTIVE_BRAND.primary_env_prefix
            );
            return Err(1);
        };
        return Ok((base.clone(), format!("{base}/mcp"), tok, None));
    }

    let (base_dir, _) = resolve_base_dir(parsed.action.base_dir());
    let discovered = discover_live_server_runtime(&base_dir).map_err(|err| {
        eprintln!("{err}");
        err.exit_code()
    })?;
    let db_path = discovered.state_dir.join("statev2.sqlite");
    let (bearer, session_id) = acquire_local_thread_auth(&db_path, &discovered.runtime.origin)
        .await
        .map_err(|msg| {
            eprintln!("{msg}");
            1
        })?;
    Ok((
        discovered.runtime.origin,
        discovered.mcp_endpoint,
        bearer,
        session_id.map(|id| (db_path, id)),
    ))
}

async fn execute(parsed: ParsedThreadCli) -> i32 {
    let (origin, mcp_endpoint, bearer, local_db_session) = match resolve_mcp_target(&parsed).await {
        Ok(target) => target,
        Err(code) => return code,
    };

    let auth_header = format!("Bearer {bearer}");
    let mcp = match McpHttpClient::connect(&mcp_endpoint, auth_header, "t3-thread-cli").await {
        Ok(mcp) => mcp,
        Err(err) => {
            revoke_fallback_thread_session(local_db_session.as_ref());
            eprintln!("Could not reach the T3 Code server at {origin}. {err}");
            return 1;
        }
    };
    let client = ThreadMcpClient::from_http_client(&origin, mcp.clone());

    let result = match parsed.action {
        ThreadAction::List {
            project, all, json, ..
        } => run_list(&mcp, &client, project.as_deref(), all, json).await,
        ThreadAction::Start(opts) => start::run_start(&mcp, &client, opts).await,
        ThreadAction::Read {
            thread_id,
            view,
            limit,
            after_position,
            json,
            ..
        } => run_read(&client, &thread_id, view, limit, after_position, json).await,
        ThreadAction::Send {
            thread_id,
            message,
            mode,
            output_schema,
            wait,
            json,
            ..
        } => {
            run_send(
                &client,
                &thread_id,
                &message,
                mode,
                output_schema.as_deref(),
                wait,
                json,
            )
            .await
        }
        ThreadAction::Wait {
            thread_id,
            run_id,
            timeout_ms,
            json,
            ..
        } => run_wait(&client, &thread_id, run_id.as_deref(), timeout_ms, json).await,
    };

    client.close().await;
    revoke_fallback_thread_session(local_db_session.as_ref());
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            err.exit_code()
        }
    }
}

async fn run_list(
    mcp: &McpHttpClient,
    client: &ThreadMcpClient,
    project_arg: Option<&str>,
    all: bool,
    json: bool,
) -> Result<i32, ThreadClientError> {
    let project_id = if all && project_arg.is_none() {
        None
    } else {
        let identifier = project_arg.unwrap_or(".");
        Some(resolve_project_id(mcp, identifier).await?)
    };

    let mut threads = Vec::new();
    let mut cursor = Some(0_u32);
    while let Some(page_cursor) = cursor {
        let page = client
            .list_threads(&ThreadListRequest {
                project_id: project_id.clone(),
                all_projects: all.then_some(true),
                include_subagents: Some(false),
                cursor: Some(page_cursor),
                limit: Some(100),
                ..ThreadListRequest::default()
            })
            .await?;
        threads.extend(page.threads);
        cursor = page.next_cursor;
    }

    if json {
        return Ok(print_json_value(&threads));
    }
    for item in &threads {
        let line = format_thread_list_line(item);
        if write_stdout_line(&line, "thread list") != 0 {
            return Ok(1);
        }
    }
    Ok(0)
}

async fn resolve_project_id(
    mcp: &McpHttpClient,
    identifier: &str,
) -> Result<String, ThreadClientError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ProjectEntry {
        id: String,
        workspace_root: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ProjectListStructured {
        projects: Vec<ProjectEntry>,
        next_cursor: Option<u32>,
    }

    let mut all = Vec::new();
    let mut cursor = Some(0_u32);
    while let Some(c) = cursor {
        let raw = mcp
            .call_tool("t3_project_list", &json!({ "cursor": c, "limit": 100 }))
            .await
            .map_err(|err| ThreadClientError::ToolFailed {
                tool: "t3_project_list".to_owned(),
                detail: err.to_string(),
            })?;
        let Some(structured) = raw.get("structuredContent") else {
            break;
        };
        let page: ProjectListStructured =
            serde_json::from_value(structured.clone()).map_err(|err| {
                ThreadClientError::ToolFailed {
                    tool: "t3_project_list".to_owned(),
                    detail: err.to_string(),
                }
            })?;
        all.extend(page.projects);
        cursor = page.next_cursor;
    }

    if let Some(exact) = all.iter().find(|p| p.id == identifier) {
        return Ok(exact.id.clone());
    }
    let normalized =
        normalize_workspace_root(identifier, false).unwrap_or_else(|_| identifier.to_owned());
    let target_path = Path::new(&normalized);
    if let Some(best) = all
        .iter()
        .filter(|p| target_path.starts_with(Path::new(&p.workspace_root)))
        .max_by_key(|p| p.workspace_root.len())
    {
        return Ok(best.id.clone());
    }
    Err(ThreadClientError::ToolFailed {
        tool: "t3_project_list".to_owned(),
        detail: format!(
            "No T3 Code project matches '{identifier}'. Pass a project id or a path inside a project."
        ),
    })
}

pub(crate) fn parse_wait(tail: &[String]) -> Result<ParsedThreadCli, i32> {
    let path = &["thread"];
    let mut common = ThreadCommonOpts::default();
    let mut run_id = None;
    let mut timeout_ms = None;
    let mut json = false;
    let mut pos = Vec::new();
    let mut idx = 0;

    while idx < tail.len() {
        let arg = &tail[idx];
        if arg == "--json" {
            json = true;
        } else if take_common_opt(path, tail, &mut idx, &mut common)? {
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "run")? {
            run_id = Some(val);
        } else if let Some(val) = take_thr_opt(path, tail, &mut idx, "timeout")? {
            let ms = parse_duration_ms(&val)
                .filter(|d| *d > 0)
                .ok_or_else(|| fail_with_command_help(path, "Invalid duration for --timeout."))?;
            timeout_ms = u64::try_from(ms).ok();
        } else if arg.starts_with('-') {
            return Err(bad_flag(path, arg, "wait"));
        } else {
            pos.push(arg.clone());
        }
        idx += 1;
    }

    Ok(ParsedThreadCli {
        remote_url: common.remote_url,
        token: common.token,
        action: ThreadAction::Wait {
            base_dir: common.base_dir,
            thread_id: take_single_thread(path, pos)?,
            run_id,
            timeout_ms,
            json,
        },
    })
}
