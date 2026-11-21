//! Internal row mapping, input validation, and SQL helpers for `project_store`.

use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::ownership::format_iso8601_utc;

use super::schema::{projection_project_columns, table_exists};
use super::{CreateProjectInput, ProjectRecord, ProjectStoreError};

const THREAD_TABLES: [&str; 2] = ["orchestration_v2_projection_threads", "projection_threads"];

fn sql_err(operation: &'static str, source: rusqlite::Error) -> ProjectStoreError {
    ProjectStoreError::Sqlite { operation, source }
}

pub(super) struct ValidatedCreateInput {
    pub(super) project_id: String,
    pub(super) title: String,
    pub(super) explicit_title: Option<String>,
    pub(super) workspace_root: String,
    pub(super) scripts_json: String,
    pub(super) now_iso: String,
}

impl ValidatedCreateInput {
    pub(super) fn parse(input: &CreateProjectInput) -> Result<Self, ProjectStoreError> {
        if input.workspace_root.trim().is_empty() {
            return Err(ProjectStoreError::InvalidWorkspaceRoot);
        }
        let explicit_title = parse_optional_title(input.title.as_deref())?;
        let title = explicit_title
            .clone()
            .unwrap_or_else(|| default_project_title(&input.workspace_root));
        let scripts_json = match input.scripts_json.as_deref() {
            Some(raw) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(raw).map_err(|_| ProjectStoreError::InvalidScriptsJson)?;
                if !parsed.is_array() {
                    return Err(ProjectStoreError::InvalidScriptsJson);
                }
                raw.to_owned()
            }
            None => "[]".to_owned(),
        };
        let project_id = match input.project_id.as_deref() {
            Some(id) if !id.trim().is_empty() => id.to_owned(),
            _ => Uuid::new_v4().to_string(),
        };
        let now_iso = input
            .now_iso
            .clone()
            .unwrap_or_else(|| format_iso8601_utc(SystemTime::now()));

        Ok(Self {
            project_id,
            title,
            explicit_title,
            workspace_root: input.workspace_root.clone(),
            scripts_json,
            now_iso,
        })
    }
}

pub(super) fn parse_optional_title(raw: Option<&str>) -> Result<Option<String>, ProjectStoreError> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(ProjectStoreError::InvalidTitle)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
    }
}

fn default_project_title(workspace_root: &str) -> String {
    Path::new(workspace_root)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(workspace_root)
        .to_owned()
}

fn build_select_sql(columns: &HashSet<String>, filter: &str) -> String {
    let col = |name: &str, fallback: &str| -> String {
        if columns.contains(name) {
            name.to_owned()
        } else {
            format!("{fallback} AS {name}")
        }
    };
    let select_list = [
        col("project_id", "''"),
        col("title", "''"),
        col("workspace_root", "''"),
        col("has_custom_title", "0"),
        col("scripts_json", "'[]'"),
        col("default_model_selection_json", "NULL"),
        col("default_thread_env_mode", "NULL"),
        col("auto_pull", "0"),
        col("favicon_path", "NULL"),
        col("project_icon_json", "NULL"),
        col("pinned_at", "NULL"),
        col("archived_at", "NULL"),
        col("created_at", "''"),
        col("updated_at", "''"),
        col("deleted_at", "NULL"),
    ]
    .join(", ");
    format!(
        "SELECT {select_list} FROM projection_projects {filter} ORDER BY created_at ASC, project_id ASC"
    )
}

fn map_project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        workspace_root: row.get(2)?,
        has_custom_title: row.get::<_, i64>(3)? != 0,
        scripts_json: row.get(4)?,
        default_model_selection_json: row.get(5)?,
        default_thread_env_mode: row.get(6)?,
        auto_pull: row.get::<_, i64>(7)? != 0,
        favicon_path: row.get(8)?,
        project_icon_json: row.get(9)?,
        pinned_at: row.get(10)?,
        archived_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        deleted_at: row.get(14)?,
    })
}

pub(super) fn read_projects_conn(
    conn: &Connection,
    include_deleted: bool,
) -> Result<Vec<ProjectRecord>, ProjectStoreError> {
    if !table_exists(conn, "projection_projects")? {
        return Ok(Vec::new());
    }
    let columns = projection_project_columns(conn)?;
    let filter = if include_deleted || !columns.contains("deleted_at") {
        ""
    } else {
        "WHERE deleted_at IS NULL"
    };
    let sql = build_select_sql(&columns, filter);
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| sql_err("list_projects:prepare", e))?;
    let rows = stmt
        .query_map([], map_project_row)
        .map_err(|e| sql_err("list_projects:query", e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sql_err("list_projects:row", e))?);
    }
    Ok(out)
}

