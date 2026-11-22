//! Cross-thread FTS5 trigram search (`searchThreads`).

use std::collections::{HashMap, HashSet};

use legion_orchestrator::projections::{
    WireShellOptions, query_wire_shell_http_snapshot_with_options,
};
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{AppState, parse_payload, rpc_fail_obj, with_db};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchThreadsPayload {
    query: String,
    limit: Option<usize>,
    older_settled_before: Option<String>,
    project_ids: Option<Vec<String>>,
    titles_only: Option<bool>,
}

pub(crate) fn search_threads(state: &AppState, payload: Value) -> RpcExit {
    let req: SearchThreadsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let query = req.query.trim();
    if query.chars().count() < 2 {
        return rpc_fail_obj(json!({
            "_tag": "OrchestrationSearchThreadsError",
            "message": "Search query must be at least 2 characters.",
        }));
    }
    let limit = req.limit.unwrap_or(30).clamp(1, 50);
    let env_id = state.descriptor.environment_id.as_str();
    with_db(state, |conn| {
        let matches = if req.titles_only.unwrap_or(false) {
            Vec::new()
        } else {
            query_message_matches(conn, query, req.project_ids.as_deref(), limit)?
        };
        let mut resp = json!({ "matches": matches });
        if let Some(before) = req.older_settled_before.as_deref() {
            let full = query_wire_shell_http_snapshot_with_options(
                conn,
                env_id,
                WireShellOptions {
                    omit_idle_subagents: false,
                    omit_older_settled: false,
                },
            )
            .map_err(|e| search_err(&e.to_string()))?;
            let needle = query.to_ascii_lowercase();
            let allowed_proj: Option<HashSet<&str>> = req
                .project_ids
                .as_ref()
                .map(|list| list.iter().map(String::as_str).collect());
            let rows: Vec<Value> = full
                .threads
                .into_iter()
                .filter(|t| t.updated_at.as_str() < before)
                .filter(|t| {
                    allowed_proj
                        .as_ref()
                        .is_none_or(|set| set.contains(t.project_id.as_str()))
                })
                .filter(|t| {
                    t.title.to_ascii_lowercase().contains(&needle)
                        || t.branch
                            .as_deref()
                            .is_some_and(|b| b.to_ascii_lowercase().contains(&needle))
                })
                .take(limit)
                .map(|t| json!(t))
                .collect();
            if let Some(obj) = resp.as_object_mut() {
                obj.insert("snapshotSequence".to_owned(), json!(full.snapshot_sequence));
                obj.insert("threads".to_owned(), json!(rows));
            }
        }
        Ok(RpcExit::Success { value: resp })
    })
    .unwrap_or_else(|e| e)
}

type MatchRow = (String, String, String, String, Option<String>, String);

fn query_message_matches(
    conn: &Connection,
    query: &str,
    project_ids: Option<&[String]>,
    limit: usize,
) -> Result<Vec<Value>, RpcExit> {
    let fts_query = build_fts_phrase_query(query);
    let mut rows: Vec<MatchRow> = Vec::new();
    let proj_json =
        project_ids.map(|p| serde_json::to_string(p).unwrap_or_else(|_| "[]".to_owned()));

    if fts_table_exists(conn)
        && let Some(match_expr) = fts_query.as_deref()
    {
        let mut stmt = conn
            .prepare(
                "SELECT m.message_id, m.thread_id, t.project_id, m.role,
                        COALESCE(json_extract(m.payload_json, '$.createdAt'), m.updated_at),
                        COALESCE(json_extract(m.payload_json, '$.text'), '')
                 FROM orchestration_v2_message_search AS idx
                 JOIN orchestration_v2_projection_messages AS m ON m.rowid = idx.rowid
                 JOIN orchestration_v2_projection_threads AS t ON t.thread_id = m.thread_id
                 WHERE idx.text MATCH ?1
                   AND t.deleted_at IS NULL
                   AND (?2 IS NULL OR t.project_id IN (SELECT value FROM json_each(?2)))
                   AND m.role IN ('user', 'assistant')
                 ORDER BY m.updated_at DESC, m.message_id DESC
                 LIMIT 200",
            )
            .map_err(|e| search_err(&e.to_string()))?;
        rows = stmt
            .query_map(params![match_expr, proj_json], map_match_row)
            .and_then(Iterator::collect)
            .map_err(|e| search_err(&e.to_string()))?;
    }

    if rows.is_empty() {
        let lower_pat = query.to_ascii_lowercase();
        let mut stmt = conn
            .prepare(
                "SELECT m.message_id, m.thread_id, t.project_id, m.role,
                        COALESCE(json_extract(m.payload_json, '$.createdAt'), m.updated_at),
                        COALESCE(json_extract(m.payload_json, '$.text'), '')
                 FROM orchestration_v2_projection_messages AS m
                 JOIN orchestration_v2_projection_threads AS t ON t.thread_id = m.thread_id
                 WHERE t.deleted_at IS NULL
                   AND (?1 IS NULL OR t.project_id IN (SELECT value FROM json_each(?1)))
                   AND m.role IN ('user', 'assistant')
                   AND instr(lower(COALESCE(json_extract(m.payload_json, '$.text'), '')), ?2) > 0
                 ORDER BY m.updated_at DESC, m.message_id DESC
                 LIMIT 200",
            )
            .map_err(|e| search_err(&e.to_string()))?;
        rows = stmt
            .query_map(params![proj_json, lower_pat], map_match_row)
            .and_then(Iterator::collect)
            .map_err(|e| search_err(&e.to_string()))?;
    }

    let query_lower = query.to_ascii_lowercase();
    let mut seen_threads = HashMap::<String, usize>::new();
    let mut out = Vec::new();
    for (msg_id, tid, pid, role, created_at, text) in rows {
        let text_lower = text.to_ascii_lowercase();
        let Some(pos) = text_lower.find(&query_lower) else {
            continue;
        };
        let count = seen_threads.entry(tid.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            continue;
        }
        out.push(json!({
            "threadId": tid,
            "projectId": pid,
            "source": if role == "assistant" { "assistant" } else { "user" },
            "snippet": build_snippet(&text, pos),
            "messageCreatedAt": created_at,
            "quality": 1.0,
            "hits": 1,
            "messageId": msg_id,
        }));
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn map_match_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MatchRow> {
    Ok((
        r.get::<_, String>(0)?,
        r.get::<_, String>(1)?,
        r.get::<_, String>(2)?,
        r.get::<_, String>(3)?,
        r.get::<_, Option<String>>(4)?,
        r.get::<_, String>(5)?,
    ))
}

pub(crate) fn search_err(msg: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "OrchestrationSearchThreadsError",
        "message": msg,
    }))
}

fn fts_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='orchestration_v2_message_search'",
        [],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn build_fts_phrase_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().count() >= 3)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

fn build_snippet(text: &str, byte_match_start: usize) -> String {
    let safe_start = floor_char_boundary(text, byte_match_start.saturating_sub(24));
    let safe_end = ceil_char_boundary(text, (safe_start + 240).min(text.len()));
    let mut raw = text[safe_start..safe_end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if safe_start > 0 {
        raw = format!("…{raw}");
    }
    if safe_end < text.len() {
        raw.push('…');
    }
    raw.chars().take(240).collect()
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}
