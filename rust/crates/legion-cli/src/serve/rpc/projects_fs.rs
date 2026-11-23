//! Project CRUD (`mutate`, `ensureScratch`, `createNew`, `createFromThread`) and sub-routing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use legion_contracts::orchestration::PatchValue;
use legion_persistence::project_store::{
    CreateProjectInput, ProjectRecord, archive_project, create_project,
    ensure_project_store_schema, find_project_by_id_or_workspace, list_projects, rename_project,
    soft_delete_project, unarchive_project,
};
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    AppState, fs_browse, orchestration_commands::publish_project_shell_event,
    parse_optional_payload, parse_payload, projects_create, projects_files, projects_search,
    rpc_fail, rpc_fail_obj, utc_now_iso, with_db_mut,
};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "projects.listEntries" => projects_files::list_entries(payload).await,
        "projects.listIgnoredFiles" => projects_files::list_ignored_files(payload).await,
        "projects.readFile" => projects_files::read_file(payload),
        "projects.writeFile" => projects_files::write_file(payload),
        "projects.searchEntries" => projects_search::search_entries(payload).await,
        "projects.searchContents" => projects_search::search_contents(payload).await,
        "projects.mutate" => mutate_project(state, payload),
        "projects.ensureScratch" => ensure_scratch(state, payload),
        "projects.createNew" => projects_create::create_new(state, payload).await,
        "projects.createFromThread" => projects_create::create_from_thread(state, payload).await,
        "filesystem.browse" | "filesystem.getMetadata" | "shell.openInEditor" => {
            fs_browse::dispatch(state, tag, payload)
        }
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutatePayload {
    #[serde(rename = "type")]
    op_type: Option<String>,
    action: Option<String>,
    project_id: Option<String>,
    id: Option<String>,
    title: Option<String>,
    workspace_root: Option<String>,
    create_workspace_root_if_missing: Option<bool>,
    scripts: Option<Value>,
    #[serde(default)]
    default_model_selection: PatchValue<Value>,
    #[serde(default)]
    default_thread_env_mode: PatchValue<String>,
    auto_pull: Option<bool>,
    pinned: Option<bool>,
    archived: Option<bool>,
    force: Option<bool>,
}

fn mutate_err(command_id: &str, project_id: &str, msg: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "ProjectMutationError",
        "commandId": command_id,
        "projectId": project_id,
        "message": msg,
    }))
}

fn mutate_project(state: &AppState, payload: Value) -> RpcExit {
    let command_id = payload
        .get("commandId")
        .and_then(Value::as_str)
        .unwrap_or("cmd-proj")
        .to_owned();
    let req: MutatePayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let op = req
        .op_type
        .as_deref()
        .or(req.action.as_deref())
        .map_or("project.create", str::trim);
    let now_iso = utc_now_iso();
    with_db_mut(state, |conn| {
        ensure_project_store_schema(conn)
            .map_err(|e| rpc_fail("ProjectMutationError", &e.to_string()))?;
        let target_id = req
            .project_id
            .as_deref()
            .or(req.id.as_deref())
            .unwrap_or("");
        let affected_id = apply_project_mutation(conn, op, &command_id, target_id, &req, &now_iso)?;
        let is_deleted = matches!(op, "delete" | "project.delete" | "remove");
        publish_project_shell_event(state, conn, &affected_id, is_deleted, &command_id, &now_iso)
            .map_err(|e| mutate_err(&command_id, &affected_id, &e.to_string()))?;
        let project = list_projects(&*conn)
            .map_err(|e| mutate_err(&command_id, target_id, &e.to_string()))?
            .into_iter()
            .find(|p| p.id == affected_id)
            .ok_or_else(|| mutate_err(&command_id, target_id, "Mutated project not found."))?;
        Ok(RpcExit::Success {
            value: project_to_wire(&project),
        })
    })
    .unwrap_or_else(|e| e)
}

fn apply_project_mutation(
    conn: &rusqlite::Connection,
    op: &str,
    command_id: &str,
    target_id: &str,
    req: &MutatePayload,
    now_iso: &str,
) -> Result<String, RpcExit> {
    match op {
        "create" | "project.create" => {
            let root = req
                .workspace_root
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    mutate_err(
                        command_id,
                        target_id,
                        "workspaceRoot is required for project.create",
                    )
                })?;
            if req.create_workspace_root_if_missing == Some(true)
                && let Err(err) = fs::create_dir_all(root)
            {
                return Err(mutate_err(command_id, target_id, &err.to_string()));
            }
            let mut input = CreateProjectInput::new(root);
            if !target_id.is_empty() {
                input = input.with_project_id(target_id);
            }
            if let Some(title) = req.title.as_deref() {
                input = input.with_title(title);
            }
            let created = create_project(conn, &input)
                .map_err(|e| mutate_err(command_id, target_id, &e.to_string()))?;
            apply_project_extra_patches(conn, &created.id, req, now_iso);
            Ok(created.id)
        }
        "update" | "project.update" | "rename" | "project.rename" => {
            if let Some(title) = req.title.as_deref() {
                rename_project(conn, target_id, title)
                    .map_err(|e| mutate_err(command_id, target_id, &e.to_string()))?;
            }
            if req.archived == Some(true) {
                let _ = archive_project(conn, target_id);
            } else if req.archived == Some(false) {
                let _ = unarchive_project(conn, target_id, None);
            }
            apply_project_extra_patches(conn, target_id, req, now_iso);
            Ok(target_id.to_owned())
        }
        "delete" | "project.delete" | "remove" => {
            soft_delete_project(conn, target_id, req.force.unwrap_or(false))
                .map(|res| res.project.id)
                .map_err(|e| mutate_err(command_id, target_id, &e.to_string()))
        }
        other => Err(mutate_err(
            command_id,
            target_id,
            &format!("Unsupported project mutation `{other}`"),
        )),
    }
}

