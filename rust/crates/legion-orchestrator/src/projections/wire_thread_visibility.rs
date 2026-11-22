//! Turn-item visibility rules and inherited fork timeline synthesis (`ProjectionStore.ts` + `orchestrationV2Timeline.ts`).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::projections::wire_types::{WireProjectedTurnItem, WireThreadProjection};
use crate::time::parse_iso_millis;

pub(crate) fn sort_messages_by_turn_item_order(
    messages: Vec<Value>,
    turn_items: &[Value],
) -> Vec<Value> {
    let mut ordinals: HashMap<String, i64> = HashMap::new();
    for item in turn_items {
        let kind = item.get("type").and_then(Value::as_str);
        if matches!(kind, Some("user_message" | "assistant_message"))
            && let Some(mid) = item.get("messageId").and_then(Value::as_str)
        {
            let ord = item
                .get("ordinal")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX);
            ordinals
                .entry(mid.to_owned())
                .and_modify(|cur| *cur = (*cur).min(ord))
                .or_insert(ord);
        }
    }
    let mut sorted = messages;
    sorted.sort_by(|left, right| {
        let l_id = left.get("id").and_then(Value::as_str).unwrap_or("");
        let r_id = right.get("id").and_then(Value::as_str).unwrap_or("");
        let l_ord = ordinals.get(l_id).copied().unwrap_or(i64::MAX);
        let r_ord = ordinals.get(r_id).copied().unwrap_or(i64::MAX);
        l_ord
            .cmp(&r_ord)
            .then_with(|| {
                let l_ts = left
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_millis);
                let r_ts = right
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_millis);
                l_ts.cmp(&r_ts)
            })
            .then_with(|| l_id.cmp(r_id))
    });
    sorted
}

pub(crate) fn inherited_run_fork(thread_val: &Value) -> Option<(String, String)> {
    let side_chat = thread_val
        .get("lineage")
        .and_then(|l| l.get("sideChat"))
        .and_then(Value::as_bool)
        == Some(true);
    if side_chat {
        return None;
    }
    let forked = thread_val.get("forkedFrom")?;
    if forked.get("type").and_then(Value::as_str) != Some("run") {
        return None;
    }
    let tid = forked.get("threadId")?.as_str()?.to_owned();
    let rid = forked.get("runId")?.as_str()?.to_owned();
    Some((tid, rid))
}

pub(crate) fn build_visible_turn_items(
    thread_val: &Value,
    runs: &[Value],
    attempts: &[Value],
    turn_items: &[Value],
    source: Option<&WireThreadProjection>,
) -> Vec<WireProjectedTurnItem> {
    let tid = thread_val.get("id").and_then(Value::as_str).unwrap_or("");
    let local: Vec<WireProjectedTurnItem> = filter_visible_items(runs, attempts, turn_items)
        .into_iter()
        .enumerate()
        .map(|(position, item)| WireProjectedTurnItem {
            position,
            visibility: "local".to_owned(),
            source_thread_id: tid.to_owned(),
            source_item_id: item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            item,
        })
        .collect();

    let Some((src_tid, src_run_id)) = inherited_run_fork(thread_val) else {
        return local;
    };
    let Some(src) = source else {
        return local;
    };

    let fork_created_ms = thread_val
        .get("createdAt")
        .and_then(Value::as_str)
        .and_then(parse_iso_millis)
        .unwrap_or(i64::MAX);
    let inherited = visible_items_through_run(src, &src_run_id, fork_created_ms);
    let marker = make_fork_marker_item(thread_val, &src_tid, &src_run_id);
    let marker_id = marker
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    let mut combined = Vec::with_capacity(inherited.len() + 1 + local.len());
    combined.extend(inherited);
    combined.push(WireProjectedTurnItem {
        position: 0,
        visibility: "synthetic".to_owned(),
        source_thread_id: src_tid,
        source_item_id: marker_id,
        item: marker,
    });
    combined.extend(local);
    for (idx, row) in combined.iter_mut().enumerate() {
        row.position = idx;
    }
    combined
}

