//! Saved-settings overrides (`defaultProjectScripts`, `defaultModelSelection`) in `legion_project_read` (`#39676`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use legion_contracts::permissions::EnvironmentScope;
use legion_contracts::workspace_folders::ProjectTrackedFolder;
use legion_persistence::project_store::{
    ensure_project_store_schema, find_project_by_id_or_workspace,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth_guard::authorize_request;
use crate::http_projects_models::{HttpProject, HttpProjectSnapshot};

/// Saved script / action entry resolved for a project (`#39676`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScriptAction {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default = "default_script_icon")]
    pub icon: String,
    #[serde(default)]
    pub run_on_worktree_create: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keybinding: Option<String>,
}

fn default_script_icon() -> String {
    "play".to_owned()
}

/// Per-project Settings overrides (`settings.json` / runtime store) applied on top of stale SQLite columns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettingsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model_selection: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project_scripts: Option<Vec<ProjectScriptAction>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracked_folders: Option<Vec<ProjectTrackedFolder>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectReadQuery {
    pub project_id: Option<String>,
}

/// Resolves project scripts from Settings override first, falling back to normalized DB scripts (`#39676`).
pub fn resolve_project_scripts_value(
    override_cfg: Option<&ProjectSettingsOverride>,
    db_scripts: &Value,
) -> Value {
    if let Some(cfg) = override_cfg
        && let Some(scripts) = &cfg.default_project_scripts
    {
        return serde_json::to_value(scripts).unwrap_or_else(|_| json!([]));
    }
    if db_scripts.is_array() {
        return db_scripts.clone();
    }
    json!([])
}

/// Applies saved per-project Settings (`defaultModelSelection` and `defaultProjectScripts`) to `project`.
pub fn apply_saved_project_settings(
    state: &RuntimeHttpState,
    mut project: HttpProject,
) -> HttpProject {
    let override_opt = state
        .project_settings_overrides
        .lock()
        .ok()
        .and_then(|map| map.get(&project.id).cloned());
    if let Some(ref cfg) = override_opt {
        if let Some(ref model) = cfg.default_model_selection {
            project.default_model_selection = Some(model.clone());
        }
        project.scripts = resolve_project_scripts_value(Some(cfg), &project.scripts);
    }
    project
}

/// Applies saved per-project Settings across a snapshot (`list`, `create`, `update`).
pub fn apply_saved_project_settings_snapshot(
    state: &RuntimeHttpState,
    mut snapshot: HttpProjectSnapshot,
) -> HttpProjectSnapshot {
    snapshot.projects = snapshot
        .projects
        .into_iter()
        .map(|p| apply_saved_project_settings(state, p))
        .collect();
    snapshot
}

/// Reads a single project record enriched with saved Settings actions (`scripts`) and `defaultModelSelection` (`#39676`).
pub fn read_project_with_saved_settings(
    state: &RuntimeHttpState,
    project_id: &str,
) -> Result<HttpProject, ServerRuntimeError> {
    let trimmed = project_id.trim();
    if trimmed.is_empty() {
        return Err(ServerRuntimeError::BadRequest(
            "projectId is required".to_owned(),
        ));
    }
    let conn = state
        .db()
        .lock()
        .map_err(|_| ServerRuntimeError::Persistence("db mutex poisoned".to_owned()))?;
    ensure_project_store_schema(&conn)?;
    let Some(record) = find_project_by_id_or_workspace(&*conn, trimmed)? else {
        return Err(ServerRuntimeError::NotFound(format!(
            "Project `{trimmed}` was not found."
        )));
    };
    Ok(apply_saved_project_settings(
        state,
        HttpProject::from(record),
    ))
}

/// `GET /api/projects/read?projectId=...` — `legion_project_read` HTTP handler (`OrchestrationRead`, `#39676`).
pub(crate) async fn handle_read_project_query(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Query(query): Query<ProjectReadQuery>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    let Some(pid) = query.project_id else {
        return Err(ServerRuntimeError::BadRequest(
            "projectId query parameter is required".to_owned(),
        ));
    };
    let project = read_project_with_saved_settings(&state, &pid)?;
    Ok((StatusCode::OK, Json(project)).into_response())
}

/// `GET /api/projects/{id}` — project detail read enriched with saved Settings actions (`OrchestrationRead`, `#39676`).
pub(crate) async fn handle_read_project_by_id(
    State(state): State<Arc<RuntimeHttpState>>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Response, ServerRuntimeError> {
    let _principal = authorize_request(
        &state,
        &headers,
        &[EnvironmentScope::OrchestrationRead],
        false,
    )?;
    let project = read_project_with_saved_settings(&state, &project_id)?;
    Ok((StatusCode::OK, Json(project)).into_response())
}
