//! Workspace named-project creation (`projects.createNew`, `projects.createFromThread`).

use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use super::orchestration_commands::publish_project_shell_event;
use super::projects_fs::base_dir_from_state;
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj, utc_now_iso, with_db, with_db_mut};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateNewProjectPayload {
    name: String,
    directory: Option<String>,
}

pub(crate) async fn create_new(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: CreateNewProjectPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let folder_name = sanitize_folder_name(&req.name);
    let parent_dir = req
        .directory
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || base_dir_from_state(state).join("projects"),
            PathBuf::from,
        );
    let target_dir = parent_dir.join(&folder_name);
    if let Err(err) = fs::create_dir_all(&target_dir) {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationDispatchCommandError",
            "commandId": "cmd-create-project",
            "commandType": "project.create",
            "message": err.to_string(),
        }));
    }
    let _ = Command::new("git")
        .current_dir(&target_dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("init")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    let root_str = target_dir.to_string_lossy().into_owned();
    let project_id = format!("project-{}", Uuid::new_v4());
    with_db_mut(state, |conn| {
        ensure_project_store_schema(conn)
            .map_err(|e| rpc_fail("OrchestrationDispatchCommandError", &e.to_string()))?;
        let record = create_project(
            &*conn,
            &CreateProjectInput::new(&root_str)
                .with_project_id(&project_id)
                .with_title(req.name.trim()),
        )
        .map_err(|e| {
            rpc_fail_obj(json!({
                "_tag": "OrchestrationDispatchCommandError",
                "commandId": "cmd-create-project",
                "commandType": "project.create",
                "message": e.to_string(),
            }))
        })?;
        let now_iso = utc_now_iso();
        publish_project_shell_event(
            state,
            conn,
            &record.id,
            false,
            "cmd-create-project",
            &now_iso,
        )
        .map_err(|e| {
            rpc_fail_obj(json!({
                "_tag": "OrchestrationDispatchCommandError",
                "commandId": "cmd-create-project",
                "commandType": "project.create",
                "message": e.to_string(),
            }))
        })?;
        Ok(RpcExit::Success {
            value: json!({
                "projectId": record.id,
                "workspaceRoot": record.workspace_root,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateFromThreadPayload {
    thread_id: String,
    #[serde(default)]
    name: Option<String>,
    directory: Option<String>,
}

pub(crate) async fn create_from_thread(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: CreateFromThreadPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                with_db(state, |conn| {
                    Ok(conn
                        .query_row(
                            "SELECT title FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
                            params![req.thread_id],
                            |r| r.get::<_, String>(0),
                        )
                        .ok())
                })
                .ok()
                .flatten()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Project".to_owned())
            },
            str::to_owned,
        );
    let created = create_new(state, json!({ "name": name, "directory": req.directory })).await;
    match created {
        RpcExit::Success { value } => {
            if let Some(project_id) = value.get("projectId").and_then(Value::as_str) {
                let _ = with_db(state, |conn| {
                    let _ = conn.execute(
                        "UPDATE orchestration_v2_projection_threads SET project_id = ?1 WHERE thread_id = ?2",
                        params![project_id, req.thread_id],
                    );
                    Ok(())
                });
                return RpcExit::Success {
                    value: json!({ "projectId": project_id }),
                };
            }
            RpcExit::Success { value }
        }
        other @ RpcExit::Failure { .. } => other,
    }
}

fn sanitize_folder_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['.', '-']);
    if trimmed.is_empty() {
        format!("project-{}", &Uuid::new_v4().simple().to_string()[..6])
    } else {
        trimmed.to_owned()
    }
}
