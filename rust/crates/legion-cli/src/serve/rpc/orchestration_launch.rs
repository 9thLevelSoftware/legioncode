//! Thread launch (`launchThread`), force-stop (`forceStopThread`), and workflow-stop (`stopWorkflow`).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use legion_contracts::ids::{CommandId, MessageId, ProjectId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, InitialThreadMessage, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, ThreadCreateCommand, ThreadStopCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::projections::query_wire_thread_detail;
use legion_persistence::project_store::find_project_by_id_or_workspace;
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use super::{AppState, parse_payload, rpc_fail_obj, with_db};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchThreadPayload {
    command_id: CommandId,
    #[serde(default)]
    creation_source: Option<CreationSource>,
    thread_id: Option<ThreadId>,
    reuse_existing_thread: Option<bool>,
    project_id: ProjectId,
    title: String,
    generate_title: Option<bool>,
    model_selection: ModelSelection,
    runtime_mode: RuntimeMode,
    interaction_mode: ProviderInteractionMode,
    workspace_strategy: LaunchWorkspaceStrategy,
    initial_message: Option<LaunchInitialMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LaunchWorkspaceStrategy {
    Root {
        branch: Option<String>,
    },
    ExistingWorktree {
        #[serde(rename = "worktreePath")]
        worktree_path: String,
        branch: Option<String>,
    },
    Worktree {
        #[serde(rename = "baseRef")]
        base_ref: String,
        branch: Option<String>,
    },
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

    let workspace_root = match with_db(state, |conn| {
        Ok(
            find_project_by_id_or_workspace(conn, req.project_id.as_str())
                .ok()
                .flatten()
                .map(|p| p.workspace_root),
        )
    }) {
        Ok(Some(root)) => root,
        Ok(None) => return launch_err("Project was not found."),
        Err(e) => return e,
    };

    let thread_id = req.thread_id.clone().unwrap_or_else(|| {
        ThreadId::new(format!("thread-{}", Uuid::new_v4()))
            .unwrap_or_else(|_| ThreadId::new("thread-fallback").expect("valid constant id"))
    });

    if req.reuse_existing_thread == Some(true)
        && let Ok(Some(existing)) = with_db(state, |conn| {
            Ok(query_wire_thread_detail(conn, thread_id.as_str()).ok())
        })
    {
        return RpcExit::Success {
            value: json!({
                "threadId": thread_id.as_str(),
                "projection": existing.projection,
                "resumed": true,
                "sequence": existing.snapshot_sequence,
            }),
        };
    }

    let (branch, worktree_path) = match prepare_workspace(
        state,
        &req.project_id,
        &workspace_root,
        &req.workspace_strategy,
    )
    .await
    {
        Ok(pair) => pair,
        Err(msg) => return launch_err(&msg),
    };

    let cmd = build_launch_thread_command(req, thread_id.clone(), branch, worktree_path);
    let committed = match state.orchestrator.dispatch_now(&cmd).await {
        Ok(c) => c,
        Err(err) => return launch_err(&err.to_string()),
    };

    let detail = with_db(state, |conn| {
        query_wire_thread_detail(conn, thread_id.as_str())
            .map_err(|err| launch_err(&err.to_string()))
    });
    match detail {
        Ok(d) => RpcExit::Success {
            value: json!({
                "threadId": thread_id.as_str(),
                "projection": d.projection,
                "resumed": false,
                "sequence": committed.outcome.receipt.result_sequence,
            }),
        },
        Err(e) => e,
    }
}

fn build_launch_thread_command(
    req: LaunchThreadPayload,
    thread_id: ThreadId,
    branch: Option<String>,
    worktree_path: Option<String>,
) -> OrchestrationCommand {
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

    OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
        created_by: OrchestrationActor::User,
        creation_source: req.creation_source.unwrap_or(CreationSource::Web),
        command_id: req.command_id,
        thread_id,
        project_id: req.project_id,
        title: req.title,
        generate_title: req.generate_title,
        model_selection: req.model_selection,
        runtime_mode: req.runtime_mode,
        interaction_mode: req.interaction_mode,
        branch,
        worktree_path,
        link_origin: None,
        delegated_from: None,
        imported_native_thread: None,
        initial_message,
    })
}

async fn prepare_workspace(
    state: &AppState,
    project_id: &ProjectId,
    workspace_root: &str,
    strategy: &LaunchWorkspaceStrategy,
) -> Result<(Option<String>, Option<String>), String> {
    match strategy {
        LaunchWorkspaceStrategy::Root { branch } => Ok((branch.clone(), None)),
        LaunchWorkspaceStrategy::ExistingWorktree {
            worktree_path,
            branch,
        } => Ok((branch.clone(), Some(worktree_path.clone()))),
        LaunchWorkspaceStrategy::Worktree { base_ref, branch } => {
            let short = &Uuid::new_v4().simple().to_string()[..8];
            let branch_name = branch.clone().unwrap_or_else(|| format!("legion/{short}"));
            let wt_dir = state
                .paths
                .worktrees_dir
                .join(project_id.as_str().replace(':', "_"))
                .join(short);
            if let Some(parent) = wt_dir.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let wt_str = wt_dir.to_string_lossy().into_owned();
            let out = Command::new("git")
                .current_dir(Path::new(workspace_root))
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(["worktree", "add", "-b", &branch_name, &wt_str, base_ref])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| format!("failed to invoke git worktree add: {e}"))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                return Err(if stderr.is_empty() {
                    "git worktree add failed".to_owned()
                } else {
                    stderr
                });
            }
            Ok((Some(branch_name), Some(wt_str)))
        }
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
