//! Materialized projection updates and decider-state queries inside `SQLite` transactions.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use t3_contracts::ids::{CheckpointId, ThreadId};
use t3_contracts::orchestration::{AppThread, ConversationMessage, DomainEventPayload, Run};

use crate::sink::event_codec::{PersistedEvent, load_thread_events};
use crate::sink::types::SinkError;

const THREAD_PROJECTION_NAME: &str = "thread-projections";
const THREAD_PROJECTION_SCHEMA_VERSION: i64 = 1;

/// Thread state loaded for pure command evaluation in [`crate::decider::decide_command`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeciderThreadState {
    /// Current materialized thread snapshot (`None` when creating a new thread).
    pub thread: Option<AppThread>,
    /// All known runs for this thread ordered by `ordinal ASC`.
    pub runs: Vec<Run>,
    /// Known checkpoint IDs recorded on this thread's event stream.
    pub known_checkpoints: HashSet<CheckpointId>,
    /// Known child threads spawned by this thread (to guard `thread.archive`).
    pub child_threads: Vec<AppThread>,
}

/// Applies persisted domain events to V2 projection tables in the caller's active transaction.
pub fn apply_events_to_projections(
    conn: &Connection,
    events: &[PersistedEvent],
    updated_at: &str,
) -> Result<(), SinkError> {
    if events.is_empty() {
        return Ok(());
    }
    let mut max_seq = 0_i64;
    for persisted in events {
        max_seq = max_seq.max(persisted.sequence);
        match &persisted.event.payload {
            DomainEventPayload::ThreadState { event_type, thread } => {
                upsert_thread_projection(conn, thread, event_type == "thread.deleted")?;
            }
            DomainEventPayload::RunUpdated { run, .. } => {
                upsert_run_projection(conn, run)?;
            }
            DomainEventPayload::MessageUpdated(message) => {
                upsert_message_projection(conn, message)?;
            }
            _ => {}
        }
    }
    if max_seq > 0 {
        conn.execute(
            "INSERT INTO orchestration_v2_projection_metadata (
               projection_name, schema_version, last_sequence, updated_at
             )
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(projection_name) DO UPDATE SET
               schema_version = excluded.schema_version,
               last_sequence = MAX(orchestration_v2_projection_metadata.last_sequence, excluded.last_sequence),
               updated_at = excluded.updated_at",
            params![
                THREAD_PROJECTION_NAME,
                THREAD_PROJECTION_SCHEMA_VERSION,
                max_seq,
                updated_at,
            ],
        )?;
    }
    Ok(())
}

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
    Ok(DeciderThreadState {
        thread,
        runs,
        known_checkpoints,
        child_threads,
    })
}

fn upsert_thread_projection(
    conn: &Connection,
    thread: &AppThread,
    is_deleted: bool,
) -> Result<(), SinkError> {
    let payload_value =
        serde_json::to_value(thread).map_err(|err| SinkError::Codec(err.to_string()))?;
    let payload_json =
        serde_json::to_string(&payload_value).map_err(|err| SinkError::Codec(err.to_string()))?;
    let runtime_mode = str_field(&payload_value, "runtimeMode").unwrap_or("full-access");
    let interaction_mode = str_field(&payload_value, "interactionMode").unwrap_or("default");
    let default_provider = thread.provider_instance_id.as_str();
    let active_provider_thread_id = str_field(&payload_value, "activeProviderThreadId");
    let provider_instance_id = Some(thread.provider_instance_id.as_str());
    let deleted_at = if is_deleted {
        thread
            .deleted_at
            .as_deref()
            .or(Some(thread.updated_at.as_str()))
    } else {
        thread.deleted_at.as_deref()
    };
    let archived_at = thread.archived_at.as_deref();

    conn.execute(
        "INSERT INTO orchestration_v2_projection_threads (
           thread_id, project_id, title, default_provider, runtime_mode, interaction_mode,
           active_provider_thread_id, created_at, updated_at, archived_at, deleted_at,
           payload_json, provider_instance_id
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(thread_id) DO UPDATE SET
           project_id = excluded.project_id,
           title = excluded.title,
           default_provider = excluded.default_provider,
           runtime_mode = excluded.runtime_mode,
           interaction_mode = excluded.interaction_mode,
           active_provider_thread_id = excluded.active_provider_thread_id,
           updated_at = excluded.updated_at,
           archived_at = excluded.archived_at,
           deleted_at = excluded.deleted_at,
           payload_json = excluded.payload_json,
           provider_instance_id = excluded.provider_instance_id",
        params![
            thread.id.as_str(),
            thread.project_id.as_str(),
            thread.title,
            default_provider,
            runtime_mode,
            interaction_mode,
            active_provider_thread_id,
            thread.created_at,
            thread.updated_at,
            archived_at,
            deleted_at,
            payload_json,
            provider_instance_id,
        ],
    )?;
    Ok(())
}

