//! JSON-RPC 2.0 envelopes and stdio NDJSON framing for ACP providers.
//!
//! Built on [`legion_protocol::provider_env::NdjsonLineCodec`] so all ACP and
//! app-server drivers share strict 128 MiB bounded line buffering, UTF-8
//! validation, and partial-frame handling across reads.

use legion_protocol::provider_env::{NdjsonCodecError, NdjsonLineCodec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Initial JSON-RPC numeric request ID (`2^30`), avoiding collisions with
/// low agent-initiated IDs on bidirectional transports.
pub const RPC_REQUEST_ID_START: i64 = 1_073_741_824;

/// Required JSON-RPC protocol version tag.
pub const JSON_RPC_VERSION: &str = "2.0";

/// Errors raised while encoding or decoding ACP JSON-RPC NDJSON traffic.
#[derive(Debug, thiserror::Error)]
pub enum AcpCodecError {
    /// Underlying NDJSON framing or UTF-8 decode failure.
    #[error("NDJSON transport error: {0}")]
    Ndjson(#[from] NdjsonCodecError),
    /// JSON payload serialization or deserialization failure.
    #[error("JSON-RPC payload error: {0}")]
    Json(#[from] serde_json::Error),
    /// Missing or unsupported `"jsonrpc"` field (must equal `"2.0"`).
    #[error("invalid JSON-RPC version: expected \"2.0\", got {found:?}")]
    InvalidVersion {
        /// Version tag found in the wire object, if any.
        found: Option<String>,
    },
    /// Envelope did not match request, response, or notification structure.
    #[error("malformed JSON-RPC envelope: {reason}")]
    MalformedEnvelope {
        /// Specific structural defect detected.
        reason: &'static str,
    },
}

/// JSON-RPC 2.0 request/response correlation identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// Signed 64-bit integer request ID.
    Number(i64),
    /// String request ID.
    String(String),
}

impl std::fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(id) => write!(f, "{id}"),
            Self::String(id) => f.write_str(id),
        }
    }
}

/// JSON-RPC 2.0 error payload carried on failed responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    /// Standard or provider-defined JSON-RPC error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Outbound or inbound JSON-RPC 2.0 request message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version string (always `"2.0"`).
    pub jsonrpc: String,
    /// Correlation identifier paired with the eventual response.
    pub id: JsonRpcId,
    /// RPC method name (for example `"initialize"` or `"session/prompt"`).
    pub method: String,
    /// Optional parameters object or array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 response message carrying either `result` or `error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version string (always `"2.0"`).
    pub jsonrpc: String,
    /// Matching request identifier.
    pub id: JsonRpcId,
    /// Successful response payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload when the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObject>,
}

/// One-way JSON-RPC 2.0 notification (no `id` field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Protocol version string (always `"2.0"`).
    pub jsonrpc: String,
    /// Notification method name (for example `"session/update"` or `"session/cancel"`).
    pub method: String,
    /// Optional parameters object or array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Validated JSON-RPC 2.0 wire envelope decoded from one NDJSON line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonRpcEnvelope {
    /// Request expecting a correlated [`JsonRpcResponse`].
    Request(JsonRpcRequest),
    /// Response to an earlier request.
    Response(JsonRpcResponse),
    /// Fire-and-forget notification.
    Notification(JsonRpcNotification),
}

impl JsonRpcEnvelope {
    /// Decodes and validates a single JSON-RPC 2.0 message from a [`Value`].
    pub fn from_value(value: Value) -> Result<Self, AcpCodecError> {
        let obj = value.as_object().ok_or(AcpCodecError::MalformedEnvelope {
            reason: "JSON-RPC message must be a JSON object",
        })?;

        let version = obj.get("jsonrpc").and_then(Value::as_str);
        if version != Some(JSON_RPC_VERSION) {
            return Err(AcpCodecError::InvalidVersion {
                found: version.map(ToOwned::to_owned),
            });
        }

        let has_id = obj.get("id").is_some_and(|v| !v.is_null());
        let has_method = obj.contains_key("method");
        let has_result = obj.contains_key("result");
        let has_error = obj.contains_key("error");

        if has_id && has_method {
            return Ok(Self::Request(serde_json::from_value(value)?));
        }
        if has_id && (has_result || has_error) && !has_method {
            return Ok(Self::Response(serde_json::from_value(value)?));
        }
        if !has_id && has_method && !has_result && !has_error {
            return Ok(Self::Notification(serde_json::from_value(value)?));
        }

        Err(AcpCodecError::MalformedEnvelope {
            reason: "object does not match request, response, or notification shape",
        })
    }

    /// Serializes the envelope back into a [`JsonRpcRequest`], [`JsonRpcResponse`], or [`JsonRpcNotification`] value.
    pub fn to_value(&self) -> Result<Value, AcpCodecError> {
        Ok(match self {
            Self::Request(req) => serde_json::to_value(req)?,
            Self::Response(resp) => serde_json::to_value(resp)?,
            Self::Notification(notif) => serde_json::to_value(notif)?,
        })
    }
}