fn visible_items_through_run(
    src: &WireThreadProjection,
    source_run_id: &str,
    fork_created_ms: i64,
) -> Vec<WireProjectedTurnItem> {
    let Some(source_run) = src
        .runs
        .iter()
        .find(|r| r.get("id").and_then(Value::as_str) == Some(source_run_id))
    else {
        return Vec::new();
    };
    let source_run_ord = source_run
        .get("ordinal")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let src_tid = src.thread.get("id").and_then(Value::as_str).unwrap_or("");
    let v1_import = src.thread.get("historyOrigin").and_then(Value::as_str) == Some("v1_import");

    let mut run_ord_by_id: HashMap<&str, u64> = HashMap::new();
    let cancelled_runs: HashSet<&str> = src
        .attempts
        .iter()
        .filter(|a| a.get("status").and_then(Value::as_str) == Some("cancelled"))
        .filter_map(|a| a.get("runId").and_then(Value::as_str))
        .collect();
    let mut adjusted_runs = Vec::with_capacity(src.runs.len());
    for r in &src.runs {
        let status = r.get("status").and_then(Value::as_str);
        let rolled_back_at_ms = if status == Some("rolled_back") {
            r.get("completedAt")
                .and_then(Value::as_str)
                .and_then(parse_iso_millis)
        } else {
            None
        };
        if rolled_back_at_ms.is_none_or(|ms| ms >= fork_created_ms)
            && let (Some(id), Some(ord)) = (
                r.get("id").and_then(Value::as_str),
                r.get("ordinal").and_then(Value::as_u64),
            )
        {
            run_ord_by_id.insert(id, ord);
        }
        if status != Some("rolled_back") {
            adjusted_runs.push(r.clone());
        } else if let Some(id) = r.get("id").and_then(Value::as_str)
            && cancelled_runs.contains(id)
        {
            let mut copy = r.clone();
            if let Some(obj) = copy.as_object_mut() {
                obj.insert("status".to_owned(), Value::String("cancelled".to_owned()));
            }
            adjusted_runs.push(copy);
        }
    }

    let mut out: Vec<WireProjectedTurnItem> = src
        .visible_turn_items
        .iter()
        .filter(|row| {
            row.source_thread_id != src_tid
                || row.item.get("type").and_then(Value::as_str) == Some("fork")
        })
        .cloned()
        .map(|mut row| {
            "inherited".clone_into(&mut row.visibility);
            row
        })
        .collect();

    for item in filter_visible_items(&adjusted_runs, &src.attempts, &src.turn_items) {
        let item_run_id = item.get("runId").and_then(Value::as_str);
        let include = match item_run_id {
            None => v1_import,
            Some(rid) => run_ord_by_id
                .get(rid)
                .is_some_and(|ord| *ord <= source_run_ord),
        };
        if include {
            let sid = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            out.push(WireProjectedTurnItem {
                position: 0,
                visibility: "inherited".to_owned(),
                source_thread_id: src_tid.to_owned(),
                source_item_id: sid,
                item,
            });
        }
    }
    out
}

fn filter_visible_items(runs: &[Value], attempts: &[Value], items: &[Value]) -> Vec<Value> {
    let mut statuses: HashMap<&str, &str> = HashMap::new();
    for r in runs {
        if let (Some(id), Some(st)) = (
            r.get("id").and_then(Value::as_str),
            r.get("status").and_then(Value::as_str),
        ) {
            statuses.insert(id, st);
        }
    }
    let mut superseded_roots: HashMap<&str, HashSet<&str>> = HashMap::new();
    for a in attempts {
        if a.get("status").and_then(Value::as_str) == Some("superseded")
            && let (Some(rid), Some(root)) = (
                a.get("runId").and_then(Value::as_str),
                a.get("rootNodeId").and_then(Value::as_str),
            )
        {
            superseded_roots.entry(rid).or_default().insert(root);
        }
    }
    let interrupt_runs: HashSet<&str> = items
        .iter()
        .filter(|i| i.get("type").and_then(Value::as_str) == Some("run_interrupt_request"))
        .filter_map(|i| i.get("runId").and_then(Value::as_str))
        .collect();

    items
        .iter()
        .filter(|item| {
            let rid = item.get("runId").and_then(Value::as_str);
            let run_st = rid.and_then(|id| statuses.get(id).copied());
            if run_st == Some("rolled_back") {
                return false;
            }
            if matches!(run_st, Some("queued" | "cancelled"))
                && item.get("type").and_then(Value::as_str) == Some("user_message")
                && item.get("inputIntent").and_then(Value::as_str) == Some("queued_turn")
            {
                return false;
            }
            if item.get("type").and_then(Value::as_str) == Some("run_interrupt_result")
                && let (Some(r), Some(nid)) = (rid, item.get("nodeId").and_then(Value::as_str))
                && superseded_roots
                    .get(r)
                    .is_some_and(|roots| roots.contains(nid))
                && !interrupt_runs.contains(r)
            {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

fn make_fork_marker_item(thread_val: &Value, source_thread_id: &str, source_run_id: &str) -> Value {
    let tid = thread_val.get("id").and_then(Value::as_str).unwrap_or("");
    let created_at = thread_val
        .get("createdAt")
        .and_then(Value::as_str)
        .unwrap_or("");
    serde_json::json!({
        "id": format!("turn-item:fork:{tid}"),
        "threadId": tid,
        "runId": null,
        "nodeId": null,
        "providerTurnId": null,
        "nativeItemRef": null,
        "parentItemId": null,
        "ordinal": 0,
        "status": "completed",
        "title": "Forked from conversation",
        "startedAt": null,
        "completedAt": created_at,
        "updatedAt": created_at,
        "type": "fork",
        "source": {
            "type": "run",
            "threadId": source_thread_id,
            "runId": source_run_id,
        },
        "targetThreadId": tid,
    })
}
