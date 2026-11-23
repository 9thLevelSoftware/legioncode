//! Effect 4 `RpcSerialization.layerJson` wire codec and streaming backpressure windows.
//!
//! Mirrors `.repos/effect/packages/effect/src/rpc/RpcMessage.ts`,
//! `RpcSerialization.ts`, and `apps/server/src/terminal/OutputProtocol.ts`.
//!
//! # Examples
//!
//! ```
//! use legion_protocol::rpc_codec::{
//!     ChunkDisposition, ClientRpcFrame, OutputWindow, RpcId, ServerRpcFrame,
//!     StreamWindowMode, chunk_bytes, decode_client_frames, decode_server_frames,
//!     encode_server_frame, stream_window_mode,
//! };
//!
//! let batch = r#"[
//!   {"_tag":"Ping"},
//!   {"_tag":"Request","id":"req-1","tag":"terminal.attach","payload":{"threadId":"t1"},"headers":[]}
//! ]"#;
//! let client_frames = decode_client_frames(batch)?;
//! assert_eq!(client_frames.len(), 2);
//! assert!(matches!(client_frames[0], ClientRpcFrame::Ping));
//!
//! let err_chunk = serde_json::json!({
//!     "type": "error",
//!     "threadId": "t1",
//!     "terminalId": "term-1",
//!     "message": "pty failed",
//!     "error": "EIO: read error"
//! });
//! let mode = stream_window_mode("terminal.attach");
//! assert_eq!(mode, StreamWindowMode::ByteAndChunk);
//! let mut window = OutputWindow::new(mode);
//! assert_eq!(window.on_send_chunk(&[err_chunk.clone()]), ChunkDisposition::EmitSyntheticAck);
//! assert_eq!(window.pending_bytes(), chunk_bytes(&[err_chunk]));
//!
//! let server_frame = ServerRpcFrame::Pong;
//! let encoded = encode_server_frame(&server_frame)?;
//! assert_eq!(decode_server_frames(&encoded)?, vec![ServerRpcFrame::Pong]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod window;

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use window::{
    AckDisposition, ChunkDisposition, EVENT_OVERHEAD, MAX_PENDING_BYTES, MAX_PENDING_CHUNKS,
    OutputWindow, OutputWindowRegistry, RegistryChunkAction, StreamWindowKey, StreamWindowMode,
    chunk_bytes, queued_event_bytes, stream_window_mode,
};

/// Required WebSocket `orchestrationProtocol` query parameter value.
pub const ORCHESTRATION_PROTOCOL_VERSION: &str = "2";

/// Errors raised while encoding, decoding, or validating Effect RPC wire frames.
#[derive(Debug, Error)]
pub enum RpcCodecError {
    /// Input bytes were not valid UTF-8.
    #[error("RPC payload is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    /// Payload JSON syntax could not be parsed.
    #[error("RPC JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// Top-level JSON value was neither a message object nor a batch array.
    #[error("expected Effect RPC JSON object or batch array, got {kind}")]
    UnexpectedJsonShape {
        /// Human-readable JSON type label of the invalid payload.
        kind: &'static str,
    },
    /// Required `orchestrationProtocol` query value did not match [`ORCHESTRATION_PROTOCOL_VERSION`].
    #[error(
        "unsupported orchestrationProtocol {found:?}; expected {:?}",
        ORCHESTRATION_PROTOCOL_VERSION
    )]
    UnsupportedProtocolVersion {
        /// Version supplied by client, if any.
        found: Option<String>,
    },
}

/// Request identifier used across Effect 4 RPC envelopes (`string | number`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    /// Stringified numeric or UUID request identifier.
    String(String),
    /// Integer request identifier.
    Integer(i64),
}

impl fmt::Display for RpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Integer(n) => write!(f, "{n}"),
        }
    }
}

impl From<&str> for RpcId {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for RpcId {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<i64> for RpcId {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<i32> for RpcId {
    fn from(value: i32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<u32> for RpcId {
    fn from(value: u32) -> Self {
        Self::Integer(i64::from(value))
    }
}

/// Client-to-server frame in Effect 4 `RpcSerialization.layerJson` format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ClientRpcFrame {
    /// RPC request or one-way notification envelope.
    Request {
        /// Unique request correlation identifier.
        id: RpcId,
        /// RPC method name.
        tag: String,
        /// Encoded request parameters (`null` when omitted on wire).
        #[serde(default)]
        payload: Value,
        /// Transport header pairs attached to the call.
        #[serde(default)]
        headers: Vec<(String, String)>,
        /// True when the sender expects no response frames.
        #[serde(
            default,
            rename = "isNotification",
            skip_serializing_if = "std::ops::Not::not"
        )]
        is_notification: bool,
        /// True when the server should acknowledge with `Success` on start and drop results.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        discard: bool,
        /// Optional distributed trace identifier.
        #[serde(default, rename = "traceId", skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
        /// Optional distributed span identifier.
        #[serde(default, rename = "spanId", skip_serializing_if = "Option::is_none")]
        span_id: Option<String>,
        /// Optional sampling decision flag.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sampled: Option<bool>,
    },
    /// Stream flow-control acknowledgement from client to server.
    Ack {
        /// Identifier of the active request being acknowledged.
        #[serde(rename = "requestId")]
        request_id: RpcId,
    },
    /// Request cancellation signal from client to server.
    Interrupt {
        /// Identifier of the active request to interrupt.
        #[serde(rename = "requestId")]
        request_id: RpcId,
    },
    /// Connection keepalive ping.
    Ping,
    /// Client end-of-stream marker.
    Eof,
}

/// Individual failure entry inside an Effect `Failure` cause array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum RpcCauseReason {
    /// Typed application error value.
    Fail {
        /// Encoded typed failure payload.
        #[serde(default)]
        error: Value,
    },
    /// Unexpected defect payload.
    Die {
        /// Encoded defect payload.
        #[serde(default)]
        defect: Value,
    },
    /// Fiber interruption record.
    Interrupt {
        /// Identifier of the interrupting fiber, if known.
        #[serde(default, rename = "fiberId", skip_serializing_if = "Option::is_none")]
        fiber_id: Option<i64>,
    },
}

/// Terminal `Exit` payload carried by [`ServerRpcFrame::Exit`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum RpcExit {
    /// Successful completion carrying encoded return value (`null` when `void`).
    Success {
        /// Encoded return value (`null` if omitted by JS `undefined`).
        #[serde(default)]
        value: Value,
    },
    /// Failure completion carrying structured Effect cause entries.
    Failure {
        /// Failure, defect, and interrupt entries composing the cause.
        cause: Vec<RpcCauseReason>,
    },
}

