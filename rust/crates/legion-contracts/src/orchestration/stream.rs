//! Thread projection and forward-compatible stream-item decoding (fixing BUG-18).

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};
use serde_json::{Map, Value};

use crate::id_table::{IdTableTarget, decode_with_id_table};

use super::checkpoint_models::{
    Checkpoint, CheckpointScope, ContextHandoff, ContextTransfer, PlanArtifact,
};
use super::common::PatchValue;
use super::event_payloads::is_known_domain_event_type;
use super::events::DomainEvent;
use super::items::{
    ProjectedTurnItem, TurnItem, TurnItemDecodeError, decode_turn_item_value,
    deserialize_forward_compatible_projected_items, deserialize_forward_compatible_turn_items,
};
use super::run_models::{ExecutionNode, Run, RunAttempt, Subagent};
use super::runtime_models::{
    ConversationMessage, ProviderSession, ProviderThread, ProviderTurn, RuntimeRequest,
};
use super::state_models::AppThread;

/// Full thread state projection delivered in `snapshot` stream items (`orchestrationV2.ts:2178-2204`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadProjection {
    /// Thread aggregate.
    pub thread: AppThread,
    /// Runs belonging to this thread.
    pub runs: Vec<Run>,
    /// Run attempts belonging to this thread.
    pub attempts: Vec<RunAttempt>,
    /// Execution tree nodes.
    pub nodes: Vec<ExecutionNode>,
    /// Subagent records.
    pub subagents: Vec<Subagent>,
    /// Provider sessions.
    pub provider_sessions: Vec<ProviderSession>,
    /// Provider threads.
    pub provider_threads: Vec<ProviderThread>,
    /// Provider turns.
    pub provider_turns: Vec<ProviderTurn>,
    /// Runtime requests.
    pub runtime_requests: Vec<RuntimeRequest>,
    /// Conversation messages.
    pub messages: Vec<ConversationMessage>,
    /// Plan artifacts.
    pub plans: Vec<PlanArtifact>,
    /// Turn items (forward-compatible: unknown string item types are skipped; non-string types fail).
    #[serde(deserialize_with = "deserialize_forward_compatible_turn_items")]
    pub turn_items: Vec<TurnItem>,
    /// Checkpoint scopes.
    pub checkpoint_scopes: Vec<CheckpointScope>,
    /// Captured checkpoints.
    pub checkpoints: Vec<Checkpoint>,
    /// Context handoffs.
    pub context_handoffs: Vec<ContextHandoff>,
    /// Context transfers.
    pub context_transfers: Vec<ContextTransfer>,
    /// Projected timeline items (forward-compatible: unknown string item types are skipped).
    #[serde(deserialize_with = "deserialize_forward_compatible_projected_items")]
    pub visible_turn_items: Vec<ProjectedTurnItem>,
    /// Last projection update timestamp.
    pub updated_at: String,
}

