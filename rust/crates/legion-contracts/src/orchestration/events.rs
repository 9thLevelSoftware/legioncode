//! Orchestration domain events (`OrchestrationV2DomainEvent` and `StoredDomainEvent`).

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};

use crate::ids::{
    CommandId, EventId, NodeId, ProviderDriverKind, ProviderInstanceId, RawEventId, RunId, ThreadId,
};

use super::event_payloads::{DomainEventPayload, decode_payload_for_type};

/// Wire domain event (`OrchestrationV2DomainEvent` at `orchestrationV2.ts:2014-2175`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainEvent {
    /// Event ID.
    pub id: EventId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Optional run ID.
    pub run_id: Option<RunId>,
    /// Optional execution node ID.
    pub node_id: Option<NodeId>,
    /// Optional provider driver kind.
    pub driver: Option<ProviderDriverKind>,
    /// Optional provider instance ID.
    pub provider_instance_id: Option<ProviderInstanceId>,
    /// Optional raw provider event ID.
    pub raw_event_id: Option<RawEventId>,
    /// ISO-8601 UTC occurrence timestamp.
    pub occurred_at: String,
    /// Event discriminant and typed payload.
    pub payload: DomainEventPayload,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDomainEventHeader {
    id: EventId,
    thread_id: ThreadId,
    #[serde(default)]
    run_id: Option<RunId>,
    #[serde(default)]
    node_id: Option<NodeId>,
    #[serde(default)]
    driver: Option<ProviderDriverKind>,
    #[serde(default)]
    provider_instance_id: Option<ProviderInstanceId>,
    #[serde(default)]
    raw_event_id: Option<RawEventId>,
    occurred_at: String,
    #[serde(rename = "type")]
    event_type: Value,
    payload: Value,
}

impl Serialize for DomainEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut obj = Map::new();
        obj.insert("id".to_owned(), Value::String(self.id.as_str().to_owned()));
        obj.insert(
            "threadId".to_owned(),
            Value::String(self.thread_id.as_str().to_owned()),
        );
        if let Some(ref run_id) = self.run_id {
            obj.insert(
                "runId".to_owned(),
                Value::String(run_id.as_str().to_owned()),
            );
        }
        if let Some(ref node_id) = self.node_id {
            obj.insert(
                "nodeId".to_owned(),
                Value::String(node_id.as_str().to_owned()),
            );
        }
        if let Some(ref driver) = self.driver {
            obj.insert(
                "driver".to_owned(),
                Value::String(driver.as_str().to_owned()),
            );
        }
        if let Some(ref provider_instance_id) = self.provider_instance_id {
            obj.insert(
                "providerInstanceId".to_owned(),
                Value::String(provider_instance_id.as_str().to_owned()),
            );
        }
        if let Some(ref raw_event_id) = self.raw_event_id {
            obj.insert(
                "rawEventId".to_owned(),
                Value::String(raw_event_id.as_str().to_owned()),
            );
        }
        obj.insert(
            "occurredAt".to_owned(),
            Value::String(self.occurred_at.clone()),
        );
        obj.insert(
            "type".to_owned(),
            Value::String(self.payload.event_type().to_owned()),
        );
        let payload_val = self
            .payload
            .serialize_payload_value()
            .map_err(S::Error::custom)?;
        obj.insert("payload".to_owned(), payload_val);
        obj.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DomainEvent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawDomainEventHeader::deserialize(deserializer)?;
        let Value::String(event_type) = raw.event_type else {
            return Err(de::Error::custom(
                "domain event `type` tag must be a string",
            ));
        };
        let payload =
            decode_payload_for_type(&event_type, raw.payload).map_err(de::Error::custom)?;
        Ok(Self {
            id: raw.id,
            thread_id: raw.thread_id,
            run_id: raw.run_id,
            node_id: raw.node_id,
            driver: raw.driver,
            provider_instance_id: raw.provider_instance_id,
            raw_event_id: raw.raw_event_id,
            occurred_at: raw.occurred_at,
            payload,
        })
    }
}

/// Persisted domain event envelope (`OrchestrationV2StoredEvent` at `orchestrationV2.ts:2461-2466`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredDomainEvent {
    /// Monotonic event sequence number.
    pub sequence: u64,
    /// Originating idempotency command ID, or `null` for system/internal events.
    pub command_id: Option<CommandId>,
    /// Persisted domain event.
    pub event: DomainEvent,
}
