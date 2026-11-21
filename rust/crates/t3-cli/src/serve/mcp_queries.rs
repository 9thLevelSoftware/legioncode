//! SQLite projection queries powering `/mcp` tools (`t3_project_list`, `t3_thread_list`, `t3_thread_read`).

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use t3_persistence::project_store::{ensure_project_store_schema, list_non_deleted_projects};

/// Executes `t3_project_list` over `projection_projects`.
pub(super) fn project_list(conn: &Connection, args: &Value) -> Result<Value, String> {
    let cursor =
        usize::try_from(args.get("cursor").and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);
    let limit = clamp_limit(args, "limit", 100, 100);
    ensure_project_store_schema(conn).map_err(|e| e.to_string())?;
    let all = list_non_deleted_projects(conn).map_err(|e| e.to_string())?;
    let end = cursor.saturating_add(limit).min(all.len());
    let next_cursor = (end < all.len()).then_some(end);
    let projects: Vec<Value> = all
        .get(cursor..end)
        .unwrap_or(&[])
        .iter()
        .map(|p| json!({ "id": p.id, "title": p.title, "workspaceRoot": p.workspace_root }))
        .collect();
    Ok(json!({ "projects": projects, "nextCursor": next_cursor }))
}

/// Executes `t3_thread_list` over V2 thread/run/message projections.
pub(super) fn thread_list(conn: &Connection, args: &Value) -> Result<Value, String> {
    let project_id = args
        .get("projectId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let cursor =
        usize::try_from(args.get("cursor").and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);
    let limit = clamp_limit(args, "limit", 100, 100);

    let mut stmt = conn
        .prepare(
            "SELECT thread_id, project_id, title, COALESCE(provider_instance_id, default_provider),
                    COALESCE(json_extract(payload_json, '$.modelSelection.model'), ''),
                    created_at, updated_at
             FROM orchestration_v2_projection_threads
             WHERE deleted_at IS NULL AND archived_at IS NULL
               AND (?1 IS NULL OR project_id = ?1)
             ORDER BY updated_at DESC, thread_id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut all = Vec::new();
    for row in rows {
        let (tid, pid, title, provider, model, created_at, updated_at) =
            row.map_err(|e| e.to_string())?;
        let (latest_run_id, status) = run_status_for(conn, &tid, None)?;
        let item_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM orchestration_v2_projection_messages WHERE thread_id = ?1",
                [&tid],
                |r| r.get(0),
            )
            .unwrap_or(0);
        all.push(json!({
            "threadId": tid,
            "projectId": pid,
            "title": title,
            "status": status,
            "latestRunId": latest_run_id,
            "providerInstanceId": provider,
            "model": model,
            "itemCount": item_count,
            "createdAt": created_at,
            "updatedAt": updated_at,
        }));
    }
    let total = all.len();
    let end = cursor.saturating_add(limit).min(total);
    Ok(json!({
        "projectId": project_id,
        "threads": all.get(cursor..end).unwrap_or(&[]),
        "nextCursor": (end < total).then_some(end),
        "total": total,
    }))
}

/// Executes `t3_thread_read` for one page of thread metadata, recent runs, and timeline items.
pub(super) fn thread_read(
    conn: &Connection,
    thread_id: &str,
    args: &Value,
) -> Result<Value, String> {
    let after_pos = args.get("afterPosition").and_then(Value::as_i64);
    let limit = clamp_limit(args, "limit", 100, 100);
    let max_chars = clamp_limit(args, "maxCharsPerItem", 20_000, 20_000);

    let (project_id, title, provider, model, branch, worktree) = conn
        .query_row(
            "SELECT project_id, title, COALESCE(provider_instance_id, default_provider),
                    COALESCE(json_extract(payload_json, '$.modelSelection.model'), ''),
                    json_extract(payload_json, '$.branch'),
                    json_extract(payload_json, '$.worktreePath')
             FROM orchestration_v2_projection_threads
             WHERE thread_id = ?1 AND deleted_at IS NULL",
            [thread_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .map_err(|_| format!("Thread `{thread_id}` not found."))?;

    let (latest_run_id, status) = run_status_for(conn, thread_id, None)?;
    let recent_runs = query_recent_runs(conn, thread_id)?;
    let (all_items, total_items) = query_thread_items(conn, thread_id, max_chars)?;
    let filtered: Vec<Value> = all_items
        .into_iter()
        .filter(|item| {
            let pos = item.get("position").and_then(Value::as_i64).unwrap_or(0);
            after_pos.is_none_or(|after| pos > after)
        })
        .collect();
    let has_more = filtered.len() > limit;
    let items: Vec<Value> = filtered.into_iter().take(limit).collect();
    let next_position = items
        .last()
        .and_then(|it| it.get("position").and_then(Value::as_u64));

    Ok(json!({
        "thread": {
            "threadId": thread_id,
            "projectId": project_id,
            "title": title,
            "status": status,
            "providerInstanceId": provider,
            "model": model,
            "branch": branch,
            "worktreePath": worktree,
            "latestRunId": latest_run_id,
            "pendingRequestCount": 0,
            "itemCount": total_items,
        },
        "recentRuns": recent_runs,
        "items": items,
        "nextPosition": next_position,
        "hasMore": has_more,
    }))
}

pub(super) fn run_status_for(
    conn: &Connection,
    thread_id: &str,
    run_id: Option<&str>,
) -> Result<(Option<String>, String), String> {
    let row = match run_id {
        Some(rid) => conn.query_row(
            "SELECT run_id, status FROM orchestration_v2_projection_runs
             WHERE thread_id = ?1 AND run_id = ?2 LIMIT 1",
            params![thread_id, rid],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ),
        None => conn.query_row(
            "SELECT run_id, status FROM orchestration_v2_projection_runs
             WHERE thread_id = ?1 ORDER BY ordinal DESC LIMIT 1",
            [thread_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ),
    };
    match row {
        Ok((rid, status)) => Ok((Some(rid), status)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok((None, "idle".to_owned())),
        Err(err) => Err(err.to_string()),
    }
}

/// Reads stored typed run output JSON value (`$.output.value`) for `run_id`, if submitted.
pub(super) fn read_run_output_for(
    conn: &Connection,
    thread_id: &str,
    run_id: Option<&str>,
) -> Option<Value> {
    let rid = run_id?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT json_extract(payload_json, '$.output.value')
             FROM orchestration_v2_projection_runs
             WHERE thread_id = ?1 AND run_id = ?2 LIMIT 1",
            params![thread_id, rid],
            |r| r.get(0),
        )
        .ok()?;
    raw.and_then(|s| serde_json::from_str::<Value>(&s).ok())
}

fn query_recent_runs(conn: &Connection, thread_id: &str) -> Result<Vec<Value>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT run_id, ordinal, status FROM orchestration_v2_projection_runs
             WHERE thread_id = ?1 ORDER BY ordinal DESC LIMIT 50",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([thread_id], |r| {
            Ok(json!({
                "runId": r.get::<_, String>(0)?,
                "ordinal": r.get::<_, i64>(1)?,
                "status": r.get::<_, String>(2)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn query_thread_items(
    conn: &Connection,
    thread_id: &str,
    max_chars: usize,
) -> Result<(Vec<Value>, usize), String> {
    let mut stmt = conn
        .prepare(
            "SELECT message_id, run_id, role, COALESCE(json_extract(payload_json, '$.text'), '')
             FROM orchestration_v2_projection_messages
             WHERE thread_id = ?1
             ORDER BY created_at ASC, message_id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([thread_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for (idx, row) in rows.enumerate() {
        let (msg_id, run_id, role, text) = row.map_err(|e| e.to_string())?;
        let item_type = match role.as_str() {
            "user" => "user_message",
            "assistant" => "assistant_message",
            _ => "error",
        };
        let char_count = text.chars().count();
        let (truncated_text, truncated, next_offset) = if char_count > max_chars {
            let cut: String = text.chars().take(max_chars).collect();
            (cut, true, Some(max_chars))
        } else {
            (text, false, None)
        };
        out.push(json!({
            "position": idx,
            "itemId": msg_id,
            "runId": run_id,
            "type": item_type,
            "text": truncated_text,
            "textTruncated": truncated,
            "nextTextOffset": next_offset,
        }));
    }
    let total = out.len();
    Ok((out, total))
}

fn clamp_limit(args: &Value, key: &str, default_val: u64, max_val: u64) -> usize {
    usize::try_from(
        args.get(key)
            .and_then(Value::as_u64)
            .unwrap_or(default_val)
            .clamp(1, max_val),
    )
    .unwrap_or(usize::try_from(default_val).unwrap_or(100))
}
