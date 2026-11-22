//! Wire project queries (`query_wire_projects`) and shell project digest helpers.

use legion_contracts::orchestration::sha256_digest;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::projections::ProjectionError;
use crate::projections::wire_codec::{encode_base64url, normalize_model_selection_wire};
use crate::projections::wire_types::WireProjectShell;
use crate::time::now_iso_utc;

/// Wire JSON representation of a project matching `packages/contracts/src/project.ts` (`Project`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProject {
    /// Stable project identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Absolute workspace root directory path.
    pub workspace_root: String,
    /// Repository identity descriptor (`null` until enriched).
    pub repository_identity: Option<Value>,
    /// Optional favicon path.
    pub favicon_path: Option<String>,
    /// Optional project icon descriptor.
    pub project_icon: Option<Value>,
    /// Optional default model selection.
    pub default_model_selection: Option<Value>,
    /// Optional default thread environment mode (`"local"` or `"worktree"`).
    pub default_thread_env_mode: Option<String>,
    /// Whether background auto-pull is enabled.
    pub auto_pull: bool,
    /// Configured setup/dev scripts.
    pub scripts: Vec<Value>,
    /// Pin timestamp if pinned.
    pub pinned_at: Option<String>,
    /// Archive timestamp if archived.
    pub archived_at: Option<String>,
    /// Creation timestamp (ISO-8601 UTC).
    pub created_at: String,
    /// Last update timestamp (ISO-8601 UTC).
    pub updated_at: String,
    /// Soft deletion timestamp (`null` when active).
    pub deleted_at: Option<String>,
}

/// Wire JSON snapshot returned by `GET /api/projects` (`ProjectSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProjectSnapshot {
    /// Non-deleted projects ordered by creation timestamp.
    pub projects: Vec<WireProject>,
    /// Snapshot timestamp (ISO-8601 UTC).
    pub updated_at: String,
}

