//! Outbound protocol v3 event definitions (`MonitorEventV3` and event payloads).

use serde::{Deserialize, Deserializer, Serialize};

use crate::resource_monitor_history::{HISTORY_CHUNK_SNAPSHOTS, RESOURCE_MONITOR_PROTOCOL_VERSION};
use crate::resource_monitor_types::ResourceSampleV3;

pub(crate) fn build_history_chunks(
    request_id: String,
    snapshots: &[ResourceSampleV3],
) -> Vec<MonitorEventV3> {
    if snapshots.is_empty() {
        return vec![MonitorEventV3::HistoryChunk(HistoryChunkEventV3 {
            version: RESOURCE_MONITOR_PROTOCOL_VERSION,
            event_type: "historyChunk".to_owned(),
            request_id,
            done: true,
            snapshots: Vec::new(),
        })];
    }
    let chunk_count = snapshots.len().div_ceil(HISTORY_CHUNK_SNAPSHOTS);
    snapshots
        .chunks(HISTORY_CHUNK_SNAPSHOTS)
        .enumerate()
        .map(|(idx, chunk)| {
            MonitorEventV3::HistoryChunk(HistoryChunkEventV3 {
                version: RESOURCE_MONITOR_PROTOCOL_VERSION,
                event_type: "historyChunk".to_owned(),
                request_id: request_id.clone(),
                done: idx + 1 == chunk_count,
                snapshots: chunk.to_vec(),
            })
        })
        .collect()
}

pub(crate) fn build_error_event(
    code: impl Into<String>,
    message: impl Into<String>,
    recoverable: bool,
) -> MonitorEventV3 {
    MonitorEventV3::Error(MonitorErrorEventV3 {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        event_type: "error".to_owned(),
        code: code.into(),
        message: message.into(),
        recoverable,
    })
}

/// Capability flags reported in the v3 `hello` handshake event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Protocol v3 wire schema defines 7 boolean capability flags"
)]
pub struct MonitorCapabilitiesV3 {
    /// Cumulative user+kernel CPU time is reported.
    pub cumulative_cpu_time: bool,
    /// Rolling CPU percentage is reported.
    pub current_cpu_percent: bool,
    /// Resident memory bytes are reported.
    pub resident_memory: bool,
    /// Virtual memory bytes are reported.
    pub virtual_memory: bool,
    /// Process I/O byte counters are reported.
    pub io_bytes: bool,
    /// Process start timestamps are reported.
    pub process_start_time: bool,
    /// Recursive parent/child tree traversal is supported.
    pub process_tree: bool,
}

impl Default for MonitorCapabilitiesV3 {
    fn default() -> Self {
        Self {
            cumulative_cpu_time: true,
            current_cpu_percent: true,
            resident_memory: true,
            virtual_memory: true,
            io_bytes: true,
            process_start_time: true,
            process_tree: true,
        }
    }
}

/// Handshake event emitted upon starting the protocol v3 monitor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloEventV3 {
    /// Protocol version (`3`).
    pub version: u32,
    /// Wire event discriminant (`"hello"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Crate version of the monitor engine.
    pub sidecar_version: String,
    /// Process identifier of the monitor engine.
    pub sidecar_pid: u32,
    /// Operating system identifier (`std::env::consts::OS`).
    pub platform: String,
    /// CPU architecture identifier (`std::env::consts::ARCH`).
    pub arch: String,
    /// Advertised telemetry capabilities.
    pub capabilities: MonitorCapabilitiesV3,
}

/// Lightweight entry inside a `processTable` event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTableEntryV3 {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier.
    pub ppid: u32,
    /// Process name.
    pub name: String,
}

/// Outbound response for `MonitorRequestV3::ProcessTable`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTableEventV3 {
    /// Protocol version (`3`).
    pub version: u32,
    /// Wire event discriminant (`"processTable"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Echoed correlation ID.
    pub request_id: String,
    /// Sorted list of visible OS processes.
    pub processes: Vec<ProcessTableEntryV3>,
}

/// Outbound chunked response for `MonitorRequestV3::ReadHistory`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryChunkEventV3 {
    /// Protocol version (`3`).
    pub version: u32,
    /// Wire event discriminant (`"historyChunk"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Echoed correlation ID.
    pub request_id: String,
    /// `true` on the final chunk for this request.
    pub done: bool,
    /// Snapshot slice (at most 32 snapshots per chunk).
    pub snapshots: Vec<ResourceSampleV3>,
}

/// Outbound protocol error event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorErrorEventV3 {
    /// Protocol version (`3`).
    pub version: u32,
    /// Wire event discriminant (`"error"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Whether the session can continue processing subsequent commands.
    pub recoverable: bool,
}

/// Union of all outbound protocol v3 NDJSON events.
#[derive(Debug, Clone, PartialEq)]
pub enum MonitorEventV3 {
    /// Initial handshake event.
    Hello(HelloEventV3),
    /// Sample snapshot event.
    Snapshot(ResourceSampleV3),
    /// Flat OS process table response.
    ProcessTable(ProcessTableEventV3),
    /// Chunked history response.
    HistoryChunk(HistoryChunkEventV3),
    /// Recoverable or fatal protocol error event.
    Error(MonitorErrorEventV3),
}

impl Serialize for MonitorEventV3 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Hello(ev) => ev.serialize(serializer),
            Self::Snapshot(ev) => ev.serialize(serializer),
            Self::ProcessTable(ev) => ev.serialize(serializer),
            Self::HistoryChunk(ev) => ev.serialize(serializer),
            Self::Error(ev) => ev.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MonitorEventV3 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("type"))?;
        match kind {
            "hello" => serde_json::from_value(value).map(Self::Hello),
            "snapshot" => serde_json::from_value(value).map(Self::Snapshot),
            "processTable" => serde_json::from_value(value).map(Self::ProcessTable),
            "historyChunk" => serde_json::from_value(value).map(Self::HistoryChunk),
            "error" => serde_json::from_value(value).map(Self::Error),
            other => {
                return Err(serde::de::Error::unknown_variant(
                    other,
                    &["hello", "snapshot", "processTable", "historyChunk", "error"],
                ));
            }
        }
        .map_err(serde::de::Error::custom)
    }
}
