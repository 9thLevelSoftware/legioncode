//! Canonical thread wire projection builder matching `ProjectionStore.readCanonicalProjection`.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::projections::ProjectionError;
use crate::projections::wire_codec::{
    cap_checkpoint_files_value, normalize_app_thread_wire, normalize_model_selection_wire,
    project_context_handoff_for_wire, project_turn_item_for_wire,
};
use crate::projections::wire_thread_visibility::{
    build_visible_turn_items, inherited_run_fork, sort_messages_by_turn_item_order,
};
use crate::projections::wire_types::{WireThreadDetailSnapshot, WireThreadProjection};

/// Queries full wire thread detail (`OrchestrationV2ThreadDetailSnapshot`).
pub fn query_wire_thread_detail(
    conn: &Connection,
    thread_id: &str,
) -> Result<WireThreadDetailSnapshot, ProjectionError> {
    let (snapshot_sequence, canonical) = load_canonical_thread_projection(conn, thread_id)?;
    Ok(WireThreadDetailSnapshot {
        snapshot_sequence,
        projection: project_thread_projection_for_wire(canonical),
        history_cursor: None,
        has_more_history: None,
        latest_local_turn_ordinal: None,
    })
}

/// Applies `projectThreadProjectionForWire` (`WireProjection.ts`) to a canonical projection.
pub(crate) fn project_thread_projection_for_wire(
    mut p: WireThreadProjection,
) -> WireThreadProjection {
    p.checkpoints = p
        .checkpoints
        .into_iter()
        .map(cap_checkpoint_files_value)
        .collect();
    p.context_handoffs = p
        .context_handoffs
        .into_iter()
        .map(project_context_handoff_for_wire)
        .collect();
    p.turn_items = p
        .turn_items
        .into_iter()
        .map(project_turn_item_for_wire)
        .collect();
    p.visible_turn_items = p
        .visible_turn_items
        .into_iter()
        .map(|mut row| {
            row.item = project_turn_item_for_wire(row.item);
            row
        })
        .collect();
    p
}

/// Loads full canonical [`WireThreadProjection`] and thread-stream sequence (including fork ancestry).
pub(crate) fn load_canonical_thread_projection(
    conn: &Connection,
    thread_id: &str,
) -> Result<(i64, WireThreadProjection), ProjectionError> {
    let mut visited = HashSet::new();
    let canonical = load_projection_recursive(conn, thread_id, &mut visited)?;
    let snapshot_sequence: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0)
         FROM orchestration_events
         WHERE application_event_version = 2
           AND aggregate_kind = 'thread'
           AND stream_id = ?1",
        params![thread_id],
        |r| r.get(0),
    )?;
    Ok((snapshot_sequence, canonical))
}

fn load_projection_recursive(
    conn: &Connection,
    thread_id: &str,
    visited: &mut HashSet<String>,
) -> Result<WireThreadProjection, ProjectionError> {
    if !visited.insert(thread_id.to_owned()) {
        return Err(ProjectionError::Codec(format!(
            "circular fork lineage detected at thread `{thread_id}`"
        )));
    }
    let raw_thread: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id = ?1 LIMIT 1",
            params![thread_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw_thread_json) = raw_thread else {
        return Err(ProjectionError::ThreadNotFound(thread_id.to_owned()));
    };
    let thread_val = normalize_app_thread_wire(
        serde_json::from_str(&raw_thread_json)
            .map_err(|e| ProjectionError::Codec(format!("thread `{thread_id}` json: {e}")))?,
    );
    let mut rows = collect_thread_rows(conn, thread_id)?;
    let messages = sort_messages_by_turn_item_order(rows.raw_messages, &rows.turn_items);
    let updated_at = thread_val
        .get("updatedAt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let source_projection = if let Some((src_tid, _)) = inherited_run_fork(&thread_val) {
        load_projection_recursive(conn, &src_tid, visited).ok()
    } else {
        None
    };
    let visible_turn_items = build_visible_turn_items(
        &thread_val,
        &rows.runs,
        &rows.attempts,
        &rows.turn_items,
        source_projection.as_ref(),
    );

    Ok(WireThreadProjection {
        thread: thread_val,
        runs: std::mem::take(&mut rows.runs),
        attempts: rows.attempts,
        nodes: rows.nodes,
        subagents: rows.subagents,
        provider_sessions: rows.provider_sessions,
        provider_threads: rows.provider_threads,
        provider_turns: rows.provider_turns,
        runtime_requests: rows.runtime_requests,
        messages,
        plans: rows.plans,
        turn_items: rows.turn_items,
        checkpoint_scopes: rows.checkpoint_scopes,
        checkpoints: rows.checkpoints,
        context_handoffs: rows.context_handoffs,
        context_transfers: rows.context_transfers,
        visible_turn_items,
        updated_at,
    })
}

