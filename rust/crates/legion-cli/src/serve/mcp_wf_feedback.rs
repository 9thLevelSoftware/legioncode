//! `/mcp` tool handlers for `legion_workflow` / `t3_workflow` (`#38882`) and
//! local-only `legion_feedback` / `t3_feedback` (`#35871`, `ADR-013`).

use std::sync::Arc;

use legion_workflows::feedback_types::{
    FEEDBACK_AWAITING_APPROVAL_NOTE, FeedbackMode, FeedbackSource, FeedbackSubmitOutcome,
    SubmitFeedbackInput,
};
use legion_workflows::trace_types::FeedbackKind;
use legion_workflows::types::WorkflowRunStatus;
use legion_workflows::types_extra::{SignalAddress, SignalDelivery};
use serde_json::{Value, json};
use uuid::Uuid;

use super::AppState;
use super::mcp_milestones::McpCallerContext;
use super::rpc::workflows_feedback::read_feedback_mode;

pub(super) async fn workflow_tool(
    state: &Arc<AppState>,
    caller: &McpCallerContext,
    args: &Value,
) -> Result<Value, String> {
    if !caller.shows_outside_only() {
        return Err("Workflow tool is hidden inside thread scopes.".to_owned());
    }
    let Some(engine) = state.runtime_http.workflow_engine() else {
        return Err("Workflow engine is unavailable.".to_owned());
    };
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Missing required `action` parameter.".to_owned())?;

    match action {
        "list" => {
            let status = match args.get("status").and_then(Value::as_str) {
                Some(raw) => Some(
                    serde_json::from_value::<WorkflowRunStatus>(Value::String(raw.to_owned()))
                        .map_err(|_| format!("Invalid workflow status `{raw}`."))?,
                ),
                None => None,
            };
            let runs = engine.list(status).map_err(|e| e.to_string())?;
            Ok(json!({ "runs": runs }))
        }
        "read" => {
            let run_id = required_field(args, "runId")?;
            let detail = engine.get(run_id).map_err(|e| e.to_string())?;
            Ok(json!(detail))
        }
        "questions" => {
            let questions = engine.questions().map_err(|e| e.to_string())?;
            Ok(json!({ "questions": questions }))
        }
        "hooks" => {
            let hooks = engine.hooks().map_err(|e| e.to_string())?;
            Ok(json!({ "hooks": hooks }))
        }
        "store" => {
            let prefix = args.get("key").and_then(Value::as_str).unwrap_or("");
            let entries = engine.stored(prefix).map_err(|e| e.to_string())?;
            Ok(json!({ "entries": entries }))
        }
        "store_set" => {
            let key = required_field(args, "key")?;
            let value = args
                .get("value")
                .ok_or_else(|| "This action needs `value`.".to_owned())?;
            engine.store_set(key, value).map_err(|e| e.to_string())?;
            let entries = engine.stored(key).map_err(|e| e.to_string())?;
            Ok(json!({ "entries": entries }))
        }
        "store_delete" => {
            let key = required_field(args, "key")?;
            engine.store_delete(key).map_err(|e| e.to_string())?;
            let entries = engine.stored(key).map_err(|e| e.to_string())?;
            Ok(json!({ "entries": entries }))
        }
        "start" => {
            let workflow = required_field(args, "workflow")?;
            let idempotency_key = required_field(args, "idempotencyKey")?;
            let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
            let run_id = engine
                .start(workflow, input, idempotency_key, None, None)
                .await
                .map_err(|e| e.to_string())?;
            let detail = engine.get(&run_id).map_err(|e| e.to_string())?;
            Ok(json!(detail))
        }
        "signal" => dispatch_workflow_signal(&engine, args).await,
        "resume" => {
            let run_id = required_field(args, "runId")?;
            engine.resume(run_id).await.map_err(|e| e.to_string())?;
            let detail = engine.get(run_id).map_err(|e| e.to_string())?;
            Ok(json!(detail))
        }
        "cancel" => {
            let run_id = required_field(args, "runId")?;
            let reason = args
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned);
            engine
                .cancel(run_id, reason)
                .await
                .map_err(|e| e.to_string())?;
            let detail = engine.get(run_id).map_err(|e| e.to_string())?;
            Ok(json!(detail))
        }
        other => Err(format!("Unsupported workflow action `{other}`.")),
    }
}

