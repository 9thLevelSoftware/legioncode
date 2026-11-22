//! Event serialization, insertion (`application_event_version = 2`), and decoding for `orchestration_events`.

use std::collections::HashMap;

use legion_contracts::ids::{
    CommandId, EventId, ProviderDriverKind, ProviderInstanceId, RawEventId, RunId, ThreadId,
};
use legion_contracts::orchestration::DomainEvent;
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::sink::types::SinkError;

/// Application event schema version written by Orchestrator V2.
pub const APPLICATION_EVENT_VERSION_V2: i64 = 2;

/// Provenance metadata persisted alongside each row in `orchestration_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEventMetadata {
    /// Actor kind (`server`, `client`, `provider`, etc.).
    pub actor_kind: String,
    /// Optional parent event ID.
    pub causation_event_id: Option<EventId>,
    /// Optional correlation ID.
    pub correlation_id: Option<String>,
    /// Optional command ID.
    pub command_id: Option<CommandId>,
}

/// Row persisted in `orchestration_events` together with its decoded [`DomainEvent`].
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedEvent {
    /// Global monotonically increasing sequence number (`orchestration_events.sequence`).
    pub sequence: i64,
    /// Per-thread stream version (`orchestration_events.stream_version`).
    pub stream_version: i64,
    /// Event provenance metadata.
    pub metadata: StoredEventMetadata,
    /// Decoded domain event.
    pub event: DomainEvent,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvelopeExtra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node_id: Option<legion_contracts::ids::NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    driver: Option<ProviderDriverKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_instance_id: Option<ProviderInstanceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw_event_id: Option<RawEventId>,
}

/// Inserts `events` into `orchestration_events` within the caller's active `SQLite` transaction.
pub fn insert_thread_events(
    conn: &Connection,
    meta: &StoredEventMetadata,
    events: &[DomainEvent],
) -> Result<Vec<PersistedEvent>, SinkError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let mut next_versions: HashMap<String, i64> = HashMap::new();
    let mut persisted = Vec::with_capacity(events.len());
    let mut insert_stmt = conn.prepare_cached(
        "INSERT INTO orchestration_events (
           event_id, aggregate_kind, stream_id, stream_version, event_type,
           occurred_at, command_id, causation_event_id, correlation_id,
           actor_kind, payload_json, metadata_json, application_event_version
         )
         VALUES (?1, 'thread', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         RETURNING sequence",
    )?;
    let mut version_stmt = conn.prepare_cached(
        "SELECT COALESCE(MAX(stream_version), 0)
         FROM orchestration_events
         WHERE aggregate_kind = 'thread' AND stream_id = ?1",
    )?;

    for event in events {
        let stream_key = event.thread_id.as_str().to_owned();
        let next_ver = if let Some(current) = next_versions.get_mut(&stream_key) {
            *current += 1;
            *current
        } else {
            let max_ver: i64 = version_stmt.query_row(params![stream_key], |row| row.get(0))?;
            let ver = max_ver + 1;
            next_versions.insert(stream_key.clone(), ver);
            ver
        };

        let (event_type, payload_json) = encode_event(event)?;
        let metadata_json = encode_envelope_extra(event)?;
        let sequence: i64 = insert_stmt.query_row(
            params![
                event.id.as_str(),
                event.thread_id.as_str(),
                next_ver,
                event_type,
                event.occurred_at,
                meta.command_id.as_ref().map(CommandId::as_str),
                meta.causation_event_id.as_ref().map(EventId::as_str),
                meta.correlation_id.as_deref(),
                meta.actor_kind,
                payload_json,
                metadata_json,
                APPLICATION_EVENT_VERSION_V2,
            ],
            |row| row.get(0),
        )?;

        persisted.push(PersistedEvent {
            sequence,
            stream_version: next_ver,
            metadata: meta.clone(),
            event: event.clone(),
        });
    }
    Ok(persisted)
}

/// Loads all V2 persisted events associated with `command_id` ordered by `sequence ASC`.
pub fn load_events_by_command(
    conn: &Connection,
    command_id: &CommandId,
) -> Result<Vec<PersistedEvent>, SinkError> {
    let mut stmt = conn.prepare(
        "SELECT sequence, event_id, stream_id, stream_version, event_type, occurred_at,
                command_id, causation_event_id, correlation_id, actor_kind, payload_json, metadata_json
         FROM orchestration_events
         WHERE command_id = ?1 AND application_event_version = 2
         ORDER BY sequence ASC",
    )?;
    let rows = stmt
        .query_map(params![command_id.as_str()], row_to_tuple)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(decode_tuple).collect()
}

