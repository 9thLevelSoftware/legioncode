//! SQL loaders and fork-count calculation for [`crate::projections::wire_shell_rows`].

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::projections::ProjectionError;

pub(crate) struct ThreadShellContext {
    pub(crate) runs: Vec<Value>,
    pub(crate) messages: Vec<Value>,
    pub(crate) provider_threads: Vec<Value>,
    pub(crate) provider_sessions: Vec<Value>,
    pub(crate) runtime_requests: Vec<Value>,
    pub(crate) turn_items: Vec<Value>,
    pub(crate) has_active_proposed_plan: bool,
    pub(crate) visible_local_count: u64,
}

pub(crate) fn empty_context() -> ThreadShellContext {
    ThreadShellContext {
        runs: Vec::new(),
        messages: Vec::new(),
        provider_threads: Vec::new(),
        provider_sessions: Vec::new(),
        runtime_requests: Vec::new(),
        turn_items: Vec::new(),
        has_active_proposed_plan: false,
        visible_local_count: 0,
    }
}

pub(crate) fn load_raw_threads(
    conn: &Connection,
    where_clause: &str,
    only_thread_id: Option<&str>,
) -> Result<HashMap<String, Value>, ProjectionError> {
    let mut map = HashMap::new();
    let sql = if only_thread_id.is_some() {
        format!(
            "SELECT thread_id, payload_json FROM orchestration_v2_projection_threads {where_clause} AND thread_id = ?1"
        )
    } else {
        format!(
            "SELECT thread_id, payload_json FROM orchestration_v2_projection_threads {where_clause}"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(tid) = only_thread_id {
        stmt.query_map(params![tid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (tid, raw) in rows {
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| ProjectionError::Codec(format!("thread `{tid}` decode: {e}")))?;
        map.insert(tid, parsed);
    }
    Ok(map)
}

pub(crate) fn load_contexts_for_threads(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, ThreadShellContext>, ProjectionError> {
    let mut out: HashMap<String, ThreadShellContext> =
        ids.iter().map(|id| (id.clone(), empty_context())).collect();
    let ids_json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_owned());

    for (tid, v) in query_payloads(
        conn,
        "SELECT thread_id, payload_json FROM orchestration_v2_projection_runs
         WHERE thread_id IN (SELECT value FROM json_each(?1)) ORDER BY thread_id, ordinal ASC",
        &ids_json,
    )? {
        if let Some(c) = out.get_mut(&tid) {
            c.runs.push(v);
        }
    }
    for (tid, v) in query_payloads(
        conn,
        "SELECT thread_id, payload_json FROM orchestration_v2_projection_messages
         WHERE thread_id IN (SELECT value FROM json_each(?1))",
        &ids_json,
    )? {
        if let Some(c) = out.get_mut(&tid) {
            c.messages.push(v);
        }
    }
    for (tid, v) in query_payloads(
        conn,
        "SELECT thread_id, payload_json FROM orchestration_v2_projection_provider_threads
         WHERE thread_id IN (SELECT value FROM json_each(?1))",
        &ids_json,
    )? {
        if let Some(c) = out.get_mut(&tid) {
            c.provider_threads.push(v);
        }
    }
    for (tid, v) in query_payloads(
        conn,
        "SELECT binding.thread_id, session.payload_json
         FROM orchestration_v2_projection_provider_session_bindings binding
         JOIN orchestration_v2_projection_provider_sessions session
           ON session.provider_session_id = binding.provider_session_id
         WHERE binding.thread_id IN (SELECT value FROM json_each(?1))",
        &ids_json,
    )? {
        if let Some(c) = out.get_mut(&tid) {
            c.provider_sessions.push(v);
        }
    }
    for (tid, v) in query_payloads(
        conn,
        "SELECT thread_id, payload_json FROM orchestration_v2_projection_runtime_requests
         WHERE status = 'pending' AND thread_id IN (SELECT value FROM json_each(?1))",
        &ids_json,
    )? {
        if let Some(c) = out.get_mut(&tid) {
            c.runtime_requests.push(v);
        }
    }
    for (tid, v) in query_payloads(
        conn,
        "SELECT thread_id, payload_json FROM orchestration_v2_projection_turn_items
         WHERE thread_id IN (SELECT value FROM json_each(?1)) ORDER BY ordinal ASC, turn_item_id ASC",
        &ids_json,
    )? {
        if let Some(c) = out.get_mut(&tid) {
            c.turn_items.push(v);
        }
    }

    let mut plan_stmt = conn.prepare(
        "SELECT DISTINCT thread_id FROM orchestration_v2_projection_plans
         WHERE kind = 'proposed_plan' AND status = 'active'
           AND thread_id IN (SELECT value FROM json_each(?1))",
    )?;
    for row in plan_stmt.query_map(params![ids_json], |r| r.get::<_, String>(0))? {
        if let Some(c) = out.get_mut(&row?) {
            c.has_active_proposed_plan = true;
        }
    }
    for c in out.values_mut() {
        c.visible_local_count = count_visible_items(&c.runs, &c.turn_items, None);
    }
    Ok(out)
}

fn query_payloads(
    conn: &Connection,
    sql: &str,
    ids_json: &str,
) -> Result<Vec<(String, Value)>, ProjectionError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![ids_json], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for (tid, raw) in rows {
        let val: Value =
            serde_json::from_str(&raw).map_err(|e| ProjectionError::Codec(e.to_string()))?;
        out.push((tid, val));
    }
    Ok(out)
}

pub(crate) fn compute_visible_counts(
    threads: &HashMap<String, Value>,
    contexts: &HashMap<String, ThreadShellContext>,
) -> HashMap<String, u64> {
    let mut memo = HashMap::new();
    for tid in threads.keys() {
        compute_visible_count_rec(tid, threads, contexts, &mut memo, &mut HashSet::new());
    }
    memo
}

fn compute_visible_count_rec(
    tid: &str,
    threads: &HashMap<String, Value>,
    contexts: &HashMap<String, ThreadShellContext>,
    memo: &mut HashMap<String, u64>,
    visiting: &mut HashSet<String>,
) -> u64 {
    if let Some(&cached) = memo.get(tid) {
        return cached;
    }
    if !visiting.insert(tid.to_owned()) {
        return 0;
    }
    let local = contexts.get(tid).map_or(0, |c| c.visible_local_count);
    let total = if let Some(t) = threads.get(tid)
        && let Some((src_tid, src_run_id)) = fork_source_pair(t)
        && let Some(src_ctx) = contexts.get(&src_tid)
        && let Some(src_run_ord) = src_ctx
            .runs
            .iter()
            .find(|r| str_at(r, "id") == Some(src_run_id.as_str()))
            .map(|r| u64_at(r, "ordinal"))
    {
        let prefix = compute_visible_count_rec(&src_tid, threads, contexts, memo, visiting);
        let src_prefix = count_visible_items(&src_ctx.runs, &src_ctx.turn_items, Some(src_run_ord));
        let full_src = src_ctx.visible_local_count;
        let base = prefix.saturating_sub(full_src).saturating_add(src_prefix);
        base.saturating_add(1).saturating_add(local)
    } else {
        local
    };
    visiting.remove(tid);
    memo.insert(tid.to_owned(), total);
    total
}

fn count_visible_items(runs: &[Value], items: &[Value], max_run_ord: Option<u64>) -> u64 {
    let mut run_status = HashMap::<&str, (&str, u64)>::new();
    for r in runs {
        if let (Some(id), Some(st)) = (str_at(r, "id"), str_at(r, "status")) {
            run_status.insert(id, (st, u64_at(r, "ordinal")));
        }
    }
    items
        .iter()
        .filter(|item| {
            let rid = str_at(item, "runId");
            if let Some(limit_ord) = max_run_ord {
                let ord = rid.and_then(|id| run_status.get(id).map(|v| v.1));
                if ord.is_none_or(|o| o > limit_ord) {
                    return false;
                }
            } else if let Some(id) = rid
                && run_status
                    .get(id)
                    .is_some_and(|(st, _)| *st == "rolled_back")
            {
                return false;
            }
            if str_at(item, "type") == Some("user_message")
                && str_at(item, "inputIntent") == Some("queued_turn")
                && let Some(id) = rid
                && run_status
                    .get(id)
                    .is_some_and(|(st, _)| matches!(*st, "queued" | "cancelled"))
            {
                return false;
            }
            true
        })
        .count() as u64
}

pub(crate) fn fork_source_thread_id(t: &Value) -> Option<String> {
    fork_source_pair(t).map(|(tid, _)| tid)
}

fn fork_source_pair(t: &Value) -> Option<(String, String)> {
    let side_chat = t
        .get("lineage")
        .and_then(|l| l.get("sideChat"))
        .and_then(Value::as_bool)
        == Some(true);
    if side_chat {
        return None;
    }
    let forked = t.get("forkedFrom")?;
    if str_at(forked, "type") != Some("run") {
        return None;
    }
    Some((
        str_at(forked, "threadId")?.to_owned(),
        str_at(forked, "runId")?.to_owned(),
    ))
}

pub(crate) fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

pub(crate) fn opt_str_at(v: &Value, k: &str) -> Option<String> {
    str_at(v, k).map(str::to_owned)
}

pub(crate) fn bool_at(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(Value::as_bool)
}

pub(crate) fn u64_at(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn extract_pending_tasks(ctx: &ThreadShellContext) -> Vec<Value> {
    let mut out = Vec::new();
    for pt in &ctx.provider_threads {
        if let Some(tasks) = pt.get("pendingBackgroundTasks").and_then(Value::as_array) {
            out.extend(tasks.iter().cloned());
        }
    }
    out
}

pub(crate) fn provider_instance_history(tid: &str, provider_threads: &[Value]) -> Vec<String> {
    let mut matching: Vec<&Value> = provider_threads
        .iter()
        .filter(|pt| {
            str_at(pt, "appThreadId") == Some(tid)
                && pt.get("ownerNodeId").is_none_or(Value::is_null)
        })
        .collect();
    matching.sort_by(|a, b| {
        str_at(a, "createdAt")
            .unwrap_or("")
            .cmp(str_at(b, "createdAt").unwrap_or(""))
            .then_with(|| {
                str_at(a, "id")
                    .unwrap_or("")
                    .cmp(str_at(b, "id").unwrap_or(""))
            })
    });
    let mut history = Vec::new();
    for pt in matching {
        if let Some(inst) = str_at(pt, "providerInstanceId")
            && !history.iter().any(|x| x == inst)
        {
            history.push(inst.to_owned());
        }
    }
    history
}

pub(crate) fn active_provider_goal(t: &Value, provider_threads: &[Value]) -> Option<Value> {
    let active_pt = str_at(t, "activeProviderThreadId")?;
    provider_threads
        .iter()
        .find(|pt| str_at(pt, "id") == Some(active_pt))
        .and_then(|pt| pt.get("goal"))
        .cloned()
        .filter(|v| !v.is_null())
}

pub(crate) fn derive_blocked_on_thread_wait(
    active_run: Option<&Value>,
    turn_items: &[Value],
) -> bool {
    let Some(run) = active_run else {
        return false;
    };
    if str_at(run, "status") != Some("running") {
        return false;
    }
    let Some(run_id) = str_at(run, "id") else {
        return false;
    };
    let mut has_wait = false;
    for item in turn_items {
        if str_at(item, "runId") != Some(run_id) {
            continue;
        }
        let Some(kind) = str_at(item, "type") else {
            continue;
        };
        if !matches!(
            kind,
            "command_execution"
                | "dynamic_tool"
                | "subagent"
                | "file_change"
                | "file_search"
                | "web_search"
                | "compaction"
        ) || !matches!(str_at(item, "status"), Some("running" | "waiting"))
            || (kind == "subagent" && str_at(item, "origin") == Some("app_owned"))
        {
            continue;
        }
        if kind != "dynamic_tool" {
            return false;
        }
        let tool = str_at(item, "toolName").unwrap_or("");
        let base = tool.rsplit("__").next().unwrap_or(tool);
        let is_wait = matches!(base, "legion_thread_wait" | "t3_thread_wait")
            || (base == "delegate_task"
                && item.get("input").and_then(|i| str_at(i, "mode")) == Some("wait"));
        if !is_wait {
            return false;
        }
        has_wait = true;
    }
    has_wait
}
