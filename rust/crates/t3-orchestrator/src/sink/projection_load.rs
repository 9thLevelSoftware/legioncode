//! Projection query helpers loading materialized thread, run, and message records.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};
use t3_contracts::ids::{CheckpointId, MessageId, RunId, ThreadId};
use t3_contracts::orchestration::{AppThread, ConversationMessage, DomainEventPayload, Run};

use crate::sink::event_codec::load_thread_events;
use crate::sink::projection_apply::DeciderThreadState;
use crate::sink::types::SinkError;

/// Loads the materialized [`AppThread`] row from `orchestration_v2_projection_threads`.
pub fn load_thread_projection(
    conn: &Connection,
    thread_id: &ThreadId,
) -> Result<Option<AppThread>, SinkError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
            params![thread_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|json| serde_json::from_str::<AppThread>(&json))
        .transpose()
        .map_err(|err| SinkError::Codec(err.to_string()))
}

/// Loads all materialized [`Run`] rows for `thread_id` ordered by `ordinal ASC`.
pub fn load_thread_runs(conn: &Connection, thread_id: &ThreadId) -> Result<Vec<Run>, SinkError> {
    let mut stmt = conn.prepare(
        "SELECT payload_json
         FROM orchestration_v2_projection_runs
         WHERE thread_id = ?1
         ORDER BY ordinal ASC",
    )?;
    let rows = stmt
        .query_map(params![thread_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|json| serde_json::from_str::<Run>(&json))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| SinkError::Codec(err.to_string()))
}

/// Loads a single materialized [`Run`] row from `orchestration_v2_projection_runs`.
pub fn load_run_projection(conn: &Connection, run_id: &RunId) -> Result<Option<Run>, SinkError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM orchestration_v2_projection_runs WHERE run_id = ?1",
            params![run_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|json| serde_json::from_str::<Run>(&json))
        .transpose()
        .map_err(|err| SinkError::Codec(err.to_string()))
}

/// Loads all materialized [`ConversationMessage`] rows for `thread_id` keyed by [`MessageId`].
pub fn load_thread_messages(
    conn: &Connection,
    thread_id: &ThreadId,
) -> Result<HashMap<MessageId, ConversationMessage>, SinkError> {
    let mut stmt = conn.prepare(
        "SELECT payload_json
         FROM orchestration_v2_projection_messages
         WHERE thread_id = ?1",
    )?;
    let rows = stmt
        .query_map(params![thread_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = HashMap::with_capacity(rows.len());
    for json in rows {
        let msg: ConversationMessage =
            serde_json::from_str(&json).map_err(|err| SinkError::Codec(err.to_string()))?;
        out.insert(msg.id.clone(), msg);
    }
    Ok(out)
}

/// Loads [`DeciderThreadState`] for `thread_id` from projections and checkpoint events.
pub fn load_decider_state(
    conn: &Connection,
    thread_id: &ThreadId,
) -> Result<DeciderThreadState, SinkError> {
    let thread = load_thread_projection(conn, thread_id)?;
    let runs = load_thread_runs(conn, thread_id)?;
    let mut known_checkpoints = load_known_checkpoints(conn, thread_id)?;
    for run in &runs {
        if let Some(cp_id) = &run.checkpoint_id {
            known_checkpoints.insert(cp_id.clone());
        }
    }
    let child_threads = load_child_threads(conn, thread_id)?;
    let messages_by_id = load_thread_messages(conn, thread_id)?;
    Ok(DeciderThreadState {
        thread,
        runs,
        known_checkpoints,
        child_threads,
        messages_by_id,
        has_active_terminal_subprocess: false,
    })
}

fn load_known_checkpoints(
    conn: &Connection,
    thread_id: &ThreadId,
) -> Result<HashSet<CheckpointId>, SinkError> {
    let mut set = HashSet::new();
    for persisted in load_thread_events(conn, thread_id, 0)? {
        if let DomainEventPayload::CheckpointCaptured(cp) = persisted.event.payload {
            set.insert(cp.id);
        }
    }
    Ok(set)
}

fn load_child_threads(
    conn: &Connection,
    parent_thread_id: &ThreadId,
) -> Result<Vec<AppThread>, SinkError> {
    let mut stmt = conn.prepare(
        "SELECT payload_json
         FROM orchestration_v2_projection_threads
         WHERE deleted_at IS NULL
           AND json_extract(payload_json, '$.lineage.parentThreadId') = ?1",
    )?;
    let rows = stmt
        .query_map(params![parent_thread_id.as_str()], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|json| serde_json::from_str::<AppThread>(&json))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| SinkError::Codec(err.to_string()))
}