fn apply_project_extra_patches(
    conn: &rusqlite::Connection,
    project_id: &str,
    req: &MutatePayload,
    now_iso: &str,
) {
    if let Some(scripts) = &req.scripts
        && let Ok(encoded) = serde_json::to_string(scripts)
    {
        let _ = conn.execute(
            "UPDATE projection_projects SET scripts_json = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![encoded, now_iso, project_id],
        );
    }
    match &req.default_model_selection {
        PatchValue::Absent => {}
        PatchValue::Null => {
            let _ = conn.execute(
                "UPDATE projection_projects SET default_model_selection_json = NULL, updated_at = ?1 WHERE project_id = ?2",
                params![now_iso, project_id],
            );
        }
        PatchValue::Set(v) => {
            if let Ok(encoded) = serde_json::to_string(v) {
                let _ = conn.execute(
                    "UPDATE projection_projects SET default_model_selection_json = ?1, updated_at = ?2 WHERE project_id = ?3",
                    params![encoded, now_iso, project_id],
                );
            }
        }
    }
    match &req.default_thread_env_mode {
        PatchValue::Absent => {}
        PatchValue::Null => {
            let _ = conn.execute(
                "UPDATE projection_projects SET default_thread_env_mode = NULL, updated_at = ?1 WHERE project_id = ?2",
                params![now_iso, project_id],
            );
        }
        PatchValue::Set(v) => {
            let _ = conn.execute(
                "UPDATE projection_projects SET default_thread_env_mode = ?1, updated_at = ?2 WHERE project_id = ?3",
                params![v, now_iso, project_id],
            );
        }
    }
    if let Some(auto_pull) = req.auto_pull {
        let _ = conn.execute(
            "UPDATE projection_projects SET auto_pull = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![i64::from(auto_pull), now_iso, project_id],
        );
    }
    if let Some(pinned) = req.pinned {
        let val = pinned.then_some(now_iso);
        let _ = conn.execute(
            "UPDATE projection_projects SET pinned_at = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![val, now_iso, project_id],
        );
    }
}

fn ensure_scratch(state: &AppState, payload: Value) -> RpcExit {
    let _: Value = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let scratch_dir = base_dir_from_state(state).join("scratch");
    if let Err(err) = fs::create_dir_all(&scratch_dir) {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationDispatchCommandError",
            "commandId": "cmd-ensure-scratch",
            "commandType": "project.create",
            "message": err.to_string(),
        }));
    }
    let root_str = scratch_dir.to_string_lossy().into_owned();
    with_db_mut(state, |conn| {
        ensure_project_store_schema(conn)
            .map_err(|e| rpc_fail("OrchestrationDispatchCommandError", &e.to_string()))?;
        let (record, created) = if let Some(existing) =
            find_project_by_id_or_workspace(&*conn, &root_str)
                .ok()
                .flatten()
        {
            (existing, false)
        } else {
            let created_record = create_project(
                &*conn,
                &CreateProjectInput::new(&root_str)
                    .with_project_id("project-scratch")
                    .with_title("Scratch"),
            )
            .map_err(|e| {
                rpc_fail_obj(json!({
                    "_tag": "OrchestrationDispatchCommandError",
                    "commandId": "cmd-ensure-scratch",
                    "commandType": "project.create",
                    "message": e.to_string(),
                }))
            })?;
            (created_record, true)
        };
        if created {
            let now_iso = utc_now_iso();
            publish_project_shell_event(
                state,
                conn,
                &record.id,
                false,
                "cmd-ensure-scratch",
                &now_iso,
            )
            .map_err(|e| {
                rpc_fail_obj(json!({
                    "_tag": "OrchestrationDispatchCommandError",
                    "commandId": "cmd-ensure-scratch",
                    "commandType": "project.create",
                    "message": e.to_string(),
                }))
            })?;
        }
        Ok(RpcExit::Success {
            value: json!({ "projectId": record.id }),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(crate) fn base_dir_from_state(state: &AppState) -> PathBuf {
    state
        .paths
        .state_dir
        .parent()
        .map_or_else(|| state.paths.state_dir.clone(), Path::to_path_buf)
}

pub(crate) fn project_to_wire(p: &ProjectRecord) -> Value {
    let scripts: Value = serde_json::from_str(&p.scripts_json).unwrap_or_else(|_| json!([]));
    let model: Value = p
        .default_model_selection_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let icon: Value = p
        .project_icon_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let non_empty = |opt: &Option<String>| {
        opt.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or(Value::Null, Value::from)
    };
    json!({
        "id": p.id,
        "title": p.title,
        "workspaceRoot": p.workspace_root,
        "defaultModelSelection": model,
        "defaultThreadEnvMode": non_empty(&p.default_thread_env_mode),
        "autoPull": p.auto_pull,
        "faviconPath": non_empty(&p.favicon_path),
        "projectIcon": icon,
        "scripts": scripts,
        "pinnedAt": non_empty(&p.pinned_at),
        "archivedAt": non_empty(&p.archived_at),
        "createdAt": p.created_at,
        "updatedAt": p.updated_at,
        "deletedAt": non_empty(&p.deleted_at),
    })
}
