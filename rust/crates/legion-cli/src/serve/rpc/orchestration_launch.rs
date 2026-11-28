//! Thread launch (`launchThread`), force-stop (`forceStopThread`), and workflow-stop (`stopWorkflow`).

use std::sync::Arc;

use legion_contracts::ids::{CommandId, MessageId, ProjectId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, InitialThreadMessage, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, ThreadStopCommand, WorkspaceStrategy,
};
use legion_contracts::policy::RuntimeMode;
use legion_protocol::rpc_codec::RpcExit;
use legion_server_runtime::{LaunchThreadRequest, ThreadLauncher};
use rusqlite::{OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, parse_payload, rpc_fail_obj, with_db};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchThreadPayload {
    command_id: CommandId,
    #[serde(default)]
    creation_source: Option<CreationSource>,
    thread_id: Option<ThreadId>,
    #[serde(default)]
    stable_launch_key: Option<String>,
    reuse_existing_thread: Option<bool>,
    project_id: ProjectId,
    title: String,
    generate_title: Option<bool>,
    model_selection: ModelSelection,
    runtime_mode: RuntimeMode,
    interaction_mode: ProviderInteractionMode,
    workspace_strategy: WorkspaceStrategy,
    initial_message: Option<LaunchInitialMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchInitialMessage {
    message_id: Option<MessageId>,
    text: String,
    context: Option<Value>,
    #[serde(default)]
    attachments: Vec<Value>,
}

pub(crate) async fn launch_thread(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: LaunchThreadPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let err_cmd_id = req.command_id.as_str().to_owned();
    let err_proj_id = req.project_id.as_str().to_owned();
    let launch_err = |msg: &str| {
        rpc_fail_obj(json!({
            "_tag": "OrchestrationV2ThreadLaunchError",
            "commandId": err_cmd_id,
            "projectId": err_proj_id,
            "message": msg,
        }))
    };

    let initial_message = req.initial_message.map(|m| InitialThreadMessage {
        message_id: m.message_id.unwrap_or_else(|| {
            MessageId::new(format!("msg-{}", Uuid::new_v4()))
                .unwrap_or_else(|_| MessageId::new("msg-fallback").expect("valid constant id"))
        }),
        text: m.text,
        attachments: m.attachments,
        context: m.context,
        title_seed: None,
        scheduled_task_id: None,
        sender_thread_id: None,
        output_schema: None,
        workspace_strategy: None,
    });

    let launcher = ThreadLauncher::new(
        Arc::clone(&state.orchestrator),
        state.paths.worktrees_dir.clone(),
    )
    .with_settings_path(Some(state.paths.settings_path.clone()));

    let res = match launcher
        .launch_thread(LaunchThreadRequest {
            command_id: req.command_id,
            created_by: OrchestrationActor::User,
            creation_source: req.creation_source.unwrap_or(CreationSource::Web),
            thread_id: req.thread_id,
            stable_launch_key: req.stable_launch_key,
            reuse_existing_thread: req.reuse_existing_thread == Some(true),
            project_id: req.project_id.as_str().to_owned(),
            title: req.title,
            title_renamed: None,
            generate_title: req.generate_title,
            model_selection: req.model_selection,
            runtime_mode: req.runtime_mode,
            interaction_mode: req.interaction_mode,
            workspace_strategy: req.workspace_strategy,
            workflow_run: None,
            link_origin: None,
            delegated_from: None,
            initial_message,
        })
        .await
    {
        Ok(res) => res,
        Err(err) => return launch_err(&err.to_string()),
    };

    RpcExit::Success {
        value: json!({
            "threadId": res.thread_id.as_str(),
            "projection": res.projection,
            "resumed": res.resumed,
            "sequence": res.sequence,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForceStopPayload {
    thread_id: ThreadId,
    run_id: Option<String>,
}

pub(crate) async fn force_stop_thread(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: ForceStopPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let latest_run_id: Option<String> = match with_db(state, |conn| {
        Ok(conn
            .query_row(
                "SELECT run_id FROM orchestration_v2_projection_runs
                 WHERE thread_id = ?1 ORDER BY ordinal DESC LIMIT 1",
                params![req.thread_id.as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap_or(None))
    }) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(expected) = req.run_id.as_deref()
        && latest_run_id.as_deref() != Some(expected)
    {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationV2ForceStopThreadError",
            "threadId": req.thread_id.as_str(),
            "reason": "superseded",
            "message": "A new turn started, so nothing was force stopped.",
        }));
    }
    if let Ok(cmd_id) = CommandId::new(format!("force-stop-{}", Uuid::new_v4())) {
        let stop_cmd = OrchestrationCommand::ThreadStop(ThreadStopCommand {
            command_id: cmd_id,
            thread_id: req.thread_id.clone(),
            reason: None,
            stop_delegated_tasks: Some(true),
        });
        let _ = state.orchestrator.dispatch_now(&stop_cmd).await;
    }
    RpcExit::Success {
        value: json!({
            "outcome": "released",
            "sharedWithThreadIds": [],
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StopWorkflowPayload {
    thread_id: ThreadId,
    subagent_id: String,
}

pub(crate) async fn stop_workflow(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: StopWorkflowPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let child_thread_id: Option<String> = match with_db(state, |conn| {
        Ok(conn
            .query_row(
                "SELECT json_extract(payload_json, '$.childThreadId')
                 FROM orchestration_v2_projection_subagents
                 WHERE thread_id = ?1 AND subagent_id = ?2",
                params![req.thread_id.as_str(), req.subagent_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .unwrap_or(None)
            .flatten())
    }) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(child_tid) = child_thread_id.and_then(|s| ThreadId::new(s).ok()) else {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationV2StopWorkflowError",
            "threadId": req.thread_id.as_str(),
            "subagentId": req.subagent_id,
            "reason": "not-running",
            "message": "This workflow is no longer running.",
        }));
    };
    if let Ok(cmd_id) = CommandId::new(format!("stop-wf-{}", Uuid::new_v4())) {
        let stop_cmd = OrchestrationCommand::ThreadStop(ThreadStopCommand {
            command_id: cmd_id,
            thread_id: child_tid,
            reason: None,
            stop_delegated_tasks: Some(true),
        });
        let _ = state.orchestrator.dispatch_now(&stop_cmd).await;
    }
    RpcExit::Success { value: Value::Null }
}