/// Stateful stdio NDJSON codec and request ID sequencer for ACP transports.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use legion_providers::acp::{AcpStdioCodec, JsonRpcEnvelope, RPC_REQUEST_ID_START};
///
/// let mut codec = AcpStdioCodec::new();
/// let (id, frame) = codec.encode_request("initialize", Some(&json!({"protocolVersion": 1})))?;
/// assert_eq!(id, legion_providers::acp::JsonRpcId::Number(RPC_REQUEST_ID_START));
///
/// let decoded = codec.push_bytes(frame.as_bytes())?;
/// assert_eq!(decoded.len(), 1);
/// assert!(matches!(decoded[0], JsonRpcEnvelope::Request(_)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct AcpStdioCodec {
    ndjson: NdjsonLineCodec,
    next_id: i64,
}

impl Default for AcpStdioCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpStdioCodec {
    /// Creates a new codec using [`legion_protocol::provider_env::MAX_NDJSON_LINE_BYTES`]
    /// and starting request IDs at [`RPC_REQUEST_ID_START`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            ndjson: NdjsonLineCodec::default(),
            next_id: RPC_REQUEST_ID_START,
        }
    }

    /// Creates a new codec with a custom maximum NDJSON line length in bytes.
    #[must_use]
    pub fn with_max_line_bytes(max_line_bytes: usize) -> Self {
        Self {
            ndjson: NdjsonLineCodec::new(max_line_bytes),
            next_id: RPC_REQUEST_ID_START,
        }
    }

    /// Allocates and returns the next monotonically increasing numeric [`JsonRpcId`].
    pub fn next_request_id(&mut self) -> JsonRpcId {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        JsonRpcId::Number(id)
    }

    /// Builds a validated [`JsonRpcRequest`] with a newly allocated [`JsonRpcId`].
    pub fn build_request<P: Serialize>(
        &mut self,
        method: impl Into<String>,
        params: Option<&P>,
    ) -> Result<JsonRpcRequest, AcpCodecError> {
        let params = params.map(serde_json::to_value).transpose()?;
        Ok(JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: self.next_request_id(),
            method: method.into(),
            params,
        })
    }

    /// Encodes a new JSON-RPC request as an NDJSON line (`...\n`) and returns its ID.
    pub fn encode_request<P: Serialize>(
        &mut self,
        method: impl Into<String>,
        params: Option<&P>,
    ) -> Result<(JsonRpcId, String), AcpCodecError> {
        let req = self.build_request(method, params)?;
        let id = req.id.clone();
        let line = self.ndjson.encode_line(&req)?;
        Ok((id, line))
    }

    /// Encodes a JSON-RPC notification as an NDJSON line (`...\n`).
    pub fn encode_notification<P: Serialize>(
        &self,
        method: impl Into<String>,
        params: Option<&P>,
    ) -> Result<String, AcpCodecError> {
        let notif = JsonRpcNotification {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: method.into(),
            params: params.map(serde_json::to_value).transpose()?,
        };
        Ok(self.ndjson.encode_line(&notif)?)
    }

    /// Encodes a successful JSON-RPC response as an NDJSON line (`...\n`).
    pub fn encode_response_ok(
        &self,
        id: JsonRpcId,
        result: Value,
    ) -> Result<String, AcpCodecError> {
        let resp = JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id,
            result: Some(result),
            error: None,
        };
        Ok(self.ndjson.encode_line(&resp)?)
    }

    /// Encodes an error JSON-RPC response as an NDJSON line (`...\n`).
    pub fn encode_response_err(
        &self,
        id: JsonRpcId,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Result<String, AcpCodecError> {
        let resp = JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id,
            result: None,
            error: Some(JsonRpcErrorObject {
                code,
                message: message.into(),
                data,
            }),
        };
        Ok(self.ndjson.encode_line(&resp)?)
    }

    /// Encodes any [`JsonRpcEnvelope`] as a newline-terminated NDJSON string.
    pub fn encode_envelope(&self, envelope: &JsonRpcEnvelope) -> Result<String, AcpCodecError> {
        match envelope {
            JsonRpcEnvelope::Request(req) => Ok(self.ndjson.encode_line(req)?),
            JsonRpcEnvelope::Response(resp) => Ok(self.ndjson.encode_line(resp)?),
            JsonRpcEnvelope::Notification(notif) => Ok(self.ndjson.encode_line(notif)?),
        }
    }

    /// Feeds raw stdout bytes from an ACP subprocess and returns all complete envelopes.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<JsonRpcEnvelope>, AcpCodecError> {
        let values: Vec<Value> = self.ndjson.push_json_lines(bytes)?;
        values
            .into_iter()
            .map(JsonRpcEnvelope::from_value)
            .collect()
    }

    /// Flushes any trailing unterminated NDJSON line when the child stdout hits EOF.
    pub fn finish(&mut self) -> Result<Option<JsonRpcEnvelope>, AcpCodecError> {
        let Some(line) = self.ndjson.finish()? else {
            return Ok(None);
        };
        let value: Value = serde_json::from_str(&line)?;
        JsonRpcEnvelope::from_value(value).map(Some)
    }

    /// Returns the number of buffered incomplete bytes waiting for `\n`.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.ndjson.pending_bytes()
    }
}
