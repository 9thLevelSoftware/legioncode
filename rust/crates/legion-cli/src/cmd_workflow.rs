#![forbid(unsafe_code)]
//! Standalone `legioncode workflow` CLI subcommands backed by `legion_workflow` over `/mcp`.

use std::path::PathBuf;

use legion_contracts::brand::ACTIVE_BRAND;
use legion_remote::acp_mcp_bridge::McpHttpClient;
use legion_remote::thread_client::discover_live_server_runtime;
use serde_json::{Value, json};

use crate::cmd_thread::ops::{
    acquire_local_thread_auth, extract_mcp_tool_structured, print_json_value,
    revoke_fallback_thread_session,
};
use crate::fallback::{
    delegate_to_node, resolve_base_dir, unit_forced_original, write_stdout_line,
};

#[path = "cmd_workflow_args.rs"]
mod args;
use args::{CommonFlags, WorkflowAction, parse_workflow_cli};

/// Unit identifier for `RUST_ANYTHING_UNIT_WORKFLOW` fallback control.
pub const UNIT_WORKFLOW: &str = "WORKFLOW";

/// Executes `legioncode workflow ...`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_WORKFLOW) {
        return delegate_to_node(UNIT_WORKFLOW, args);
    }
    let rest = args.get(1..).unwrap_or(&[]);
    let (common, action) = match parse_workflow_cli(rest) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `legioncode workflow`.");
        return 1;
    };
    rt.block_on(execute(common, action))
}

async fn execute(common: CommonFlags, action: WorkflowAction) -> i32 {
    let (origin, endpoint, bearer, local_session) = match resolve_target(&common).await {
        Ok(t) => t,
        Err(code) => return code,
    };
    let Ok(mcp) = McpHttpClient::connect(
        &endpoint,
        format!("Bearer {bearer}"),
        "legioncode-workflow-cli",
    )
    .await
    else {
        revoke_fallback_thread_session(local_session.as_ref());
        eprintln!("Could not reach the LegionCode server at {origin}.");
        return 1;
    };
    let tool_name = format!("{}workflow", ACTIVE_BRAND.mcp_tool_prefix);
    let args = build_tool_args(&action);
    let res = mcp.call_tool(&tool_name, &args).await;
    revoke_fallback_thread_session(local_session.as_ref());
    match res {
        Ok(raw) => match extract_mcp_tool_structured(raw) {
            Ok(structured) => render_result(&action, &structured, common.json),
            Err(err) => {
                eprintln!("{err}");
                1
            }
        },
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

async fn resolve_target(
    common: &CommonFlags,
) -> Result<(String, String, String, Option<(PathBuf, String)>), i32> {
    if let Some(ref remote_url) = common.remote_url {
        let base = remote_url.trim_end_matches('/').to_owned();
        let Some(tok) = common
            .token
            .clone()
            .or_else(|| ACTIVE_BRAND.read_env("TOKEN"))
        else {
            eprintln!("--remote-url requires --token.");
            return Err(1);
        };
        return Ok((base.clone(), format!("{base}/mcp"), tok, None));
    }
    let (base_dir, _) =
        resolve_base_dir(common.base_dir.as_deref().and_then(std::path::Path::to_str));
    let discovered = discover_live_server_runtime(&base_dir).map_err(|err| {
        eprintln!("{err}");
        err.exit_code()
    })?;
    let db_path = discovered.state_dir.join("statev2.sqlite");
    let (bearer, sid) = acquire_local_thread_auth(&db_path, &discovered.runtime.origin)
        .await
        .map_err(|msg| {
            eprintln!("{msg}");
            1
        })?;
    Ok((
        discovered.runtime.origin,
        discovered.mcp_endpoint,
        bearer,
        sid.map(|id| (db_path, id)),
    ))
}

fn build_tool_args(action: &WorkflowAction) -> Value {
    match action {
        WorkflowAction::Start {
            workflow,
            key,
            input,
        } => json!({
            "action": "start",
            "workflow": workflow,
            "idempotencyKey": key,
            "input": input,
        }),
        WorkflowAction::List { status } => {
            let mut v = json!({ "action": "list" });
            if let Some(s) = status
                && let Some(obj) = v.as_object_mut()
            {
                obj.insert("status".to_owned(), Value::String(s.clone()));
            }
            v
        }
        WorkflowAction::Show { run_id } => json!({ "action": "read", "runId": run_id }),
        WorkflowAction::Resume { run_id } => json!({ "action": "resume", "runId": run_id }),
        WorkflowAction::Cancel { run_id, reason } => json!({
            "action": "cancel",
            "runId": run_id,
            "reason": reason,
        }),
        WorkflowAction::Signal {
            value,
            run_id,
            wait,
            workflow,
            channel,
            signal_id,
        } => json!({
            "action": "signal",
            "value": value,
            "runId": run_id,
            "wait": wait,
            "workflow": workflow,
            "channel": channel,
            "signalId": signal_id,
        }),
        WorkflowAction::Questions => json!({ "action": "questions" }),
        WorkflowAction::Hooks => json!({ "action": "hooks" }),
        WorkflowAction::StoreList { prefix } => json!({ "action": "store", "key": prefix }),
        WorkflowAction::StoreSet { key, value } => json!({
            "action": "store_set",
            "key": key,
            "value": value,
        }),
        WorkflowAction::StoreDelete { key } => json!({ "action": "store_delete", "key": key }),
    }
}

fn render_result(action: &WorkflowAction, val: &Value, json_out: bool) -> i32 {
    if json_out {
        return print_json_value(val);
    }
    match action {
        WorkflowAction::List { status } => {
            let Some(runs) = val.get("runs").and_then(Value::as_array) else {
                return print_json_value(val);
            };
            for r in runs {
                let st = r.get("status").and_then(Value::as_str).unwrap_or("");
                let has_parent = r.get("parentRunId").is_some_and(|p| !p.is_null());
                if status.is_none() && st == "skipped" && !has_parent {
                    continue;
                }
                let id = r.get("runId").and_then(Value::as_str).unwrap_or("");
                let wf = r.get("workflow").and_then(Value::as_str).unwrap_or("");
                let title = r.get("title").and_then(Value::as_str).unwrap_or("");
                if write_stdout_line(&format!("{id}  {st:<9}  {wf}  {title}"), "workflow list") != 0
                {
                    return 1;
                }
            }
            0
        }
        WorkflowAction::Signal { .. } => {
            let msg = if val.get("delivery").and_then(Value::as_str) == Some("delivered") {
                "Delivered."
            } else {
                "Kept until a run waits for it."
            };
            write_stdout_line(msg, "workflow signal")
        }
        WorkflowAction::StoreSet { key, .. } => {
            write_stdout_line(&format!("Set {key}."), "workflow store")
        }
        WorkflowAction::StoreDelete { key } => {
            write_stdout_line(&format!("Deleted {key}."), "workflow store")
        }
        _ => print_json_value(val),
    }
}