/// Loads V2 persisted events globally after `after_sequence` ordered by `sequence ASC`.
pub fn load_events_after(
    conn: &Connection,
    after_sequence: i64,
    limit: usize,
) -> Result<Vec<PersistedEvent>, SinkError> {
    let limit_i64 = i64::try_from(limit.max(1)).unwrap_or(1_000);
    let mut stmt = conn.prepare(
        "SELECT sequence, event_id, stream_id, stream_version, event_type, occurred_at,
                command_id, causation_event_id, correlation_id, actor_kind, payload_json, metadata_json
         FROM orchestration_events
         WHERE application_event_version = 2 AND sequence > ?1
         ORDER BY sequence ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![after_sequence, limit_i64], row_to_tuple)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(decode_tuple).collect()
}

/// Loads V2 persisted events for a single `thread_id` ordered by `sequence ASC`.
pub fn load_thread_events(
    conn: &Connection,
    thread_id: &ThreadId,
    after_sequence: i64,
) -> Result<Vec<PersistedEvent>, SinkError> {
    let mut stmt = conn.prepare(
        "SELECT sequence, event_id, stream_id, stream_version, event_type, occurred_at,
                command_id, causation_event_id, correlation_id, actor_kind, payload_json, metadata_json
         FROM orchestration_events
         WHERE aggregate_kind = 'thread'
           AND stream_id = ?1
           AND application_event_version = 2
           AND sequence > ?2
         ORDER BY sequence ASC",
    )?;
    let rows = stmt
        .query_map(params![thread_id.as_str(), after_sequence], row_to_tuple)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(decode_tuple).collect()
}

fn encode_event(event: &DomainEvent) -> Result<(String, String), SinkError> {
    let event_type = event.payload.event_type().to_owned();
    let value = serde_json::to_value(event).map_err(|err| SinkError::Codec(err.to_string()))?;
    let payload_val = value
        .get("payload")
        .ok_or_else(|| SinkError::Codec("serialized DomainEvent missing `payload`".to_owned()))?;
    let payload_json =
        serde_json::to_string(payload_val).map_err(|err| SinkError::Codec(err.to_string()))?;
    Ok((event_type, payload_json))
}

fn encode_envelope_extra(event: &DomainEvent) -> Result<String, SinkError> {
    let extra = EnvelopeExtra {
        run_id: event.run_id.clone(),
        node_id: event.node_id.clone(),
        driver: event.driver.clone(),
        provider_instance_id: event.provider_instance_id.clone(),
        raw_event_id: event.raw_event_id.clone(),
    };
    serde_json::to_string(&extra).map_err(|err| SinkError::Codec(err.to_string()))
}

type EventRowTuple = (
    i64,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

fn row_to_tuple(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRowTuple> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn decode_tuple(row: EventRowTuple) -> Result<PersistedEvent, SinkError> {
    let command_id = row
        .6
        .map(CommandId::new)
        .transpose()
        .map_err(|err| SinkError::Codec(err.to_string()))?;
    let causation_event_id = row
        .7
        .map(EventId::new)
        .transpose()
        .map_err(|err| SinkError::Codec(err.to_string()))?;
    let payload_val: Value =
        serde_json::from_str(&row.10).map_err(|err| SinkError::Codec(err.to_string()))?;
    let extra: EnvelopeExtra = if row.11.trim().is_empty() || row.11.trim() == "{}" {
        EnvelopeExtra {
            run_id: None,
            node_id: None,
            driver: None,
            provider_instance_id: None,
            raw_event_id: None,
        }
    } else {
        serde_json::from_str(&row.11).map_err(|err| SinkError::Codec(err.to_string()))?
    };

    let mut envelope = serde_json::Map::new();
    envelope.insert("id".to_owned(), Value::String(row.1));
    envelope.insert("threadId".to_owned(), Value::String(row.2));
    envelope.insert("occurredAt".to_owned(), Value::String(row.5));
    envelope.insert("type".to_owned(), Value::String(row.4));
    if let Some(run_id) = &extra.run_id {
        envelope.insert(
            "runId".to_owned(),
            Value::String(run_id.as_str().to_owned()),
        );
    }
    if let Some(node_id) = &extra.node_id {
        envelope.insert(
            "nodeId".to_owned(),
            Value::String(node_id.as_str().to_owned()),
        );
    }
    if let Some(driver) = &extra.driver {
        envelope.insert(
            "driver".to_owned(),
            Value::String(driver.as_str().to_owned()),
        );
    }
    if let Some(pid) = &extra.provider_instance_id {
        envelope.insert(
            "providerInstanceId".to_owned(),
            Value::String(pid.as_str().to_owned()),
        );
    }
    if let Some(raw_id) = &extra.raw_event_id {
        envelope.insert(
            "rawEventId".to_owned(),
            Value::String(raw_id.as_str().to_owned()),
        );
    }
    envelope.insert("payload".to_owned(), payload_val);

    let event: DomainEvent = serde_json::from_value(Value::Object(envelope))
        .map_err(|err| SinkError::Codec(err.to_string()))?;

    Ok(PersistedEvent {
        sequence: row.0,
        stream_version: row.3,
        metadata: StoredEventMetadata {
            actor_kind: row.9,
            causation_event_id,
            correlation_id: row.8,
            command_id,
        },
        event,
    })
}
