//! Bounded thread snapshots and backward keyset history pagination (`threadHistoryPaging.ts`).

use std::collections::HashSet;

use rusqlite::Connection;
use serde_json::Value;

use crate::projections::ProjectionError;
use crate::projections::wire_codec::{
    ThreadHistoryCursorPayload, decode_history_cursor, encode_history_cursor,
};
use crate::projections::wire_thread::{
    load_canonical_thread_projection, project_thread_projection_for_wire,
};
use crate::projections::wire_types::{
    WireProjectedTurnItem, WireThreadBoundedSnapshot, WireThreadHistoryPage, WireThreadProjection,
};

const DEFAULT_MAX_USER_TURNS: usize = 10;
const DEFAULT_MAX_ITEMS: usize = 75;
const DEFAULT_MAX_TURN_ITEMS: usize = 500;
const MAX_ENCODED_BYTES: usize = 1_048_576;
const MAX_RAW_TURNS: usize = 150;

/// Queries a bounded window snapshot (`OrchestrationV2ThreadBoundedSnapshot`).
pub fn query_wire_thread_bounded(
    conn: &Connection,
    thread_id: &str,
    limit: Option<usize>,
) -> Result<WireThreadBoundedSnapshot, ProjectionError> {
    let (snapshot_sequence, canonical) = load_canonical_thread_projection(conn, thread_id)?;
    let wire = project_thread_projection_for_wire(canonical);
    let max_items = limit.map_or(DEFAULT_MAX_ITEMS, |v| v.max(1));
    let max_turn_items = limit.map_or(DEFAULT_MAX_TURN_ITEMS, |v| v.max(1));

    let end = wire.visible_turn_items.len();
    let window = select_older_slice(
        &wire.visible_turn_items,
        end,
        snapshot_sequence,
        max_items,
        max_turn_items,
        |row| row_bounded_encoded_bytes(row, thread_id),
    );

    let bounded_projection = retain_projection_window(wire, &window.items, thread_id);
    let latest_local_turn_ordinal = max_local_run_ordinal(&bounded_projection.runs);

    Ok(WireThreadBoundedSnapshot {
        snapshot_sequence,
        projection: bounded_projection,
        history_cursor: window.next_cursor,
        has_more_history: window.has_more_history,
        latest_local_turn_ordinal,
    })
}

/// Queries a strictly older page of visible timeline items (`OrchestrationV2ThreadHistoryPage`).
pub fn query_wire_thread_history_page(
    conn: &Connection,
    thread_id: &str,
    before_cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<WireThreadHistoryPage, ProjectionError> {
    let (snapshot_sequence, canonical) = load_canonical_thread_projection(conn, thread_id)?;
    let wire = project_thread_projection_for_wire(canonical);
    let total = wire.visible_turn_items.len();

    let end_index = match before_cursor {
        None => total,
        Some(raw) => {
            let parsed = decode_history_cursor(raw)?;
            find_cursor_end_index(&wire.visible_turn_items, &parsed)
        }
    };

    let max_items = limit.map_or(DEFAULT_MAX_ITEMS, |v| v.max(1));
    let max_turn_items = limit.map_or(DEFAULT_MAX_TURN_ITEMS, |v| v.max(1));
    let page = select_older_slice(
        &wire.visible_turn_items,
        end_index,
        snapshot_sequence,
        max_items,
        max_turn_items,
        row_encoded_bytes,
    );

    let subagent_ids: HashSet<&str> = page
        .items
        .iter()
        .filter(|row| row.source_thread_id == thread_id)
        .filter_map(|row| {
            (row.item.get("type").and_then(Value::as_str) == Some("subagent"))
                .then(|| row.item.get("subagentId").and_then(Value::as_str))
                .flatten()
        })
        .collect();
    let subagents: Vec<Value> = if subagent_ids.is_empty() {
        Vec::new()
    } else {
        wire.subagents
            .into_iter()
            .filter(|s| {
                s.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| subagent_ids.contains(id))
            })
            .collect()
    };

    Ok(WireThreadHistoryPage {
        snapshot_sequence,
        items: page.items,
        next_cursor: page.next_cursor,
        has_more_history: page.has_more_history,
        subagents: (!subagents.is_empty()).then_some(subagents),
    })
}

struct SelectedWindow {
    items: Vec<WireProjectedTurnItem>,
    next_cursor: Option<String>,
    has_more_history: bool,
}

