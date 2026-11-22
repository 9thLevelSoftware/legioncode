//! Wire models and SQL column patch helpers for project HTTP routes.

use legion_contracts::orchestration::PatchValue;
use legion_persistence::project_store::ProjectRecord;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ServerRuntimeError;

/// Wire-compatible JSON representation of a workspace project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpProject {
    /// Stable project identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Absolute workspace root directory path.
    pub workspace_root: String,
    /// Whether the title was explicitly customized by the user.
    pub has_custom_title: bool,
    /// Optional default model selection JSON object.
    pub default_model_selection: Option<Value>,
    /// Optional default environment mode.
    pub default_thread_env_mode: Option<String>,
    /// Whether auto-pull is enabled.
    pub auto_pull: bool,
    /// Optional favicon path.
    pub favicon_path: Option<String>,
    /// Optional project icon descriptor.
    pub project_icon: Option<Value>,
    /// Configured setup/dev scripts.
    pub scripts: Value,
    /// Pin timestamp if pinned.
    pub pinned_at: Option<String>,
    /// Archive timestamp if archived.
    pub archived_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Soft deletion timestamp if deleted.
    pub deleted_at: Option<String>,
}

/// Snapshot payload returned by `GET /api/projects` and `POST /api/projects/mutate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpProjectSnapshot {
    /// Active/archived non-deleted projects.
    pub projects: Vec<HttpProject>,
    /// Snapshot generation timestamp.
    pub updated_at: String,
}

impl From<ProjectRecord> for HttpProject {
    fn from(r: ProjectRecord) -> Self {
        let scripts = serde_json::from_str(&r.scripts_json).unwrap_or_else(|_| json!([]));
        let default_model_selection = r
            .default_model_selection_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let project_icon = r
            .project_icon_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        Self {
            id: r.id,
            title: r.title,
            workspace_root: r.workspace_root,
            has_custom_title: r.has_custom_title,
            default_model_selection,
            default_thread_env_mode: r.default_thread_env_mode,
            auto_pull: r.auto_pull,
            favicon_path: r.favicon_path,
            project_icon,
            scripts,
            pinned_at: r.pinned_at,
            archived_at: r.archived_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectMutateRequest {
    #[serde(rename = "type")]
    pub op_type: Option<String>,
    pub action: Option<String>,
    pub project_id: Option<String>,
    pub id: Option<String>,
    pub title: Option<String>,
    pub workspace_root: Option<String>,
    pub scripts: Option<Value>,
    #[serde(default)]
    pub default_model_selection: PatchValue<Value>,
    #[serde(default)]
    pub default_thread_env_mode: PatchValue<String>,
    pub auto_pull: Option<bool>,
    #[serde(default)]
    pub favicon_path: PatchValue<String>,
    #[serde(default)]
    pub project_icon: PatchValue<Value>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub force: Option<bool>,
}

fn update_patch_col(
    conn: &Connection,
    column: &str,
    val: Option<&str>,
    now_iso: &str,
    project_id: &str,
) -> Result<(), ServerRuntimeError> {
    let sql = format!(
        "UPDATE projection_projects SET {column} = ?1, updated_at = ?2 WHERE project_id = ?3"
    );
    conn.execute(&sql, params![val, now_iso, project_id])?;
    Ok(())
}

fn patch_text_col(
    conn: &Connection,
    column: &str,
    patch: &PatchValue<String>,
    now_iso: &str,
    project_id: &str,
) -> Result<(), ServerRuntimeError> {
    match patch {
        PatchValue::Absent => Ok(()),
        PatchValue::Null => update_patch_col(conn, column, None, now_iso, project_id),
        PatchValue::Set(v) => update_patch_col(conn, column, Some(v), now_iso, project_id),
    }
}

fn patch_json_col(
    conn: &Connection,
    column: &str,
    patch: &PatchValue<Value>,
    now_iso: &str,
    project_id: &str,
) -> Result<(), ServerRuntimeError> {
    match patch {
        PatchValue::Absent => Ok(()),
        PatchValue::Null => update_patch_col(conn, column, None, now_iso, project_id),
        PatchValue::Set(v) => {
            let encoded = serde_json::to_string(v)?;
            update_patch_col(conn, column, Some(&encoded), now_iso, project_id)
        }
    }
}

pub(crate) fn apply_extra_fields(
    conn: &Connection,
    project_id: &str,
    req: &ProjectMutateRequest,
    now_iso: &str,
) -> Result<(), ServerRuntimeError> {
    patch_json_col(
        conn,
        "default_model_selection_json",
        &req.default_model_selection,
        now_iso,
        project_id,
    )?;
    patch_text_col(
        conn,
        "default_thread_env_mode",
        &req.default_thread_env_mode,
        now_iso,
        project_id,
    )?;
    if let Some(auto_pull) = req.auto_pull {
        conn.execute(
            "UPDATE projection_projects SET auto_pull = ?1, updated_at = ?2 WHERE project_id = ?3",
            params![i64::from(auto_pull), now_iso, project_id],
        )?;
    }
    patch_text_col(conn, "favicon_path", &req.favicon_path, now_iso, project_id)?;
    patch_json_col(
        conn,
        "project_icon_json",
        &req.project_icon,
        now_iso,
        project_id,
    )?;
    if let Some(pinned) = req.pinned {
        update_patch_col(
            conn,
            "pinned_at",
            pinned.then_some(now_iso),
            now_iso,
            project_id,
        )?;
    }
    Ok(())
}
