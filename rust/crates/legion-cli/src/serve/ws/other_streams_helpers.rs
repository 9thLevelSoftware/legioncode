//! Helper routines split out of `ws::other_streams` to keep modules concise.

use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::{Value, json};

use super::StreamContext;

pub(super) async fn stream_git_stacked_action(ctx: &StreamContext, payload: Value) -> RpcExit {
    let action_id = payload
        .get("actionId")
        .and_then(Value::as_str)
        .unwrap_or("action-1")
        .to_owned();
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or(".")
        .to_owned();
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("commit")
        .to_owned();
    let _ = ctx
        .send_chunk(json!({
            "actionId": action_id,
            "cwd": cwd,
            "action": action,
            "kind": "action_started",
            "phases": [],
        }))
        .await;
    let res = crate::serve::rpc::git_actions::git_run_stacked_action(payload).await;
    match res {
        RpcExit::Success { value } => {
            let _ = ctx
                .send_chunk(json!({
                    "actionId": action_id,
                    "cwd": cwd,
                    "action": action,
                    "kind": "action_finished",
                    "result": value,
                }))
                .await;
        }
        RpcExit::Failure { cause } => {
            let msg = cause
                .first()
                .and_then(|c| match c {
                    RpcCauseReason::Fail { error } => error
                        .get("detail")
                        .or_else(|| error.get("message"))
                        .and_then(Value::as_str),
                    RpcCauseReason::Die { defect } => defect.as_str(),
                    RpcCauseReason::Interrupt { .. } => Some("Interrupted"),
                })
                .unwrap_or("Git action failed");
            let _ = ctx
                .send_chunk(json!({
                    "actionId": action_id,
                    "cwd": cwd,
                    "action": action,
                    "kind": "action_failed",
                    "phase": Value::Null,
                    "message": msg,
                }))
                .await;
        }
    }
    RpcExit::Success { value: Value::Null }
}

pub(super) fn load_scheduled_tasks_json(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<Vec<Value>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'scheduled_tasks')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT task_id, title, prompt, enabled, schedule_json, project_id, thread_id,
                thread_mode, workspace_strategy_json, model_selection_json,
                runtime_mode, interaction_mode, created_by, creation_source,
                created_at, updated_at, next_run_at, last_run_at,
                last_run_status, last_run_error, run_count
         FROM scheduled_tasks
         ORDER BY created_at ASC, task_id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let schedule_raw: String = row.get(4)?;
        let ws_raw: String = row.get(8)?;
        let model_raw: String = row.get(9)?;
        let enabled_int: i64 = row.get(3)?;
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "title": row.get::<_, String>(1)?,
            "prompt": row.get::<_, String>(2)?,
            "enabled": enabled_int != 0,
            "schedule": serde_json::from_str::<Value>(&schedule_raw).unwrap_or_else(|_| json!({})),
            "projectId": row.get::<_, String>(5)?,
            "threadId": row.get::<_, Option<String>>(6)?,
            "threadMode": row.get::<_, String>(7)?,
            "workspaceStrategy": serde_json::from_str::<Value>(&ws_raw).unwrap_or_else(|_| json!({ "type": "root", "branch": null })),
            "modelSelection": serde_json::from_str::<Value>(&model_raw).unwrap_or_else(|_| json!({})),
            "runtimeMode": row.get::<_, String>(10)?,
            "interactionMode": row.get::<_, String>(11)?,
            "createdBy": row.get::<_, String>(12)?,
            "creationSource": row.get::<_, String>(13)?,
            "createdAt": row.get::<_, String>(14)?,
            "updatedAt": row.get::<_, String>(15)?,
            "nextRunAt": row.get::<_, Option<String>>(16)?,
            "lastRunAt": row.get::<_, Option<String>>(17)?,
            "lastRunStatus": row.get::<_, String>(18)?,
            "lastRunError": row.get::<_, Option<String>>(19)?,
            "runCount": row.get::<_, i64>(20)?,
        }))
    })?;
    rows.collect()
}
