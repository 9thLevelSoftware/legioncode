//! Materialized projection updates inside `SQLite` transactions.

use std::collections::{HashMap, HashSet};

use legion_contracts::ids::{CheckpointId, MessageId};
use legion_contracts::orchestration::{AppThread, ConversationMessage, DomainEventPayload, Run};
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::sink::event_codec::PersistedEvent;
pub use crate::sink::projection_load::{
    load_decider_state, load_run_projection, load_thread_messages, load_thread_projection,
    load_thread_runs,
};
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
    /// Known thread messages indexed by [`MessageId`] (used for queued-sender ordering/selection).
    pub messages_by_id: HashMap<MessageId, ConversationMessage>,
    /// True when any PTY terminal on this thread still has an active subprocess (`#39835`).
    pub has_active_terminal_subprocess: bool,
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
            DomainEventPayload::RunOutputSubmitted(submitted) => {
                if let Some(mut run) = load_run_projection(conn, &submitted.run_id)? {
                    run.output = Some(submitted.output.clone());
                    upsert_run_projection(conn, &run)?;
                }
            }
            DomainEventPayload::RunBackgroundWorkCancelled(cancelled) => {
                if let Some(mut run) = load_run_projection(conn, &cancelled.run_id)? {
                    run.restart_cancelled_background_work =
                        Some(cancelled.restart_cancelled_background_work.clone());
                    upsert_run_projection(conn, &run)?;
                }
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
    let mut effective_run = run.clone();
    if (effective_run.output.is_none() || effective_run.restart_cancelled_background_work.is_none())
        && let Some(existing) = load_run_projection(conn, &run.id)?
    {
        if effective_run.output.is_none() {
            effective_run.output = existing.output;
        }
        if effective_run.restart_cancelled_background_work.is_none() {
            effective_run.restart_cancelled_background_work =
                existing.restart_cancelled_background_work;
        }
    }

    let payload_value =
        serde_json::to_value(&effective_run).map_err(|err| SinkError::Codec(err.to_string()))?;
    let payload_json =
        serde_json::to_string(&payload_value).map_err(|err| SinkError::Codec(err.to_string()))?;
    let provider = effective_run.provider_instance_id.as_str();
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
            effective_run.id.as_str(),
            effective_run.thread_id.as_str(),
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

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
