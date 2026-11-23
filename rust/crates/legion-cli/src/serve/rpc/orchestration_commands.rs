//! `orchestration.dispatchCommand` / `orchestration.v2.dispatchCommand` handler
//! supporting both [`OrchestrationCommand`] and [`ProjectWireCommand`].

use std::sync::Arc;

use legion_contracts::orchestration::{
    ModelSelection, PatchValue, ProjectUpdateWireCommand, ProjectWireCommand, WireClientCommand,
    decode_wire_client_command,
};
use legion_persistence::project_store::{
    CreateProjectInput, ProjectStoreError, archive_project, create_project,
    ensure_project_store_schema, rename_project, soft_delete_project, unarchive_project,
};
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, rpc_fail_obj, utc_now_iso, with_db_mut};

pub(crate) async fn dispatch_command(state: &Arc<AppState>, payload: Value) -> RpcExit {
    let command_id = payload
        .get("commandId")
        .and_then(Value::as_str)
        .unwrap_or("cmd:unknown")
        .to_owned();
    let command_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();

    match decode_wire_client_command(payload) {
        Ok(WireClientCommand::Orchestration(cmd)) => {
            match state.orchestrator.dispatch_now(&cmd).await {
                Ok(committed) => RpcExit::Success {
                    value: json!({ "sequence": committed.outcome.receipt.result_sequence }),
                },
                Err(err) => rpc_fail_obj(json!({
                    "_tag": "OrchestrationV2DispatchCommandError",
                    "commandId": command_id,
                    "commandType": command_type,
                    "message": err.to_string(),
                })),
            }
        }
        Ok(WireClientCommand::Project(proj_cmd)) => {
            dispatch_project_command(state, &command_id, *proj_cmd)
        }
        Err(err) => rpc_fail_obj(json!({
            "_tag": "OrchestrationV2DispatchCommandError",
            "commandId": command_id,
            "commandType": command_type,
            "message": err.to_string(),
        })),
    }
}

fn dispatch_project_command(
    state: &AppState,
    command_id: &str,
    cmd: ProjectWireCommand,
) -> RpcExit {
    let now_iso = utc_now_iso();
    with_db_mut(state, |conn| {
        ensure_project_store_schema(conn).map_err(|e| {
            rpc_fail_obj(json!({
                "_tag": "ProjectMutationError",
                "commandId": command_id,
                "message": e.to_string(),
            }))
        })?;
        let target_id = project_cmd_target(&cmd);
        let deleted = matches!(cmd, ProjectWireCommand::Delete { .. });
        let res = match cmd {
            ProjectWireCommand::Create(c) => {
                if c.create_workspace_root_if_missing == Some(true) {
                    let _ = std::fs::create_dir_all(&c.workspace_root);
                }
                let mut input = CreateProjectInput::new(&c.workspace_root).with_title(&c.title);
                if let Some(pid) = c.project_id.as_ref() {
                    input = input.with_project_id(pid.as_str());
                } else {
                    input = input.with_project_id(&target_id);
                }
                create_project(&*conn, &input).map(|created| {
                    apply_model_patch(conn, &created.id, &c.default_model_selection, &now_iso);
                    if let Some(scripts) = &c.scripts {
                        apply_scripts_json(conn, &created.id, scripts, &now_iso);
                    }
                })
            }
            ProjectWireCommand::Update(u) => apply_project_update_cmd(conn, &u, &now_iso),
            ProjectWireCommand::Archive { project_id, .. } => {
                archive_project(&*conn, project_id.as_str()).map(|_| ())
            }
            ProjectWireCommand::Unarchive {
                project_id, title, ..
            } => unarchive_project(&*conn, project_id.as_str(), title.as_deref()).map(|_| ()),
            ProjectWireCommand::Delete {
                project_id, force, ..
            } => {
                soft_delete_project(&*conn, project_id.as_str(), force.unwrap_or(true)).map(|_| ())
            }
        };
        if let Err(err) = res {
            return Err(rpc_fail_obj(json!({
                "_tag": "ProjectMutationError",
                "commandId": command_id,
                "projectId": target_id,
                "message": err.to_string(),
            })));
        }
        publish_project_shell_event(state, conn, &target_id, deleted, command_id, &now_iso)
            .map_err(|err| {
                rpc_fail_obj(json!({
                    "_tag": "ProjectMutationError",
                    "commandId": command_id,
                    "projectId": target_id,
                    "message": err.to_string(),
                }))
            })
            .map(|seq| RpcExit::Success {
                value: json!({ "sequence": seq }),
            })
    })
    .unwrap_or_else(|e| e)
}

