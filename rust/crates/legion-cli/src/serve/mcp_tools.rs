//! Built-in `/mcp` tool definitions and dispatchers for `legion_project_list`
//! and `legion_thread_{list,read,send,wait}` so `legioncode thread` and ACP bridges work standalone.

#[path = "mcp_launch_output.rs"]
mod launch_output;
#[path = "mcp_queries.rs"]
mod queries;

use std::sync::Arc;
use std::time::Duration;

use legion_contracts::brand::{ACTIVE_BRAND, canonical_mcp_tool_suffix};
use legion_contracts::ids::{CommandId, MessageId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DispatchMode, MessageDispatchCommand, OrchestrationActor, OrchestrationCommand,
};
use legion_orchestrator::decider::output_ops::validate_output_request_for_message;
use legion_workflows::feedback_types::FeedbackMode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::AppState;
use super::mcp_milestones::McpCallerContext;
use super::mcp_wf_feedback::{feedback_tool, workflow_tool};
use super::rpc::workflows_feedback::read_feedback_mode;
use launch_output::{parse_output_schema_arg, submit_output, thread_launch};
use queries::{
    evaluate_wait_wake_for_thread, project_list, read_run_output_for, run_status_for, thread_list,
    thread_read,
};

/// Metadata returned for `tools/list` on `/mcp`.
pub(super) fn list_tools_json(state: &AppState, caller: &McpCallerContext) -> Value {
    let prefix = ACTIVE_BRAND.mcp_tool_prefix;
    let name = |suffix: &str| format!("{prefix}{suffix}");
    let mut tools = vec![
        json!({ "name": name("project_list"), "description": format!("List active {} projects.", ACTIVE_BRAND.product_name), "inputSchema": { "type": "object" } }),
        json!({ "name": name("thread_list"), "description": "List threads in a project.", "inputSchema": { "type": "object" } }),
        json!({ "name": name("thread_read"), "description": "Read a thread and its messages.", "inputSchema": { "type": "object" } }),
        json!({ "name": name("thread_launch"), "description": "Create a new top-level thread.", "inputSchema": { "type": "object" } }),
        json!({ "name": name("thread_send"), "description": "Send a message to a thread.", "inputSchema": { "type": "object" } }),
        json!({ "name": name("submit_output"), "description": "Submit a schema-checked JSON result for the active run.", "inputSchema": { "type": "object" } }),
        json!({ "name": name("thread_wait"), "description": "Wait for a thread run to settle.", "inputSchema": { "type": "object" } }),
    ];
    if caller.shows_outside_only() {
        tools.push(json!({
            "name": name("workflow"),
            "description": "Durable workflow runs on this server (start, list, read, questions, hooks, store, signal, resume, cancel).",
            "inputSchema": { "type": "object" }
        }));
    }
    if read_feedback_mode(state) != FeedbackMode::Off {
        tools.push(json!({
            "name": name("feedback"),
            "description": "Record local feedback for a tool or command when agent feedback is enabled.",
            "inputSchema": { "type": "object" }
        }));
    }
    json!({ "tools": tools })
}

/// Executes one `/mcp` `tools/call` dispatching `legion_*` (and `t3_*`) tool names.
pub(super) async fn call_tool(
    state: &Arc<AppState>,
    caller: &McpCallerContext,
    name: &str,
    args: &Value,
) -> Value {
    let canonical = canonical_mcp_tool_suffix(name);
    let outcome = match canonical {
        "project_list" => with_db(state, |conn| project_list(conn, args)),
        "thread_list" => with_db(state, |conn| thread_list(conn, args)),
        "thread_read" => match required_str(args, "threadId") {
            Ok(tid) => with_db(state, |conn| thread_read(conn, tid, args)),
            Err(err) => Err(err),
        },
        "thread_launch" => thread_launch(state, args).await,
        "thread_send" => thread_send(state, args).await,
        "submit_output" => submit_output(state, args).await,
        "thread_wait" => thread_wait(state, caller, args).await,
        "workflow" => workflow_tool(state, caller, args).await,
        "feedback" => feedback_tool(state, caller, args),
        _ => Err(format!("Unknown MCP tool `{name}`.")),
    };

    match outcome {
        Ok(structured) => {
            let text = serde_json::to_string(&structured).unwrap_or_else(|_| "{}".to_owned());
            json!({
                "content": [{ "type": "text", "text": text }],
                "structuredContent": structured,
                "isError": false
            })
        }
        Err(message) => {
            let err_text = json!({ "message": message }).to_string();
            json!({
                "content": [{ "type": "text", "text": err_text }],
                "isError": true
            })
        }
    }
}

fn with_db<F>(state: &AppState, op: F) -> Result<Value, String>
where
    F: FnOnce(&rusqlite::Connection) -> Result<Value, String>,
{
    let conn = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| "database mutex poisoned".to_owned())?;
    op(&conn)
}

