//! Scheduled tasks CRUD/run handlers and agent session scan/import routing.

use std::sync::Arc;

use legion_contracts::ids::{CommandId, MessageId, ProjectId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, InitialThreadMessage, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, ThreadCreateCommand,
};
use legion_contracts::policy::RuntimeMode;
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::tasks_webhooks_mcp::{compute_next_run_at, read_task_row, validate_schedule};
use super::{
    AppState, agent_sessions, parse_payload, rpc_fail, rpc_fail_obj, tasks_webhooks_mcp,
    utc_now_iso, with_db,
};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "scheduledTasks.upsert" => upsert_task(state, payload),
        "scheduledTasks.setEnabled" => set_enabled(state, payload),
        "scheduledTasks.delete" => delete_task(state, payload),
        "scheduledTasks.runNow" => run_now(state, payload).await,
        "scheduledTasks.rotateWebhookToken"
        | "scheduledTasks.listWebhookDeliveries"
        | "scheduledTasks.getWebhookDelivery" => tasks_webhooks_mcp::dispatch(state, tag, payload),
        "agentSessions.scan" => agent_sessions::scan(state, payload),
        "agentSessions.import" => agent_sessions::import(state, payload).await,
        _ if tag.starts_with("mcpApps.") => tasks_webhooks_mcp::dispatch(state, tag, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

pub(crate) fn ensure_tasks_schema(conn: &Connection) -> Result<(), RpcExit> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS scheduled_tasks (
           task_id TEXT PRIMARY KEY,
           title TEXT NOT NULL,
           prompt TEXT NOT NULL,
           enabled INTEGER NOT NULL,
           schedule_json TEXT NOT NULL,
           project_id TEXT NOT NULL,
           thread_id TEXT,
           workspace_strategy_json TEXT NOT NULL,
           model_selection_json TEXT NOT NULL,
           runtime_mode TEXT NOT NULL,
           interaction_mode TEXT NOT NULL,
           created_by TEXT NOT NULL,
           creation_source TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           next_run_at TEXT,
           last_run_at TEXT,
           last_run_status TEXT NOT NULL,
           last_run_error TEXT,
           run_count INTEGER NOT NULL,
           webhook_token TEXT,
           webhook_secret TEXT,
           thread_mode TEXT NOT NULL DEFAULT 'new_thread'
         );
         CREATE TABLE IF NOT EXISTS scheduled_task_webhook_deliveries (
           delivery_id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL,
           received_at TEXT NOT NULL,
           method TEXT NOT NULL,
           query TEXT NOT NULL,
           headers_json TEXT NOT NULL,
           body TEXT NOT NULL,
           body_bytes INTEGER NOT NULL,
           body_truncated INTEGER NOT NULL,
           outcome TEXT NOT NULL,
           signature_verified INTEGER NOT NULL,
           missing_fields_json TEXT NOT NULL,
           rendered_prompt TEXT,
           error TEXT
         );",
    )
    .map_err(|e| task_err(&e.to_string()))
}

