//! Worktree path normalization and indexed `orchestration_v2_projection_threads` lookup
//! matching `packages/shared/src/path.ts` and `ProjectionStore.ts` (`#36802`).

use rusqlite::{Connection, params};

fn is_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || matches!(bytes[2], b'/' | b'\\'))
}

fn is_unc_path(value: &str) -> bool {
    value.starts_with(r"\\")
}

fn is_root_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value == "/"
        || value == r"\"
        || (bytes.len() == 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn trim_trailing_path_separators(value: &str) -> String {
    if value.is_empty() || is_root_path(value) {
        return value.to_owned();
    }
    let trimmed = if value.starts_with('/') {
        value.trim_end_matches('/')
    } else {
        value.trim_end_matches(['/', '\\'])
    };
    if trimmed.is_empty() {
        return value.to_owned();
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        format!("{trimmed}\\")
    } else {
        trimmed.to_owned()
    }
}

/// Normalizes a project/worktree path for dispatch (`normalizeProjectPathForDispatch`).
pub fn normalize_project_path_for_dispatch(value: &str) -> String {
    trim_trailing_path_separators(value.trim())
}

/// Normalizes a project/worktree path for exact comparison across platforms
/// (`normalizeProjectPathForComparison` in `packages/shared/src/path.ts`).
///
/// POSIX paths preserve case while stripping trailing slashes; Windows drive and UNC paths
/// normalize slashes to `\` and fold case.
pub fn normalize_project_path_for_comparison(value: &str) -> String {
    let normalized = normalize_project_path_for_dispatch(value);
    if is_windows_drive_path(&normalized) || is_unc_path(&normalized) {
        normalized.replace('/', "\\").to_lowercase()
    } else {
        normalized
    }
}

/// Computes the ASCII-folded forward-slash key matching SQLite's
/// `orchestration_v2_projection_threads_worktree_path_key_idx` expression (`#36802`).
pub fn loose_worktree_path_sql_key(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// SQL query selecting matching `(thread_id, payload_json)` rows via
/// `orchestration_v2_projection_threads_worktree_path_key_idx`.
pub(crate) fn worktree_threads_query_sql(include_deleted: bool) -> &'static str {
    if include_deleted {
        "SELECT thread_id, json_extract(payload_json, '$.worktreePath'), payload_json
         FROM orchestration_v2_projection_threads
         WHERE CASE WHEN json_valid(payload_json)
             THEN lower(rtrim(replace(trim(json_extract(payload_json, '$.worktreePath')), '\\', '/'), '/'))
           END = lower(rtrim(replace(trim(?1), '\\', '/'), '/'))
         ORDER BY updated_at ASC, thread_id ASC"
    } else {
        "SELECT thread_id, json_extract(payload_json, '$.worktreePath'), payload_json
         FROM orchestration_v2_projection_threads
         WHERE deleted_at IS NULL
           AND CASE WHEN json_valid(payload_json)
             THEN lower(rtrim(replace(trim(json_extract(payload_json, '$.worktreePath')), '\\', '/'), '/'))
           END = lower(rtrim(replace(trim(?1), '\\', '/'), '/'))
         ORDER BY updated_at ASC, thread_id ASC"
    }
}

/// Looks up `(thread_id, payload_json)` rows in `orchestration_v2_projection_threads` whose
/// `$.worktreePath` matches `worktree_path` under `normalize_project_path_for_comparison`,
/// using `orchestration_v2_projection_threads_worktree_path_key_idx` to avoid full-table scans.
pub fn list_threads_by_worktree_path(
    conn: &Connection,
    worktree_path: &str,
    include_deleted: bool,
) -> rusqlite::Result<Vec<(String, String)>> {
    let target_key = normalize_project_path_for_comparison(worktree_path);
    let mut stmt = conn.prepare(worktree_threads_query_sql(include_deleted))?;
    let rows = stmt.query_map(params![worktree_path], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (thread_id, raw_worktree, payload_json) = row?;
        if let Some(wt) = raw_worktree
            && normalize_project_path_for_comparison(&wt) == target_key
        {
            out.push((thread_id, payload_json));
        }
    }
    Ok(out)
}

/// Convenience wrapper returning only matching `thread_id` values in deterministic shell order.
pub fn list_thread_ids_by_worktree_path(
    conn: &Connection,
    worktree_path: &str,
    include_deleted: bool,
) -> rusqlite::Result<Vec<String>> {
    Ok(
        list_threads_by_worktree_path(conn, worktree_path, include_deleted)?
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect(),
    )
}