/// Appends a synthetic project shell event to `orchestration_events` and wakes live `subscribeShell` streams.
pub(crate) fn publish_project_shell_event(
    state: &AppState,
    conn: &Connection,
    project_id: &str,
    deleted: bool,
    command_id: &str,
    now_iso: &str,
) -> Result<i64, rusqlite::Error> {
    let seq = append_project_shell_event(conn, project_id, deleted, command_id, now_iso)?;
    state
        .orchestrator
        .sink()
        .pubsub()
        .notify_external_sequence(seq);
    Ok(seq)
}

fn append_project_shell_event(
    conn: &Connection,
    project_id: &str,
    deleted: bool,
    command_id: &str,
    now_iso: &str,
) -> Result<i64, rusqlite::Error> {
    let max_ver: i64 = conn.query_row(
        "SELECT COALESCE(MAX(stream_version), 0)
         FROM orchestration_events
         WHERE aggregate_kind = 'project' AND stream_id = ?1",
        [project_id],
        |row| row.get(0),
    )?;
    let event_type = if deleted {
        "project.deleted"
    } else {
        "project.updated"
    };
    let event_id = Uuid::new_v4().to_string();
    conn.query_row(
        "INSERT INTO orchestration_events (
           event_id, aggregate_kind, stream_id, stream_version, event_type,
           occurred_at, command_id, causation_event_id, correlation_id,
           actor_kind, payload_json, metadata_json, application_event_version
         )
         VALUES (?1, 'project', ?2, ?3, ?4, ?5, ?6, NULL, NULL, 'client', '{}', '{}', 1)
         RETURNING sequence",
        params![
            event_id,
            project_id,
            max_ver + 1,
            event_type,
            now_iso,
            command_id
        ],
        |row| row.get(0),
    )
}

fn project_cmd_target(cmd: &ProjectWireCommand) -> String {
    match cmd {
        ProjectWireCommand::Create(c) => c.project_id.as_ref().map_or_else(
            || format!("project-{}", Uuid::new_v4()),
            |id| id.as_str().to_owned(),
        ),
        ProjectWireCommand::Update(u) => u.project_id.as_str().to_owned(),
        ProjectWireCommand::Archive { project_id, .. }
        | ProjectWireCommand::Unarchive { project_id, .. }
        | ProjectWireCommand::Delete { project_id, .. } => project_id.as_str().to_owned(),
    }
}

fn apply_project_update_cmd(
    conn: &Connection,
    u: &ProjectUpdateWireCommand,
    now_iso: &str,
) -> Result<(), ProjectStoreError> {
    let project_id = u.project_id.as_str();
    if let Some(new_title) = u.title.as_deref() {
        rename_project(conn, project_id, new_title)?;
    }
    match u.archived {
        Some(true) => {
            archive_project(conn, project_id)?;
        }
        Some(false) => {
            unarchive_project(conn, project_id, None)?;
        }
        None => {}
    }
    apply_model_patch(conn, project_id, &u.default_model_selection, now_iso);
    if let Some(list) = u.scripts.as_deref() {
        apply_scripts_json(conn, project_id, list, now_iso);
    }
    match u.pinned {
        Some(true) => {
            let _ = conn.execute(
                "UPDATE projection_projects SET pinned_at = ?1, updated_at = ?1 WHERE project_id = ?2",
                params![now_iso, project_id],
            );
        }
        Some(false) => {
            let _ = conn.execute(
                "UPDATE projection_projects SET pinned_at = NULL, updated_at = ?1 WHERE project_id = ?2",
                params![now_iso, project_id],
            );
        }
        None => {}
    }
    Ok(())
}

fn apply_model_patch(
    conn: &Connection,
    project_id: &str,
    model_patch: &PatchValue<ModelSelection>,
    now_iso: &str,
) {
    match model_patch {
        PatchValue::Absent => {}
        PatchValue::Null => {
            let _ = conn.execute(
                "UPDATE projection_projects SET default_model_selection_json = NULL, updated_at = ?1 WHERE project_id = ?2",
                params![now_iso, project_id],
            );
        }
        PatchValue::Set(ms) => {
            if let Ok(encoded) = serde_json::to_string(ms) {
                let _ = conn.execute(
                    "UPDATE projection_projects SET default_model_selection_json = ?1, updated_at = ?2 WHERE project_id = ?3",
                    params![encoded, now_iso, project_id],
                );
            }
        }
    }
}

fn apply_scripts_json(conn: &Connection, project_id: &str, scripts: &[Value], now_iso: &str) {
    if let Ok(encoded) = serde_json::to_string(scripts) {
        let _ = conn.execute(
            "UPDATE projection_projects SET scripts_json = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![encoded, now_iso, project_id],
        );
    }
}