pub(crate) fn task_err(detail: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "ScheduledTaskError",
        "message": detail,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertTaskPayload {
    #[serde(alias = "id")]
    task_id: Option<String>,
    title: String,
    prompt: String,
    enabled: Option<bool>,
    schedule: Value,
    project_id: String,
    thread_id: Option<String>,
    workspace_strategy: Option<Value>,
    model_selection: Value,
    runtime_mode: Option<String>,
    interaction_mode: Option<String>,
    thread_mode: Option<String>,
}

fn upsert_task(state: &AppState, payload: Value) -> RpcExit {
    let req: UpsertTaskPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(err) = validate_schedule(&req.schedule) {
        return task_err(&err);
    }
    let now = utc_now_iso();
    let task_id = req
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("sched-{}", Uuid::new_v4()), str::to_owned);
    let enabled = req.enabled.unwrap_or(true);
    let schedule_json = serde_json::to_string(&req.schedule).unwrap_or_else(|_| "{}".to_owned());
    let ws_json = serde_json::to_string(
        &req.workspace_strategy
            .unwrap_or_else(|| json!({ "type": "root", "branch": null })),
    )
    .unwrap_or_else(|_| "{}".to_owned());
    let ms_json = serde_json::to_string(&req.model_selection).unwrap_or_else(|_| "{}".to_owned());
    let runtime_mode = req
        .runtime_mode
        .unwrap_or_else(|| "approval-required".to_owned());
    let interaction_mode = req.interaction_mode.unwrap_or_else(|| "default".to_owned());
    let thread_mode = req.thread_mode.unwrap_or_else(|| {
        if req.thread_id.is_some() {
            "continue".to_owned()
        } else {
            "new_thread".to_owned()
        }
    });
    let next_run_at = if enabled {
        compute_next_run_at(&req.schedule)
    } else {
        None
    };

    with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        conn.execute(
            "INSERT INTO scheduled_tasks (
               task_id, title, prompt, enabled, schedule_json, project_id, thread_id,
               workspace_strategy_json, model_selection_json, runtime_mode, interaction_mode,
               created_by, creation_source, created_at, updated_at, next_run_at,
               last_run_at, last_run_status, last_run_error, run_count, thread_mode
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'user', 'web', ?12, ?12, ?13, NULL, 'never', NULL, 0, ?14)
             ON CONFLICT(task_id) DO UPDATE SET
               title = excluded.title,
               prompt = excluded.prompt,
               enabled = excluded.enabled,
               schedule_json = excluded.schedule_json,
               project_id = excluded.project_id,
               thread_id = excluded.thread_id,
               workspace_strategy_json = excluded.workspace_strategy_json,
               model_selection_json = excluded.model_selection_json,
               runtime_mode = excluded.runtime_mode,
               interaction_mode = excluded.interaction_mode,
               updated_at = excluded.updated_at,
               next_run_at = excluded.next_run_at,
               thread_mode = excluded.thread_mode",
            params![
                task_id,
                req.title,
                req.prompt,
                i64::from(enabled),
                schedule_json,
                req.project_id,
                req.thread_id,
                ws_json,
                ms_json,
                runtime_mode,
                interaction_mode,
                now,
                next_run_at,
                thread_mode,
            ],
        )
        .map_err(|e| task_err(&e.to_string()))?;
        let task = read_task_row(conn, &task_id)?
            .ok_or_else(|| task_err("Saved scheduled task could not be reloaded."))?;
        Ok(RpcExit::Success {
            value: json!({ "task": task }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEnabledPayload {
    #[serde(alias = "id")]
    task_id: String,
    enabled: bool,
}

fn set_enabled(state: &AppState, payload: Value) -> RpcExit {
    let req: SetEnabledPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let now = utc_now_iso();
    with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        let changed = conn
            .execute(
                "UPDATE scheduled_tasks SET enabled = ?1, updated_at = ?2 WHERE task_id = ?3",
                params![i64::from(req.enabled), now, req.task_id],
            )
            .map_err(|e| task_err(&e.to_string()))?;
        if changed == 0 {
            return Err(task_err("Scheduled task not found."));
        }
        let task = read_task_row(conn, &req.task_id)?
            .ok_or_else(|| task_err("Scheduled task not found."))?;
        Ok(RpcExit::Success {
            value: json!({ "task": task }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskIdPayload {
    #[serde(alias = "id")]
    pub(crate) task_id: String,
}

fn delete_task(state: &AppState, payload: Value) -> RpcExit {
    let req: TaskIdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        let _changed = conn
            .execute(
                "DELETE FROM scheduled_tasks WHERE task_id = ?1",
                params![req.task_id],
            )
            .map_err(|e| task_err(&e.to_string()))?;
        Ok(RpcExit::Success {
            value: json!({ "id": req.task_id }),
        })
    })
    .unwrap_or_else(|e| e)
}

async fn run_now(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: TaskIdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let task = match with_db(state, |conn| {
        ensure_tasks_schema(conn)?;
        read_task_row(conn, &req.task_id)?.ok_or_else(|| task_err("Scheduled task not found."))
    }) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let project_id_str = task.get("projectId").and_then(Value::as_str).unwrap_or("");
    let title = task
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Scheduled Task")
        .to_owned();
    let prompt = task
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let thread_id_str = format!("thread-{}", Uuid::new_v4());

    if let (Ok(cmd_id), Ok(tid), Ok(pid), Ok(mid), Ok(ms)) = (
        CommandId::new(format!("sched-run-{}", Uuid::new_v4())),
        ThreadId::new(thread_id_str.clone()),
        ProjectId::new(project_id_str),
        MessageId::new(format!("msg-{}", Uuid::new_v4())),
        serde_json::from_value::<ModelSelection>(task["modelSelection"].clone()),
    ) {
        let cmd = OrchestrationCommand::ThreadCreate(ThreadCreateCommand {
            created_by: OrchestrationActor::System,
            creation_source: CreationSource::Server,
            command_id: cmd_id,
            thread_id: tid,
            project_id: pid,
            title,
            generate_title: Some(false),
            model_selection: ms,
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Default,
            branch: None,
            worktree_path: None,
            link_origin: None,
            delegated_from: None,
            imported_native_thread: None,
            initial_message: Some(InitialThreadMessage {
                message_id: mid,
                text: prompt,
                attachments: Vec::new(),
                context: None,
                title_seed: None,
                scheduled_task_id: None,
                sender_thread_id: None,
                output_schema: None,
                workspace_strategy: None,
            }),
        });
        let _ = state.orchestrator.dispatch_now(&cmd).await;
    }

    let now = utc_now_iso();
    with_db(state, |conn| {
        let _ = conn.execute(
            "UPDATE scheduled_tasks
             SET last_run_at = ?1, last_run_status = 'running', run_count = run_count + 1, updated_at = ?1
             WHERE task_id = ?2",
            params![now, req.task_id],
        );
        let updated = read_task_row(conn, &req.task_id)?
            .unwrap_or_else(|| json!({ "id": req.task_id }));
        Ok(RpcExit::Success {
            value: json!({
                "task": updated,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}