fn select_older_slice<F>(
    all: &[WireProjectedTurnItem],
    exclusive_end: usize,
    snapshot_sequence: i64,
    max_items: usize,
    max_turn_items: usize,
    row_cost: F,
) -> SelectedWindow
where
    F: Fn(&WireProjectedTurnItem) -> usize,
{
    let end = exclusive_end.min(all.len());
    if end == 0 {
        return SelectedWindow {
            items: Vec::new(),
            next_cursor: None,
            has_more_history: false,
        };
    }

    let candidates = &all[..end];
    let has_user_turns = candidates.iter().any(|r| is_user_authored_turn(&r.item));
    let item_cap = if has_user_turns {
        max_turn_items
    } else {
        max_items
    };
    let turn_starts = compute_turn_starts(candidates);

    let mut selected_rev = Vec::new();
    let mut encoded_bytes = 0_usize;
    let mut user_turns = 0_usize;
    let mut raw_turns = 0_usize;

    for idx in (0..end).rev() {
        let row = &all[idx];
        if !selected_rev.is_empty()
            && (selected_rev.len() >= item_cap
                || (has_user_turns
                    && (user_turns >= DEFAULT_MAX_USER_TURNS || raw_turns >= MAX_RAW_TURNS)))
        {
            break;
        }
        let cost = row_cost(row);
        if !selected_rev.is_empty() && encoded_bytes.saturating_add(cost) > MAX_ENCODED_BYTES {
            break;
        }
        selected_rev.push(row.clone());
        encoded_bytes = encoded_bytes.saturating_add(cost);
        if is_user_authored_turn(&row.item) {
            user_turns += 1;
        }
        if turn_starts[idx] {
            raw_turns += 1;
        }
    }

    if user_turns == 0 && selected_rev.len() > max_items {
        selected_rev.truncate(max_items);
    }
    selected_rev.reverse();

    let oldest_pos = end - selected_rev.len();
    let has_more_history = oldest_pos > 0;
    let next_cursor = if has_more_history && let Some(oldest) = selected_rev.first() {
        Some(encode_history_cursor(&ThreadHistoryCursorPayload {
            v: 1,
            seq: snapshot_sequence,
            st: oldest.source_thread_id.clone(),
            si: oldest.source_item_id.clone(),
            p: oldest.position,
            inclusive: None,
        }))
    } else {
        None
    };

    for (idx, row) in selected_rev.iter_mut().enumerate() {
        row.position = idx;
    }

    SelectedWindow {
        items: selected_rev,
        next_cursor,
        has_more_history,
    }
}

fn find_cursor_end_index(
    items: &[WireProjectedTurnItem],
    cursor: &ThreadHistoryCursorPayload,
) -> usize {
    if let Some(pos) = items
        .iter()
        .position(|r| r.source_thread_id == cursor.st && r.source_item_id == cursor.si)
    {
        return if cursor.inclusive == Some(true) {
            pos + 1
        } else {
            pos
        };
    }
    cursor.p.min(items.len())
}

fn retain_projection_window(
    mut p: WireThreadProjection,
    window_rows: &[WireProjectedTurnItem],
    thread_id: &str,
) -> WireThreadProjection {
    let retained_item_ids: HashSet<&str> = window_rows
        .iter()
        .filter(|r| r.source_thread_id == thread_id)
        .map(|r| r.source_item_id.as_str())
        .collect();
    let pending_req_ids: HashSet<&str> = p
        .runtime_requests
        .iter()
        .filter(|r| r.get("status").and_then(Value::as_str) == Some("pending"))
        .filter_map(|r| r.get("id").and_then(Value::as_str))
        .collect();

    p.turn_items.retain(|item| {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("");
        if retained_item_ids.contains(id) {
            return true;
        }
        let kind = item.get("type").and_then(Value::as_str);
        matches!(kind, Some("approval_request" | "user_input_request"))
            && item
                .get("requestId")
                .and_then(Value::as_str)
                .is_some_and(|req_id| pending_req_ids.contains(req_id))
    });

    p.visible_turn_items = window_rows.to_vec();
    p
}

fn max_local_run_ordinal(runs: &[Value]) -> Option<u64> {
    runs.iter()
        .filter(|r| r.get("status").and_then(Value::as_str) != Some("rolled_back"))
        .filter_map(|r| r.get("ordinal").and_then(Value::as_u64))
        .max()
}

fn row_encoded_bytes(row: &WireProjectedTurnItem) -> usize {
    serde_json::to_vec(row).map_or(256, |b| b.len())
}

fn row_bounded_encoded_bytes(row: &WireProjectedTurnItem, thread_id: &str) -> usize {
    let row_bytes = row_encoded_bytes(row);
    if row.visibility == "local" || row.source_thread_id == thread_id {
        row_bytes + serde_json::to_vec(&row.item).map_or(128, |b| b.len())
    } else {
        row_bytes
    }
}

fn is_turn_start(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("user_message")
        && matches!(
            item.get("inputIntent").and_then(Value::as_str),
            Some("turn_start" | "queued_turn")
        )
}

fn is_user_authored_turn(item: &Value) -> bool {
    is_turn_start(item) && item.get("createdBy").and_then(Value::as_str) == Some("user")
}

fn compute_turn_starts(rows: &[WireProjectedTurnItem]) -> Vec<bool> {
    let mut started_runs = HashSet::<&str>::new();
    rows.iter()
        .map(|row| {
            let rid = row.item.get("runId").and_then(Value::as_str);
            if is_turn_start(&row.item) {
                if let Some(id) = rid {
                    started_runs.insert(id);
                }
                true
            } else if matches!(
                row.item.get("type").and_then(Value::as_str),
                Some("notification" | "assistant_message")
            ) && let Some(id) = rid
                && started_runs.insert(id)
            {
                true
            } else {
                false
            }
        })
        .collect()
}