async fn thread_send(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let thread_id_str = required_str(args, "threadId")?;
    let message = required_str(args, "message")?.to_owned();
    let thread_id = ThreadId::new(thread_id_str).map_err(|e| e.to_string())?;
    let command_id =
        CommandId::new(format!("mcp-cmd-{}", Uuid::new_v4())).map_err(|e| e.to_string())?;
    let message_id =
        MessageId::new(format!("mcp-msg-{}", Uuid::new_v4())).map_err(|e| e.to_string())?;
    let delivery_intent = args
        .get("mode")
        .and_then(Value::as_str)
        .filter(|m| matches!(*m, "steer" | "restart"))
        .map(str::to_owned);

    let output_schema = parse_output_schema_arg(args)?;
    validate_output_request_for_message(&message, &[], output_schema.as_ref())
        .map_err(|e| e.to_string())?;
    let dispatch_mode = if output_schema.is_some() {
        DispatchMode::QueueAfterActive
    } else {
        DispatchMode::StartImmediately
    };

    let cmd = OrchestrationCommand::MessageDispatch(MessageDispatchCommand {
        notification: None,
        guest_message_id: None,
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        scheduled_task_id: None,
        sender_thread_id: None,
        linked_sender: None,
        idle_compaction: None,
        fresh_context: None,
        output_schema,
        command_id,
        thread_id: thread_id.clone(),
        message_id: message_id.clone(),
        text: message,
        context: None,
        attachments: Vec::new(),
        title_seed: None,
        model_selection: None,
        source_plan_ref: None,
        restart_continuation_of_run_id: None,
        usage_limit_continuation_of_run_id: None,
        manual_continuation_of_run_id: None,
        steer_follow_up_of_run_id: None,
        steer_follow_up_after_run_ordinal: None,
        usage_limit_recovery_request_id: None,
        delivery_intent,
        delegated_completion: None,
        dispatch_mode,
    });
    state
        .orchestrator
        .dispatch_now(&cmd)
        .await
        .map_err(|e| e.to_string())?;

    let (run_id, status) = {
        let conn = state
            .orchestrator
            .sink()
            .db()
            .lock()
            .map_err(|_| "database mutex poisoned".to_owned())?;
        let (rid, st) = run_status_for(&conn, thread_id.as_str(), None)?;
        (rid.unwrap_or_default(), st)
    };
    let delivery = if status == "queued" {
        "queued"
    } else {
        "started"
    };
    Ok(json!({
        "threadId": thread_id_str,
        "messageId": message_id.as_str(),
        "runId": run_id,
        "status": status,
        "delivery": delivery,
    }))
}

async fn thread_wait(
    state: &Arc<AppState>,
    caller: &McpCallerContext,
    args: &Value,
) -> Result<Value, String> {
    let thread_id = required_str(args, "threadId")?.to_owned();
    let target_run_id = args.get("runId").and_then(Value::as_str).map(str::to_owned);
    let timeout_ms = args
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(15_000)
        .clamp(1, 45_000);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut rx = state.orchestrator.subscribe();

    loop {
        let (run_id, status, output) = {
            let conn = state
                .orchestrator
                .sink()
                .db()
                .lock()
                .map_err(|_| "database mutex poisoned".to_owned())?;
            let (rid, st) = run_status_for(&conn, &thread_id, target_run_id.as_deref())?;
            let out = read_run_output_for(&conn, &thread_id, rid.as_deref());
            (rid, st, out)
        };
        if is_terminal_run_status(&status) || run_id.is_none() {
            let mut resp = json!({
                "threadId": thread_id,
                "runId": run_id,
                "status": status,
                "timedOut": false,
            });
            if let Some(val) = output
                && let Some(obj) = resp.as_object_mut()
            {
                obj.insert("output".to_owned(), val);
            }
            return Ok(resp);
        }
        let now = tokio::time::Instant::now();
        if now >= deadline
            || tokio::time::timeout(deadline - now, rx.recv())
                .await
                .is_err()
        {
            let parent_tid = caller
                .thread_id
                .as_deref()
                .or_else(|| args.get("parentThreadId").and_then(Value::as_str));
            let wake = state.orchestrator.sink().db().lock().ok().and_then(|conn| {
                evaluate_wait_wake_for_thread(
                    &conn,
                    parent_tid,
                    &thread_id,
                    run_id.as_deref(),
                    &status,
                )
            });
            let mut out = json!({
                "threadId": thread_id,
                "runId": run_id,
                "status": status,
                "timedOut": true,
            });
            if let Some(decision) = wake
                && let Some(obj) = out.as_object_mut()
            {
                obj.insert("wakesCaller".to_owned(), Value::Bool(decision.wakes_caller));
                if let Some(hint) = decision.next_hint {
                    obj.insert("nextHint".to_owned(), Value::String(hint.to_owned()));
                }
            }
            return Ok(out);
        }
    }
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing required `{key}` parameter."))
}

fn is_terminal_run_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "interrupted" | "rolled_back" | "idle"
    )
}
