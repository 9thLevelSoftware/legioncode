//! Authenticated `/api/projects` & `/api/projects/mutate` handlers backed by `legion_persistence::project_store`.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_persistence::project_store::{
    CreateProjectInput, archive_project, create_project, ensure_project_store_schema,
    find_project_by_id_or_workspace, list_non_deleted_projects, rename_project,
    soft_delete_project, unarchive_project,
};
use rusqlite::{Connection, params};
use serde_json::json;

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::{authorize_request, parse_authorized_json};
use crate::http_projects_models::{
    HttpProject, HttpProjectSnapshot, ProjectMutateRequest, apply_extra_fields,
};
use crate::http_projects_read::{
    apply_saved_project_settings, apply_saved_project_settings_snapshot,
};
use crate::outbox_state::utc_now_iso;
use legion_contracts::permissions::EnvironmentScope;

/// `GET /api/projects` — return all non-deleted projects after `BUG-03` credential check.
pub(crate) async fn handle_get_projects(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    ensure_project_store_schema(&conn)?;
    let snapshot = apply_saved_project_settings_snapshot(&state, build_snapshot(&conn)?);
    Ok((StatusCode::OK, Json(snapshot)).into_response())
}

/// `POST /api/projects/mutate` — execute `project.create`, `project.update`, or `project.delete`.
pub(crate) async fn handle_mutate_project(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServerRuntimeError> {
    let (_principal, req): (_, ProjectMutateRequest) = parse_authorized_json(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationOperate],
        true,
        &body,
    )?;
    let now_iso = utc_now_iso();
    let op = req
        .op_type
        .as_deref()
        .or(req.action.as_deref())
        .map_or("", str::trim);

    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    ensure_project_store_schema(&conn)?;

    let project = match op {
        "project.create" | "create" => Some(apply_create(&conn, &req, &now_iso)?),
        "project.update" | "update" => Some(apply_update(&conn, &req, &now_iso)?),
        "project.archive" | "archive" => {
            let target = resolve_target(&req)?;
            Some(HttpProject::from(archive_project(&*conn, &target)?))
        }
        "project.unarchive" | "unarchive" => {
            let target = resolve_target(&req)?;
            Some(HttpProject::from(unarchive_project(
                &*conn,
                &target,
                req.title.as_deref(),
            )?))
        }
        "project.delete" | "delete" => {
            let target = resolve_target(&req)?;
            let _ = soft_delete_project(&*conn, &target, req.force.unwrap_or(false))?;
            None
        }
        other => {
            return Err(ServerRuntimeError::BadRequest(format!(
                "unsupported project mutation `{other}`"
            )));
        }
    };

    let project = project.map(|p| apply_saved_project_settings(&state, p));
    let snapshot = apply_saved_project_settings_snapshot(&state, build_snapshot(&conn)?);
    Ok((
        StatusCode::OK,
        Json(json!({
            "project": project,
            "snapshot": snapshot,
        })),
    )
        .into_response())
}

fn resolve_target(req: &ProjectMutateRequest) -> Result<String, ServerRuntimeError> {
    req.project_id
        .as_deref()
        .or(req.id.as_deref())
        .or(req.workspace_root.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ServerRuntimeError::BadRequest("missing projectId or id".to_owned()))
}

fn apply_create(
    conn: &Connection,
    req: &ProjectMutateRequest,
    now_iso: &str,
) -> Result<HttpProject, ServerRuntimeError> {
    let ws = req
        .workspace_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServerRuntimeError::BadRequest("missing workspaceRoot".to_owned()))?;
    let mut input = CreateProjectInput::new(ws).with_now_iso(now_iso);
    if let Some(id) = req.project_id.as_deref().or(req.id.as_deref()) {
        input = input.with_project_id(id);
    }
    if let Some(ref title) = req.title {
        input = input.with_title(title);
    }
    if let Some(ref scripts) = req.scripts {
        input = input.with_scripts_json(serde_json::to_string(scripts)?);
    }
    let created = create_project(conn, &input)?;
    apply_extra_fields(conn, &created.id, req, now_iso)?;
    reload_project(conn, &created.id)
}

fn apply_update(
    conn: &Connection,
    req: &ProjectMutateRequest,
    now_iso: &str,
) -> Result<HttpProject, ServerRuntimeError> {
    let target = resolve_target(req)?;
    let existing = find_project_by_id_or_workspace(conn, &target)?
        .ok_or_else(|| ServerRuntimeError::NotFound(target.clone()))?;
    let project_id = existing.id;

    if let Some(ref new_title) = req.title {
        let _ = rename_project(conn, &project_id, new_title)?;
    }
    if let Some(archived) = req.archived {
        if archived {
            let _ = archive_project(conn, &project_id)?;
        } else {
            let _ = unarchive_project(conn, &project_id, req.title.as_deref())?;
        }
    }
    if let Some(ref ws) = req.workspace_root {
        let trimmed = ws.trim();
        if trimmed.is_empty() {
            return Err(ServerRuntimeError::BadRequest(
                "workspaceRoot cannot be empty".to_owned(),
            ));
        }
        conn.execute(
            "UPDATE projection_projects SET workspace_root = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![trimmed, now_iso, project_id],
        )?;
    }
    if let Some(ref scripts) = req.scripts {
        let json_str = serde_json::to_string(scripts)?;
        conn.execute(
            "UPDATE projection_projects SET scripts_json = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![json_str, now_iso, project_id],
        )?;
    }
    apply_extra_fields(conn, &project_id, req, now_iso)?;
    reload_project(conn, &project_id)
}

fn reload_project(conn: &Connection, project_id: &str) -> Result<HttpProject, ServerRuntimeError> {
    let rec = find_project_by_id_or_workspace(conn, project_id)?
        .ok_or_else(|| ServerRuntimeError::NotFound(project_id.to_owned()))?;
    Ok(HttpProject::from(rec))
}

fn build_snapshot(conn: &Connection) -> Result<HttpProjectSnapshot, ServerRuntimeError> {
    let records = list_non_deleted_projects(conn)?;
    Ok(HttpProjectSnapshot {
        projects: records.into_iter().map(HttpProject::from).collect(),
        updated_at: utc_now_iso(),
    })
}
