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

use super::projects_fs::{base_dir_from_state, project_to_wire};
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj, with_db, with_db_mut};

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
            "_tag": "NamedProjectFolderError",
            "folder": target_dir.to_string_lossy(),
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
            .map_err(|e| rpc_fail("NamedProjectCreateError", &e.to_string()))?;
        let record = create_project(
            conn,
            &CreateProjectInput::new(&root_str)
                .with_project_id(&project_id)
                .with_title(req.name.trim()),
        )
        .map_err(|e| {
            rpc_fail_obj(json!({
                "_tag": "NamedProjectCreateError",
                "workspaceRoot": root_str,
                "message": e.to_string(),
            }))
        })?;
        Ok(RpcExit::Success {
            value: json!({ "project": project_to_wire(&record) }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateFromThreadPayload {
    thread_id: String,
    name: String,
    directory: Option<String>,
}

pub(crate) async fn create_from_thread(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let req: CreateFromThreadPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let created = create_new(
        state,
        json!({ "name": req.name, "directory": req.directory }),
    )
    .await;
    if let RpcExit::Success { ref value } = created
        && let Some(project_id) = value.pointer("/project/id").and_then(Value::as_str)
    {
        let _ = with_db(state, |conn| {
            let _ = conn.execute(
                "UPDATE orchestration_v2_projection_threads SET project_id = ?1 WHERE thread_id = ?2",
                params![project_id, req.thread_id],
            );
            Ok(())
        });
    }
    created
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