/// Server-to-client frame in Effect 4 `RpcSerialization.layerJson` format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ServerRpcFrame {
    /// Batch of streamed response items for an open request.
    Chunk {
        /// Identifier of the streaming request.
        #[serde(rename = "requestId")]
        request_id: RpcId,
        /// Encoded chunk values in emission order.
        values: Vec<Value>,
    },
    /// Terminal completion frame for an open request.
    Exit {
        /// Identifier of the completed request.
        #[serde(rename = "requestId")]
        request_id: RpcId,
        /// Terminal exit outcome.
        exit: RpcExit,
    },
    /// Connection-level protocol defect notification.
    Defect {
        /// Encoded defect payload.
        #[serde(default)]
        defect: Value,
    },
    /// Response to [`ClientRpcFrame::Ping`].
    Pong,
}

fn json_kind_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn decode_single_or_batch<T: DeserializeOwned>(raw: &str) -> Result<Vec<T>, RpcCodecError> {
    let parsed: Value = serde_json::from_str(raw)?;
    match parsed {
        Value::Object(_) => Ok(vec![serde_json::from_value(parsed)?]),
        Value::Array(items) => items
            .into_iter()
            .map(|item| serde_json::from_value(item).map_err(RpcCodecError::from))
            .collect(),
        other => Err(RpcCodecError::UnexpectedJsonShape {
            kind: json_kind_label(&other),
        }),
    }
}

/// Decodes either a single JSON object or a JSON batch array into [`ClientRpcFrame`]s.
pub fn decode_client_frames(input: &str) -> Result<Vec<ClientRpcFrame>, RpcCodecError> {
    decode_single_or_batch(input)
}

/// Decodes UTF-8 wire bytes into [`ClientRpcFrame`]s (single object or batch array).
pub fn decode_client_frames_bytes(bytes: &[u8]) -> Result<Vec<ClientRpcFrame>, RpcCodecError> {
    let text = std::str::from_utf8(bytes)?;
    decode_client_frames(text)
}

/// Decodes either a single JSON object or a JSON batch array into [`ServerRpcFrame`]s.
pub fn decode_server_frames(input: &str) -> Result<Vec<ServerRpcFrame>, RpcCodecError> {
    decode_single_or_batch(input)
}

/// Decodes UTF-8 wire bytes into [`ServerRpcFrame`]s (single object or batch array).
pub fn decode_server_frames_bytes(bytes: &[u8]) -> Result<Vec<ServerRpcFrame>, RpcCodecError> {
    let text = std::str::from_utf8(bytes)?;
    decode_server_frames(text)
}

/// Serializes a single [`ClientRpcFrame`] into compact Effect JSON wire format.
pub fn encode_client_frame(frame: &ClientRpcFrame) -> Result<String, RpcCodecError> {
    Ok(serde_json::to_string(frame)?)
}

/// Serializes a slice of [`ClientRpcFrame`]s as a JSON batch array.
pub fn encode_client_frames(frames: &[ClientRpcFrame]) -> Result<String, RpcCodecError> {
    Ok(serde_json::to_string(frames)?)
}

/// Serializes a single [`ServerRpcFrame`] into compact Effect JSON wire format.
pub fn encode_server_frame(frame: &ServerRpcFrame) -> Result<String, RpcCodecError> {
    Ok(serde_json::to_string(frame)?)
}

/// Serializes a slice of [`ServerRpcFrame`]s as a JSON batch array.
pub fn encode_server_frames(frames: &[ServerRpcFrame]) -> Result<String, RpcCodecError> {
    Ok(serde_json::to_string(frames)?)
}

/// Validates the WebSocket `orchestrationProtocol` query parameter.
pub fn validate_orchestration_protocol(version: Option<&str>) -> Result<(), RpcCodecError> {
    match version {
        Some(ORCHESTRATION_PROTOCOL_VERSION) => Ok(()),
        other => Err(RpcCodecError::UnsupportedProtocolVersion {
            found: other.map(str::to_owned),
        }),
    }
}
