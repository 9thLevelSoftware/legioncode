//! `/mcp` handlers for `legion_thread_launch` (`#28597`, `#38874`) and `legion_submit_output` (`#38874`).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use legion_contracts::ids::{CommandId, MessageId, RunId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, InitialThreadMessage, OrchestrationActor, SubmitRunOutputCommand,
};
use legion_orchestrator::decider::output_ops::validate_output_request_for_message;
use legion_server_runtime::{LaunchThreadRequest, ThreadLauncher};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::AppState;
#[path = "mcp_launch_args.rs"]
mod args;
pub(super) use args::parse_output_schema_arg;
use args::{
    parse_interaction_mode, parse_or_resolve_model_selection, parse_or_resolve_runtime_mode,
    parse_workflow_run, parse_workspace_strategy,
};

/// Validates and handles `legion_thread_launch` / `t3_thread_launch`.
pub(super) async fn thread_launch(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let project_hint = required_non_empty(args, "projectId")?;
    let title = required_non_empty(args, "title")?.to_owned();
    let generate_title = args.get("generateTitle").and_then(Value::as_bool);
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let attachments = args
        .get("attachments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if generate_title == Some(true)
        && message.as_deref().is_none_or(|m| m.trim().is_empty())
        && attachments.is_empty()
    {
        return Err(
            "Title generation requires a message or attachments. No thread was created.".to_owned(),
        );
    }

    let output_schema = parse_output_schema_arg(args)?;
    if output_schema.is_some() && message.as_deref().is_none_or(|m| m.trim().is_empty()) {
        return Err("outputSchema needs a message that asks for the result.".to_owned());
    }
    validate_output_request_for_message(
        message.as_deref().unwrap_or(""),
        &attachments,
        output_schema.as_ref(),
    )
    .map_err(|e| e.to_string())?;

    let launcher = ThreadLauncher::new(
        Arc::clone(&state.orchestrator),
        state.paths.worktrees_dir.clone(),
    )
    .with_settings_path(Some(state.paths.settings_path.clone()));
    let project = launcher
        .resolve_project(project_hint)
        .map_err(|e| e.to_string())?;

    let model_selection =
        parse_or_resolve_model_selection(args.get("modelSelection"), &project, state)?;
    let runtime_mode = parse_or_resolve_runtime_mode(args.get("runtimeMode"), state)?;
    let interaction_mode = parse_interaction_mode(args.get("interactionMode"))?;
    let workspace_strategy = parse_workspace_strategy(args.get("workspaceStrategy"))?;
    let workflow_run =
        parse_workflow_run(args.get("workflowRun"), &state.descriptor.environment_id)?;

    let stable_launch_key = args
        .get("stableLaunchKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let reuse_existing_thread = args
        .get("reuseExistingThread")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let command_id =
        CommandId::new(format!("mcp-launch-{}", Uuid::new_v4())).map_err(|e| e.to_string())?;
    let initial_message =
        build_initial_message(message, attachments, generate_title, &title, output_schema)?;

    let result = launcher
        .launch_thread(LaunchThreadRequest {
            command_id,
            created_by: OrchestrationActor::Agent,
            creation_source: CreationSource::Mcp,
            thread_id: None,
            stable_launch_key,
            reuse_existing_thread,
            project_id: project.id,
            title,
            title_renamed: None,
            generate_title,
            model_selection,
            runtime_mode,
            interaction_mode,
            workspace_strategy,
            workflow_run,
            link_origin: None,
            delegated_from: None,
            initial_message,
        })
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "threadId": result.thread_id.as_str(),
        "runId": result.run_id,
    }))
}

fn required_non_empty<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing required `{key}` parameter."))
}

fn build_initial_message(
    message: Option<String>,
    attachments: Vec<Value>,
    generate_title: Option<bool>,
    title: &str,
    output_schema: Option<legion_contracts::orchestration::RunOutputSchema>,
) -> Result<Option<InitialThreadMessage>, String> {
    if message.is_none() && attachments.is_empty() {
        return Ok(None);
    }
    Ok(Some(InitialThreadMessage {
        message_id: MessageId::new(format!("mcp-msg-{}", Uuid::new_v4()))
            .map_err(|e| e.to_string())?,
        text: message.unwrap_or_default(),
        attachments,
        context: None,
        title_seed: generate_title.and_then(|g| g.then(|| title.to_owned())),
        scheduled_task_id: None,
        sender_thread_id: None,
        output_schema,
        workspace_strategy: None,
    }))
}

/// Validates and commits `legion_submit_output` (`SubmitRunOutputCommand`).
pub(super) async fn submit_output(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let raw_json = args
        .get("json")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required `json` parameter.".to_owned())?;
    let parsed: Value =
        serde_json::from_str(raw_json).map_err(|_| "`json` is not valid JSON text.".to_owned())?;

    let (thread_id_str, run_id_str) = resolve_output_target(state, args)?;
    let thread_id = ThreadId::new(thread_id_str).map_err(|e| e.to_string())?;
    let run_id = RunId::new(run_id_str.clone()).map_err(|e| e.to_string())?;
    let command_id =
        CommandId::new(format!("mcp-output-{}", Uuid::new_v4())).map_err(|e| e.to_string())?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let now_iso = format!("1970-01-01T00:00:{now_ms:02}Z");
    state
        .orchestrator
        .dispatch_submit_run_output(
            &SubmitRunOutputCommand {
                command_id,
                thread_id,
                run_id,
                value: parsed,
            },
            &now_iso,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({ "runId": run_id_str }))
}

fn resolve_output_target(state: &Arc<AppState>, args: &Value) -> Result<(String, String), String> {
    let explicit_thread = args
        .get("threadId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let explicit_run = args
        .get("runId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let conn = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| "database mutex poisoned".to_owned())?;
    if let (Some(tid), Some(rid)) = (explicit_thread.clone(), explicit_run) {
        return Ok((tid, rid));
    }
    if let Some(tid) = explicit_thread {
        let rid: String = conn
            .query_row(
                "SELECT run_id FROM orchestration_v2_projection_runs
                 WHERE thread_id = ?1
                   AND json_type(payload_json, '$.outputSchema') IS NOT NULL
                   AND (status IN ('queued', 'starting', 'running')
                        OR (status = 'completed' AND json_type(payload_json, '$.output') IS NULL))
                 ORDER BY ordinal DESC LIMIT 1",
                [&tid],
                |r| r.get(0),
            )
            .map_err(|_| {
                "This turn was not asked for a result. Reply in your message instead.".to_owned()
            })?;
        return Ok((tid, rid));
    }

    conn.query_row(
        "SELECT thread_id, run_id FROM orchestration_v2_projection_runs
         WHERE json_type(payload_json, '$.outputSchema') IS NOT NULL
           AND (status IN ('queued', 'starting', 'running')
                OR (status = 'completed' AND json_type(payload_json, '$.output') IS NULL))
         ORDER BY updated_at DESC, ordinal DESC LIMIT 1",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .map_err(|_| "This turn was not asked for a result. Reply in your message instead.".to_owned())
}
