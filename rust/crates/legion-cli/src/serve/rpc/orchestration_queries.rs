//! Thread family, metadata batch lookups, worktree queries, and in-thread search handlers.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use legion_orchestrator::projections::{
    WireShellOptions, query_wire_shell_http_snapshot_with_options, query_wire_thread_detail,
};
use legion_protocol::rpc_codec::RpcExit;
use rusqlite::{OptionalExtension as _, params};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::orchestration::{ThreadIdPayload, shell_err};
use super::orchestration_search::search_err;
use super::{AppState, parse_payload, rpc_fail_obj, with_db};

pub(crate) fn get_thread_family(state: &AppState, payload: Value) -> RpcExit {
    let req: ThreadIdPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let env_id = state.descriptor.environment_id.as_str();
    with_db(state, |conn| {
        let root_id: Option<String> = conn
            .query_row(
                "SELECT COALESCE(json_extract(payload_json, '$.lineage.rootThreadId'), thread_id)
                 FROM orchestration_v2_projection_threads
                 WHERE thread_id = ?1",
                params![req.thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|err| shell_err(&err.to_string()))?;
        let full = query_wire_shell_http_snapshot_with_options(
            conn,
            env_id,
            WireShellOptions {
                omit_idle_subagents: false,
                omit_older_settled: false,
            },
        )
        .map_err(|err| shell_err(&err.to_string()))?;
        let Some(root_thread_id) = root_id else {
            return Ok(RpcExit::Success {
                value: json!({
                    "snapshotSequence": full.snapshot_sequence,
                    "threads": [],
                }),
            });
        };
        let family: Vec<_> = full
            .threads
            .into_iter()
            .filter(|t| {
                t.id == root_thread_id
                    || t.lineage.get("rootThreadId").and_then(Value::as_str)
                        == Some(root_thread_id.as_str())
            })
            .collect();
        Ok(RpcExit::Success {
            value: json!({
                "snapshotSequence": full.snapshot_sequence,
                "threads": family,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadIdsPayload {
    thread_ids: Vec<String>,
}

pub(crate) fn get_thread_titles(state: &AppState, payload: Value) -> RpcExit {
    let req: ThreadIdsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        let mut out = Map::new();
        let wanted: HashSet<&str> = req.thread_ids.iter().map(String::as_str).collect();
        if wanted.is_empty() {
            return Ok(RpcExit::Success {
                value: Value::Object(out),
            });
        }
        let ids_json = serde_json::to_string(&req.thread_ids).unwrap_or_else(|_| "[]".to_owned());
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, title FROM orchestration_v2_projection_threads
                 WHERE deleted_at IS NULL AND thread_id IN (SELECT value FROM json_each(?1))",
            )
            .map_err(|err| shell_err(&err.to_string()))?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![ids_json], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .and_then(Iterator::collect)
            .map_err(|err| shell_err(&err.to_string()))?;
        for (tid, title) in rows {
            if wanted.contains(tid.as_str()) {
                out.insert(tid, Value::String(title));
            }
        }
        Ok(RpcExit::Success {
            value: Value::Object(out),
        })
    })
    .unwrap_or_else(|e| e)
}

pub(crate) fn get_thread_link_metadata(state: &AppState, payload: Value) -> RpcExit {
    let req: ThreadIdsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    with_db(state, |conn| {
        let mut out = Map::new();
        if req.thread_ids.is_empty() {
            return Ok(RpcExit::Success {
                value: Value::Object(out),
            });
        }
        let ids_json = serde_json::to_string(&req.thread_ids).unwrap_or_else(|_| "[]".to_owned());
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, title, project_id FROM orchestration_v2_projection_threads
                 WHERE deleted_at IS NULL AND thread_id IN (SELECT value FROM json_each(?1))",
            )
            .map_err(|err| shell_err(&err.to_string()))?;
        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map(params![ids_json], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .and_then(Iterator::collect)
            .map_err(|err| shell_err(&err.to_string()))?;
        for (tid, title, project_id) in rows {
            out.insert(tid, json!({ "title": title, "projectId": project_id }));
        }
        Ok(RpcExit::Success {
            value: Value::Object(out),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeThreadIdsPayload {
    worktree_path: String,
}

pub(crate) fn get_worktree_thread_ids(state: &AppState, payload: Value) -> RpcExit {
    let req: WorktreeThreadIdsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let normalized = normalize_path_key(&req.worktree_path);
    with_db(state, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, json_extract(payload_json, '$.worktreePath')
                 FROM orchestration_v2_projection_threads
                 WHERE deleted_at IS NULL",
            )
            .map_err(|err| shell_err(&err.to_string()))?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .and_then(Iterator::collect)
            .map_err(|err| shell_err(&err.to_string()))?;
        let matching: Vec<String> = rows
            .into_iter()
            .filter_map(|(tid, wt)| {
                wt.filter(|path| normalize_path_key(path) == normalized)
                    .map(|_| tid)
            })
            .collect();
        Ok(RpcExit::Success {
            value: json!(matching),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchThreadPayload {
    thread_id: String,
    query: String,
    index: Option<usize>,
    offset: Option<i64>,
}

pub(crate) fn search_thread(state: &AppState, payload: Value) -> RpcExit {
    let req: SearchThreadPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let needle = req.query.trim().to_ascii_lowercase();
    with_db(state, |conn| {
        let detail = query_wire_thread_detail(conn, &req.thread_id).map_err(|err| {
            rpc_fail_obj(json!({
                "_tag": "OrchestrationV2SearchThreadError",
                "cause": err.to_string(),
            }))
        })?;
        let mut hits: Vec<(String, Option<String>, usize)> = Vec::new();
        if !needle.is_empty() {
            let mut msg_stmt = conn
                .prepare(
                    "SELECT message_id, COALESCE(json_extract(payload_json, '$.text'), '')
                     FROM orchestration_v2_projection_messages
                     WHERE thread_id = ?1
                     ORDER BY rowid ASC",
                )
                .map_err(|e| search_err(&e.to_string()))?;
            let msg_rows: Vec<(String, String)> = msg_stmt
                .query_map(params![req.thread_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .and_then(Iterator::collect)
                .unwrap_or_default();
            for (msg_id, text) in msg_rows {
                collect_needle_hits(
                    &text.to_ascii_lowercase(),
                    &needle,
                    &msg_id,
                    None,
                    &mut hits,
                );
            }
            for row in &detail.projection.visible_turn_items {
                let entry_id = row
                    .item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&row.source_item_id);
                let run_id = row
                    .item
                    .get("runId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let hay = collect_item_searchable_text(&row.item).to_ascii_lowercase();
                collect_needle_hits(&hay, &needle, entry_id, run_id.as_deref(), &mut hits);
            }
        }
        let total = hits.len();
        let active_index = if total == 0 {
            0
        } else {
            let base = i64::try_from(req.index.unwrap_or(0)).unwrap_or(0) + req.offset.unwrap_or(0);
            let total_i64 = i64::try_from(total).unwrap_or(1);
            usize::try_from(base.rem_euclid(total_i64)).unwrap_or(0)
        };
        let active_match = hits.get(active_index).map(|(entry_id, run_id, occ)| {
            json!({
                "entryId": entry_id,
                "runId": run_id,
                "occurrence": occ,
            })
        });
        Ok(RpcExit::Success {
            value: json!({
                "complete": true,
                "snapshotSequence": detail.snapshot_sequence,
                "totalMatches": total,
                "activeIndex": active_index,
                "match": active_match,
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

fn collect_item_searchable_text(item: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["text", "title", "detail", "output", "markdown"] {
        if let Some(s) = item.get(key).and_then(Value::as_str) {
            parts.push(s);
        }
    }
    parts.join("\n")
}

fn collect_needle_hits(
    hay_lower: &str,
    needle: &str,
    entry_id: &str,
    run_id: Option<&str>,
    hits: &mut Vec<(String, Option<String>, usize)>,
) {
    let mut occurrence = 0_usize;
    let mut cursor = 0_usize;
    while let Some(found) = hay_lower[cursor..].find(needle) {
        hits.push((entry_id.to_owned(), run_id.map(str::to_owned), occurrence));
        occurrence += 1;
        cursor += found + needle.len();
    }
}

fn normalize_path_key(raw: &str) -> String {
    let clean: PathBuf = Path::new(raw.trim())
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    let s = clean.to_string_lossy().replace('\\', "/");
    let trimmed = s.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_owned()
    }
}