fn query_single_project(
    conn: &Connection,
    where_clause: &str,
    param: &str,
) -> Result<Option<ProjectRecord>, ProjectStoreError> {
    let columns = projection_project_columns(conn)?;
    let sql = format!(
        "{} LIMIT 1",
        build_select_sql(&columns, &format!("WHERE {where_clause}"))
    );
    match conn.query_row(&sql, [param], map_project_row) {
        Ok(record) => Ok(Some(record)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(sql_err("query_single_project", e)),
    }
}

pub(super) fn find_non_deleted_project_conn(
    conn: &Connection,
    target: &str,
) -> Result<Option<ProjectRecord>, ProjectStoreError> {
    if !table_exists(conn, "projection_projects")? {
        return Ok(None);
    }
    let columns = projection_project_columns(conn)?;
    let deleted_clause = if columns.contains("deleted_at") {
        "deleted_at IS NULL AND "
    } else {
        ""
    };
    if let Some(by_id) =
        query_single_project(conn, &format!("{deleted_clause}project_id = ?1"), target)?
    {
        return Ok(Some(by_id));
    }
    query_single_project(
        conn,
        &format!("{deleted_clause}workspace_root = ?1"),
        target,
    )
}

pub(super) fn find_active_by_workspace_conn(
    conn: &Connection,
    workspace_root: &str,
) -> Result<Option<ProjectRecord>, ProjectStoreError> {
    query_single_project(
        conn,
        "deleted_at IS NULL AND workspace_root = ?1",
        workspace_root,
    )
}

pub(super) fn project_id_exists(
    conn: &Connection,
    project_id: &str,
) -> Result<bool, ProjectStoreError> {
    match conn.query_row(
        "SELECT 1 FROM projection_projects WHERE project_id = ?1 LIMIT 1",
        [project_id],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(sql_err("check_project_id_exists", e)),
    }
}

pub(super) fn get_by_id_conn(
    conn: &Connection,
    project_id: &str,
) -> Result<ProjectRecord, ProjectStoreError> {
    query_single_project(conn, "project_id = ?1", project_id)?.ok_or_else(|| {
        ProjectStoreError::NotFound {
            target: project_id.to_owned(),
        }
    })
}

pub(super) fn insert_project_row(
    conn: &Connection,
    validated: &ValidatedCreateInput,
) -> Result<ProjectRecord, ProjectStoreError> {
    conn.execute(
        "INSERT INTO projection_projects (
           project_id, title, workspace_root, scripts_json, created_at, updated_at,
           deleted_at, default_model_selection_json, default_thread_env_mode,
           favicon_path, auto_pull, project_icon_json, pinned_at, archived_at, has_custom_title
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL, NULL, NULL, NULL, 0, NULL, NULL, NULL, 0)",
        params![
            validated.project_id,
            validated.title,
            validated.workspace_root,
            validated.scripts_json,
            validated.now_iso,
        ],
    )
    .map_err(|e| sql_err("create_project", e))?;
    get_by_id_conn(conn, &validated.project_id)
}

pub(super) fn apply_unarchive_conn(
    conn: &Connection,
    project_id: &str,
    title_override: Option<&str>,
    now: &str,
) -> Result<ProjectRecord, ProjectStoreError> {
    if let Some(new_title) = title_override {
        conn.execute(
            "UPDATE projection_projects
             SET archived_at = NULL, title = ?1, has_custom_title = 1, updated_at = ?2
             WHERE project_id = ?3 AND deleted_at IS NULL",
            params![new_title, now, project_id],
        )
        .map_err(|e| sql_err("unarchive_project_with_title", e))?;
    } else {
        conn.execute(
            "UPDATE projection_projects
             SET archived_at = NULL, updated_at = ?1
             WHERE project_id = ?2 AND deleted_at IS NULL",
            params![now, project_id],
        )
        .map_err(|e| sql_err("unarchive_project", e))?;
    }
    get_by_id_conn(conn, project_id)
}

pub(super) fn count_non_deleted_threads(
    conn: &Connection,
    project_id: &str,
) -> Result<usize, ProjectStoreError> {
    let mut thread_ids = HashSet::new();
    for table_name in THREAD_TABLES {
        if !table_exists(conn, table_name)? {
            continue;
        }
        let sql = format!(
            "SELECT thread_id FROM {table_name} WHERE project_id = ?1 AND deleted_at IS NULL"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| sql_err("count_project_threads:prepare", e))?;
        let rows = stmt
            .query_map([project_id], |row| row.get::<_, String>(0))
            .map_err(|e| sql_err("count_project_threads:query", e))?;
        for row in rows {
            thread_ids.insert(row.map_err(|e| sql_err("count_project_threads:row", e))?);
        }
    }
    Ok(thread_ids.len())
}

pub(super) fn soft_delete_child_threads(
    conn: &Connection,
    project_id: &str,
    now: &str,
) -> Result<(), ProjectStoreError> {
    for table_name in THREAD_TABLES {
        if !table_exists(conn, table_name)? {
            continue;
        }
        let sql = format!(
            "UPDATE {table_name} SET deleted_at = ?1, updated_at = ?1 WHERE project_id = ?2 AND deleted_at IS NULL"
        );
        conn.execute(&sql, params![now, project_id])
            .map_err(|e| sql_err("soft_delete_child_threads", e))?;
    }
    Ok(())
}