fn upsert_run_projection(conn: &Connection, run: &Run) -> Result<(), SinkError> {
    let payload_value =
        serde_json::to_value(run).map_err(|err| SinkError::Codec(err.to_string()))?;
    let payload_json =
        serde_json::to_string(&payload_value).map_err(|err| SinkError::Codec(err.to_string()))?;
    let provider = run.provider_instance_id.as_str();
    let provider_thread_id = str_field(&payload_value, "providerThreadId");
    let status = str_field(&payload_value, "status").unwrap_or("queued");
    let requested_at = str_field(&payload_value, "requestedAt").unwrap_or("");
    let completed_at = str_field(&payload_value, "completedAt");
    let provider_instance_id = str_field(&payload_value, "providerInstanceId");
    let ordinal = payload_value
        .get("ordinal")
        .and_then(Value::as_i64)
        .unwrap_or(1);

    conn.execute(
        "INSERT INTO orchestration_v2_projection_runs (
           run_id, thread_id, ordinal, provider, provider_thread_id, status,
           requested_at, completed_at, payload_json, provider_instance_id
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(run_id) DO UPDATE SET
           thread_id = excluded.thread_id,
           ordinal = excluded.ordinal,
           provider = excluded.provider,
           provider_thread_id = excluded.provider_thread_id,
           status = excluded.status,
           requested_at = excluded.requested_at,
           completed_at = excluded.completed_at,
           payload_json = excluded.payload_json,
           provider_instance_id = excluded.provider_instance_id",
        params![
            run.id.as_str(),
            run.thread_id.as_str(),
            ordinal,
            provider,
            provider_thread_id,
            status,
            requested_at,
            completed_at,
            payload_json,
            provider_instance_id,
        ],
    )?;
    Ok(())
}

fn upsert_message_projection(
    conn: &Connection,
    message: &ConversationMessage,
) -> Result<(), SinkError> {
    let payload_value =
        serde_json::to_value(message).map_err(|err| SinkError::Codec(err.to_string()))?;
    let payload_json =
        serde_json::to_string(&payload_value).map_err(|err| SinkError::Codec(err.to_string()))?;
    let message_id = str_field(&payload_value, "id").unwrap_or("");
    let thread_id = str_field(&payload_value, "threadId").unwrap_or("");
    let run_id = str_field(&payload_value, "runId");
    let node_id = str_field(&payload_value, "nodeId");
    let role = str_field(&payload_value, "role").unwrap_or("user");
    let streaming = i64::from(
        payload_value
            .get("streaming")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    let created_at = str_field(&payload_value, "createdAt").unwrap_or("");
    let updated_at = str_field(&payload_value, "updatedAt").unwrap_or(created_at);
    let search_text = str_field(&payload_value, "text").map(str::to_owned);

    conn.execute(
        "INSERT INTO orchestration_v2_projection_messages (
           message_id, thread_id, run_id, node_id, role, streaming,
           created_at, updated_at, payload_json, search_text
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(message_id) DO UPDATE SET
           run_id = excluded.run_id,
           node_id = excluded.node_id,
           role = excluded.role,
           streaming = excluded.streaming,
           updated_at = excluded.updated_at,
           payload_json = excluded.payload_json,
           search_text = excluded.search_text",
        params![
            message_id,
            thread_id,
            run_id,
            node_id,
            role,
            streaming,
            created_at,
            updated_at,
            payload_json,
            search_text,
        ],
    )?;
    Ok(())
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

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