struct ThreadProjectionRows {
    runs: Vec<Value>,
    attempts: Vec<Value>,
    nodes: Vec<Value>,
    subagents: Vec<Value>,
    provider_sessions: Vec<Value>,
    provider_threads: Vec<Value>,
    provider_turns: Vec<Value>,
    runtime_requests: Vec<Value>,
    raw_messages: Vec<Value>,
    plans: Vec<Value>,
    turn_items: Vec<Value>,
    checkpoint_scopes: Vec<Value>,
    checkpoints: Vec<Value>,
    context_handoffs: Vec<Value>,
    context_transfers: Vec<Value>,
}

fn collect_thread_rows(
    conn: &Connection,
    thread_id: &str,
) -> Result<ThreadProjectionRows, ProjectionError> {
    let mut runs = query_json_rows(
        conn,
        "SELECT payload_json FROM orchestration_v2_projection_runs WHERE thread_id = ?1 ORDER BY ordinal ASC, run_id ASC",
        thread_id,
    )?;
    for run in &mut runs {
        if let Some(obj) = run.as_object_mut()
            && let Some(ms) = obj.remove("modelSelection")
        {
            obj.insert(
                "modelSelection".to_owned(),
                normalize_model_selection_wire(ms),
            );
        }
    }
    Ok(ThreadProjectionRows {
        runs,
        attempts: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_run_attempts WHERE thread_id = ?1 ORDER BY attempt_ordinal ASC, attempt_id ASC",
            thread_id,
        )?,
        nodes: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_nodes WHERE thread_id = ?1 ORDER BY COALESCE(started_at, ''), node_id ASC",
            thread_id,
        )?,
        subagents: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_subagents WHERE thread_id = ?1 ORDER BY COALESCE(started_at, ''), subagent_id ASC",
            thread_id,
        )?,
        provider_sessions: query_json_rows(
            conn,
            "SELECT s.payload_json FROM orchestration_v2_projection_provider_session_bindings b
             JOIN orchestration_v2_projection_provider_sessions s ON s.provider_session_id = b.provider_session_id
             WHERE b.thread_id = ?1 ORDER BY s.updated_at ASC, s.provider_session_id ASC",
            thread_id,
        )?,
        provider_threads: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_provider_threads WHERE thread_id = ?1 ORDER BY updated_at ASC, provider_thread_id ASC",
            thread_id,
        )?,
        provider_turns: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_provider_turns WHERE thread_id = ?1 ORDER BY ordinal ASC, provider_turn_id ASC",
            thread_id,
        )?,
        runtime_requests: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_runtime_requests WHERE thread_id = ?1 ORDER BY created_at ASC, runtime_request_id ASC",
            thread_id,
        )?,
        raw_messages: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_messages WHERE thread_id = ?1 ORDER BY created_at ASC, message_id ASC",
            thread_id,
        )?,
        plans: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_plans WHERE thread_id = ?1 ORDER BY plan_id ASC",
            thread_id,
        )?,
        turn_items: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_turn_items WHERE thread_id = ?1 ORDER BY ordinal ASC, turn_item_id ASC",
            thread_id,
        )?,
        checkpoint_scopes: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_checkpoint_scopes WHERE thread_id = ?1 ORDER BY ordinal_within_parent ASC, scope_id ASC",
            thread_id,
        )?,
        checkpoints: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_checkpoints WHERE thread_id = ?1 ORDER BY ordinal_within_scope ASC, checkpoint_id ASC",
            thread_id,
        )?,
        context_handoffs: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_context_handoffs WHERE thread_id = ?1 ORDER BY updated_at ASC, context_handoff_id ASC",
            thread_id,
        )?,
        context_transfers: query_json_rows(
            conn,
            "SELECT payload_json FROM orchestration_v2_projection_context_transfers WHERE source_thread_id = ?1 OR target_thread_id = ?1 ORDER BY updated_at ASC, context_transfer_id ASC",
            thread_id,
        )?,
    })
}

fn query_json_rows(
    conn: &Connection,
    sql: &str,
    thread_id: &str,
) -> Result<Vec<Value>, ProjectionError> {
    let mut stmt = conn.prepare(sql)?;
    let raws = stmt
        .query_map(params![thread_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(raws.len());
    for raw in raws {
        out.push(serde_json::from_str(&raw).map_err(|e| ProjectionError::Codec(e.to_string()))?);
    }
    Ok(out)
}