async fn dispatch_workflow_signal(
    engine: &Arc<legion_workflows::WorkflowEngine>,
    args: &Value,
) -> Result<Value, String> {
    let value = args
        .get("value")
        .cloned()
        .ok_or_else(|| "This action needs `value`.".to_owned())?;
    let addr = match (
        args.get("runId").and_then(Value::as_str),
        args.get("wait").and_then(Value::as_str),
        args.get("workflow").and_then(Value::as_str),
        args.get("channel").and_then(Value::as_str),
    ) {
        (Some(run_id), Some(wait), _, _) => SignalAddress::Run {
            run_id: run_id.to_owned(),
            wait: wait.to_owned(),
        },
        (_, _, Some(workflow), Some(channel)) => SignalAddress::Channel {
            workflow: workflow.to_owned(),
            channel: channel.to_owned(),
        },
        _ => {
            return Err(
                "Address a signal with `runId` and `wait`, or `workflow` and `channel`.".to_owned(),
            );
        }
    };
    let sig_id = args
        .get("signalId")
        .and_then(Value::as_str)
        .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned);
    let delivery = engine
        .signal(addr, value, &sig_id, None)
        .await
        .map_err(|e| e.to_string())?;
    let label = match delivery {
        SignalDelivery::Delivered => "delivered",
        SignalDelivery::Buffered => "buffered",
    };
    Ok(json!({ "delivery": label }))
}

const REGISTERED_FEEDBACK_TOOLS: &[&str] = &[
    "project_list",
    "thread_list",
    "thread_read",
    "thread_launch",
    "thread_send",
    "submit_output",
    "thread_wait",
    "workflow",
    "feedback",
    "workflow_board_read_brief",
    "workflow_board_propose",
];

const REGISTERED_FEEDBACK_CLI_COMMANDS: &[&str] = &[
    "legioncode thread list",
    "legioncode thread read",
    "legioncode thread start",
    "legioncode thread send",
    "legioncode thread interrupt",
    "legioncode thread wait",
    "legioncode workflow",
    "legioncode workflow start",
    "legioncode workflow list",
    "legioncode workflow show",
    "legioncode workflow resume",
    "legioncode workflow cancel",
    "legioncode workflow signal",
    "legioncode workflow questions",
    "legioncode workflow hooks",
    "legioncode workflow store",
    "legioncode feedback",
    "t3 thread list",
    "t3 thread read",
    "t3 thread start",
    "t3 thread send",
    "t3 thread interrupt",
    "t3 thread wait",
    "t3 workflow",
    "t3 feedback",
];

pub(super) fn feedback_tool(
    state: &AppState,
    caller: &McpCallerContext,
    args: &Value,
) -> Result<Value, String> {
    let mode = read_feedback_mode(state);
    if mode == FeedbackMode::Off {
        return Err(
            "Agent feedback is off. Turn it on in Settings > Labs, then try again.".to_owned(),
        );
    }
    let Some(ref store) = state.runtime_http.feedback_store else {
        return Err("Feedback store is unavailable.".to_owned());
    };
    let kind_raw = args.get("kind").cloned().unwrap_or(Value::Null);
    let kind: FeedbackKind = serde_json::from_value(kind_raw)
        .map_err(|_| "Missing or invalid `kind` parameter.".to_owned())?;
    let subject = required_field(args, "subject")?;
    let expected = required_field(args, "expected")?;
    let actual = required_field(args, "actual")?;
    let workaround = args.get("workaround").and_then(Value::as_str).unwrap_or("");
    let user_requested = args
        .get("userRequested")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let turns = usize::try_from(args.get("turns").and_then(Value::as_u64).unwrap_or(1))
        .unwrap_or(1)
        .max(1);
    let input = SubmitFeedbackInput {
        kind,
        subject,
        expected,
        actual,
        workaround,
        user_requested,
        turns,
        source: FeedbackSource::Mcp,
        thread_id: caller.thread_id.as_deref(),
        run_id: None,
        sensitive: &[],
        registered_tools: REGISTERED_FEEDBACK_TOOLS,
        cli_commands: REGISTERED_FEEDBACK_CLI_COMMANDS,
        trace_runs: &[],
        trace_items: &[],
    };
    match store.submit(mode, &input).map_err(|e| e.to_string())? {
        FeedbackSubmitOutcome::Stored(report) => {
            let trace = report.trace.map(|t| {
                json!({
                    "turns": t.turns,
                    "textTurns": t.text_turns,
                    "truncated": t.truncated,
                })
            });
            Ok(json!({
                "id": report.id,
                "threadId": report.thread_id,
                "createdAt": report.created_at,
                "kind": report.kind,
                "subject": report.subject,
                "expected": report.expected,
                "actual": report.actual,
                "workaround": report.workaround,
                "userRequested": report.user_requested,
                "source": report.source,
                "sent": report.sent,
                "trace": trace,
            }))
        }
        FeedbackSubmitOutcome::AwaitingApproval { .. } => Ok(json!({
            "status": "awaiting_approval",
            "note": FEEDBACK_AWAITING_APPROVAL_NOTE,
        })),
    }
}

fn required_field<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("This action needs `{key}`."))
}