struct ProjectDbRow {
    id: String,
    title: String,
    has_custom_title: bool,
    workspace_root: String,
    default_model_selection: Option<Value>,
    default_thread_env_mode: Option<String>,
    auto_pull: bool,
    favicon_path: Option<String>,
    project_icon: Option<Value>,
    scripts: Vec<Value>,
    pinned_at: Option<String>,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

/// Queries all active projects in `ProjectSnapshot` wire format (`packages/contracts/src/project.ts`).
pub fn query_wire_projects(conn: &Connection) -> Result<WireProjectSnapshot, ProjectionError> {
    let rows = load_project_rows(conn, None)?;
    let projects = rows
        .into_iter()
        .map(|row| WireProject {
            id: row.id,
            title: row.title,
            workspace_root: row.workspace_root,
            repository_identity: None,
            favicon_path: row.favicon_path,
            project_icon: row.project_icon,
            default_model_selection: row.default_model_selection,
            default_thread_env_mode: row.default_thread_env_mode,
            auto_pull: row.auto_pull,
            scripts: row.scripts,
            pinned_at: row.pinned_at,
            archived_at: row.archived_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            deleted_at: row.deleted_at,
        })
        .collect();
    Ok(WireProjectSnapshot {
        projects,
        updated_at: now_iso_utc(),
    })
}

/// Queries all non-deleted project shells ordered identically to `ProjectStore.listShells`.
pub(crate) fn query_wire_project_shells(
    conn: &Connection,
) -> Result<Vec<WireProjectShell>, ProjectionError> {
    Ok(load_project_rows(conn, None)?
        .into_iter()
        .map(into_project_shell)
        .collect())
}

/// Queries a single non-deleted project shell by ID (`None` if missing or soft-deleted).
pub(crate) fn query_wire_project_shell_by_id(
    conn: &Connection,
    project_id: &str,
) -> Result<Option<WireProjectShell>, ProjectionError> {
    Ok(load_project_rows(conn, Some(project_id))?
        .into_iter()
        .next()
        .map(into_project_shell))
}

/// Matches `projectIdentitiesDigest` in `apps/server/src/orchestration-v2/ShellStream.ts:111-119`.
pub(crate) fn compute_project_identities_digest(projects: &[WireProjectShell]) -> String {
    let mut material = Vec::new();
    for project in projects {
        let pair = serde_json::json!([&project.id, &project.repository_identity]);
        if let Ok(encoded) = serde_json::to_vec(&pair) {
            material.extend_from_slice(&encoded);
        }
    }
    let digest = sha256_digest(&material);
    encode_base64url(&digest)
}

fn into_project_shell(row: ProjectDbRow) -> WireProjectShell {
    WireProjectShell {
        id: row.id,
        title: row.title,
        has_custom_title: row.has_custom_title,
        workspace_root: row.workspace_root,
        repository_identity: None,
        default_model_selection: row.default_model_selection,
        default_thread_env_mode: row.default_thread_env_mode,
        auto_pull: row.auto_pull,
        favicon_path: row.favicon_path,
        project_icon: row.project_icon,
        scripts: row.scripts,
        pinned_at: row.pinned_at,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn load_project_rows(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<ProjectDbRow>, ProjectionError> {
    let base_select = "
      SELECT
        project_id,
        title,
        COALESCE(has_custom_title, 0) AS has_custom_title,
        workspace_root,
        default_model_selection_json,
        default_thread_env_mode,
        COALESCE(auto_pull, 0) AS auto_pull,
        favicon_path,
        project_icon_json,
        scripts_json,
        pinned_at,
        archived_at,
        created_at,
        updated_at,
        deleted_at
      FROM projection_projects
      WHERE deleted_at IS NULL
    ";
    let mut rows = Vec::new();
    if let Some(pid) = project_id {
        let query = format!("{base_select} AND project_id = ?1 LIMIT 1");
        let maybe_row = conn
            .query_row(&query, params![pid], decode_project_row)
            .optional()?;
        if let Some(row) = maybe_row {
            rows.push(row?);
        }
    } else {
        let query = format!("{base_select} ORDER BY created_at ASC, project_id ASC");
        let mut stmt = conn.prepare(&query)?;
        let iter = stmt.query_map([], decode_project_row)?;
        for item in iter {
            rows.push(item??);
        }
    }
    Ok(rows)
}

fn decode_project_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<ProjectDbRow, ProjectionError>> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let has_custom_title = row.get::<_, i64>(2)? != 0;
    let workspace_root: String = row.get(3)?;
    let model_raw: Option<String> = row.get(4)?;
    let default_thread_env_mode: Option<String> = row.get(5)?;
    let auto_pull = row.get::<_, i64>(6)? != 0;
    let favicon_path: Option<String> = row.get(7)?;
    let icon_raw: Option<String> = row.get(8)?;
    let scripts_raw: String = row.get(9)?;
    let pinned_at: Option<String> = row.get(10)?;
    let archived_at: Option<String> = row.get(11)?;
    let created_at: String = row.get(12)?;
    let updated_at: String = row.get(13)?;
    let deleted_at: Option<String> = row.get(14)?;

    Ok(decode_project_fields(
        id,
        title,
        has_custom_title,
        workspace_root,
        model_raw.as_deref(),
        default_thread_env_mode,
        auto_pull,
        favicon_path,
        icon_raw.as_deref(),
        &scripts_raw,
        pinned_at,
        archived_at,
        created_at,
        updated_at,
        deleted_at,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "maps 1:1 SQL projection_projects columns into decoded wire record"
)]
fn decode_project_fields(
    id: String,
    title: String,
    has_custom_title: bool,
    workspace_root: String,
    model_raw: Option<&str>,
    default_thread_env_mode: Option<String>,
    auto_pull: bool,
    favicon_path: Option<String>,
    icon_raw: Option<&str>,
    scripts_raw: &str,
    pinned_at: Option<String>,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
) -> Result<ProjectDbRow, ProjectionError> {
    let default_model_selection = model_raw
        .map(|raw| serde_json::from_str::<Value>(raw).map(normalize_model_selection_wire))
        .transpose()
        .map_err(|e| ProjectionError::Codec(format!("project modelSelection decode: {e}")))?;
    let project_icon = icon_raw
        .map(|raw| serde_json::from_str::<Value>(raw).map(normalize_project_icon))
        .transpose()
        .map_err(|e| ProjectionError::Codec(format!("project icon decode: {e}")))?;
    let scripts: Vec<Value> = serde_json::from_str(scripts_raw)
        .map_err(|e| ProjectionError::Codec(format!("project scripts decode: {e}")))?;

    Ok(ProjectDbRow {
        id,
        title,
        has_custom_title,
        workspace_root,
        default_model_selection,
        default_thread_env_mode,
        auto_pull,
        favicon_path,
        project_icon,
        scripts,
        pinned_at,
        archived_at,
        created_at,
        updated_at,
        deleted_at,
    })
}

fn normalize_project_icon(mut icon: Value) -> Value {
    if let Some(obj) = icon.as_object_mut()
        && obj.get("kind").and_then(Value::as_str) == Some("lucide")
        && let Some(text) = obj
            .remove("monogramText")
            .or_else(|| obj.remove("monogram"))
            .filter(Value::is_string)
    {
        obj.insert("kind".to_owned(), Value::String("monogram".to_owned()));
        obj.insert("text".to_owned(), text);
    }
    icon
}