/// Wire item delivered by `orchestration.subscribeThread` (`orchestrationV2.ts:4238-4260`).
///
/// Fixes BUG-18: non-string `"type"` tags on either `event` or `turn-item.updated` `payload`
/// always fail deserialization with `Err`, whereas unknown *string* `event.type` values or
/// unknown *string* `turn-item.updated` `payload.type` values map to [`StreamItem::UnknownEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamItem {
    /// Live stream caught up with all persisted events.
    Synchronized,
    /// Full or bounded thread snapshot (optionally expanded from a compact `ids` table).
    Snapshot {
        /// Sequence cursor covered by this snapshot.
        snapshot_sequence: u64,
        /// Thread projection at `snapshot_sequence`.
        projection: Box<ThreadProjection>,
        /// Optional pagination cursor for older history.
        history_cursor: PatchValue<String>,
        /// Whether earlier history pages exist.
        has_more_history: Option<bool>,
        /// Latest local turn ordinal.
        latest_local_turn_ordinal: PatchValue<u64>,
        /// Marker indicating local visible items were withheld for bounded snapshot restore.
        turn_items_omit_local_visible: Option<bool>,
    },
    /// Known domain event committed at `sequence`.
    Event {
        /// Sequence number of the committed event.
        sequence: u64,
        /// Validated domain event.
        event: Box<DomainEvent>,
    },
    /// Forward-compatible placeholder for an unknown string event or turn-item `type`.
    UnknownEvent {
        /// Sequence number so the client cursor can still advance.
        sequence: u64,
        /// Unknown wire event type string (or `"turn-item.updated"` for unknown turn-item kinds).
        event_type: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSnapshotItem {
    snapshot_sequence: u64,
    projection: ThreadProjection,
    #[serde(default)]
    history_cursor: PatchValue<String>,
    #[serde(default)]
    has_more_history: Option<bool>,
    #[serde(default)]
    latest_local_turn_ordinal: PatchValue<u64>,
    #[serde(default)]
    turn_items_omit_local_visible: Option<bool>,
}

fn decode_event_stream_item(obj: &Map<String, Value>) -> Result<StreamItem, String> {
    let sequence = obj
        .get("sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| "stream event missing non-negative integer `sequence`".to_owned())?;
    let event_val = obj
        .get("event")
        .ok_or_else(|| "stream event missing `event` object".to_owned())?;
    let event_obj = event_val
        .as_object()
        .ok_or_else(|| "stream event `event` must be a JSON object".to_owned())?;

    // BUG-18 guard: `event.type` MUST be a JSON string (reject missing, null, numbers, objects).
    let Some(Value::String(event_type)) = event_obj.get("type") else {
        return Err("stream event `event.type` tag must be a string".to_owned());
    };

    if !is_known_domain_event_type(event_type) {
        return Ok(StreamItem::UnknownEvent {
            sequence,
            event_type: event_type.clone(),
        });
    }

    if event_type == "turn-item.updated" {
        let payload = event_obj
            .get("payload")
            .ok_or_else(|| "`turn-item.updated` missing `payload`".to_owned())?;
        // BUG-18 guard: `payload.type` MUST be a JSON string; non-string tags fail immediately.
        match decode_turn_item_value(payload) {
            Ok(_) => {}
            Err(TurnItemDecodeError::UnknownType(_)) => {
                return Ok(StreamItem::UnknownEvent {
                    sequence,
                    event_type: event_type.clone(),
                });
            }
            Err(TurnItemDecodeError::Invalid(msg)) => return Err(msg),
        }
    }

    let event: DomainEvent =
        serde_json::from_value(event_val.clone()).map_err(|err| err.to_string())?;
    Ok(StreamItem::Event {
        sequence,
        event: Box::new(event),
    })
}

impl Serialize for StreamItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut obj = Map::new();
        match self {
            Self::Synchronized => {
                obj.insert("kind".to_owned(), Value::String("synchronized".to_owned()));
            }
            Self::Snapshot {
                snapshot_sequence,
                projection,
                history_cursor,
                has_more_history,
                latest_local_turn_ordinal,
                turn_items_omit_local_visible,
            } => {
                obj.insert("kind".to_owned(), Value::String("snapshot".to_owned()));
                obj.insert(
                    "snapshotSequence".to_owned(),
                    Value::Number((*snapshot_sequence).into()),
                );
                obj.insert(
                    "projection".to_owned(),
                    serde_json::to_value(projection).map_err(ser::Error::custom)?,
                );
                if !history_cursor.is_absent() {
                    obj.insert(
                        "historyCursor".to_owned(),
                        serde_json::to_value(history_cursor).map_err(ser::Error::custom)?,
                    );
                }
                if let Some(more) = has_more_history {
                    obj.insert("hasMoreHistory".to_owned(), Value::Bool(*more));
                }
                if !latest_local_turn_ordinal.is_absent() {
                    obj.insert(
                        "latestLocalTurnOrdinal".to_owned(),
                        serde_json::to_value(latest_local_turn_ordinal)
                            .map_err(ser::Error::custom)?,
                    );
                }
                if let Some(omitted) = turn_items_omit_local_visible {
                    obj.insert(
                        "turnItemsOmitLocalVisible".to_owned(),
                        Value::Bool(*omitted),
                    );
                }
            }
            Self::Event { sequence, event } => {
                obj.insert("kind".to_owned(), Value::String("event".to_owned()));
                obj.insert("sequence".to_owned(), Value::Number((*sequence).into()));
                obj.insert(
                    "event".to_owned(),
                    serde_json::to_value(event).map_err(ser::Error::custom)?,
                );
            }
            Self::UnknownEvent { .. } => {
                return Err(ser::Error::custom(
                    "servers never send unknown thread events",
                ));
            }
        }
        obj.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StreamItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(deserializer)?;
        let Some(raw_obj) = raw.as_object() else {
            return Err(de::Error::custom("stream item must be a JSON object"));
        };
        let Some(Value::String(kind)) = raw_obj.get("kind") else {
            return Err(de::Error::custom("stream item `kind` tag must be a string"));
        };
        match kind.as_str() {
            "synchronized" => Ok(Self::Synchronized),
            "snapshot" => {
                let expanded = decode_with_id_table(&raw, IdTableTarget::Field("projection"))
                    .map_err(de::Error::custom)?;
                let snap: RawSnapshotItem =
                    serde_json::from_value(expanded).map_err(de::Error::custom)?;
                Ok(Self::Snapshot {
                    snapshot_sequence: snap.snapshot_sequence,
                    projection: Box::new(snap.projection),
                    history_cursor: snap.history_cursor,
                    has_more_history: snap.has_more_history,
                    latest_local_turn_ordinal: snap.latest_local_turn_ordinal,
                    turn_items_omit_local_visible: snap.turn_items_omit_local_visible,
                })
            }
            "event" => decode_event_stream_item(raw_obj).map_err(de::Error::custom),
            other => Err(de::Error::custom(format!(
                "unknown stream item kind `{other}`"
            ))),
        }
    }
}
