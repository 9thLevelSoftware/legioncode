//! Native protocol codecs and HTTP/SSE specs for `LegionCode`'s non-ACP providers.
//!
//! Normalizes streaming events from `codex` app-server JSON-RPC, `claude`
//! `stream-json`, `cursor` sidecar NDJSON, `pi` `--mode json`, `opencode`
//! HTTP/SSE, and xAI `grok` HTTP/SSE into [`NonAcpTurnUpdate`].

pub mod claude_stream_json;
pub mod codex_app_server;
mod codex_parser;
pub mod cursor_and_pi;
pub mod grok_http;
pub mod opencode_sse;
pub mod sse;

use indexmap::IndexMap;
use legion_protocol::provider_env::NdjsonCodecError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use claude_stream_json::{
    ClaudeContentBlock, build_claude_control_response, build_claude_interrupt_control_request,
    build_claude_permission_control_response, build_claude_user_prompt_envelope,
    encode_claude_user_prompt_line, parse_claude_stream_json_line,
};
pub use codex_app_server::{
    CodexAppServerCodec, CodexApprovalDecision, CodexThreadStartParams, CodexTurnStartParams,
    codex_decision_from_approval, encode_apply_patch_approval_response,
    encode_exec_command_approval_response, parse_codex_app_server_line,
};
pub use cursor_and_pi::{
    build_cursor_cancel_request, build_cursor_send_request, build_pi_abort_command,
    build_pi_prompt_command, parse_cursor_sidecar_line, parse_pi_json_line,
};
pub use grok_http::{
    DEFAULT_XAI_API_BASE, GrokChatMessage, build_grok_chat_completions_request,
    build_grok_models_request, parse_grok_models_response, parse_grok_sse_chunk,
};
pub use opencode_sse::{
    SseEventFrame, SseStreamParser, build_opencode_abort_request,
    build_opencode_create_session_request, build_opencode_event_stream_request,
    build_opencode_permission_reply_request, build_opencode_prompt_request,
    parse_opencode_sse_data,
};

/// Errors raised when framing, validating, or decoding non-ACP provider protocols.
#[derive(Debug, thiserror::Error)]
pub enum NonAcpParseError {
    /// Underlying NDJSON buffer or UTF-8 decode error.
    #[error("NDJSON transport error: {0}")]
    Ndjson(#[from] NdjsonCodecError),
    /// Invalid JSON syntax or unexpected JSON field type.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// Wire stream contained invalid UTF-8 bytes.
    #[error("invalid UTF-8 in provider stream: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    /// Protocol-level structural invariant failed.
    #[error("malformed non-ACP frame: {reason}")]
    MalformedFrame {
        /// Human-readable reason explaining the structural mismatch.
        reason: &'static str,
    },
}

/// Lifecycle state reported for a non-ACP tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonAcpToolStatus {
    /// Tool execution was initiated.
    Started,
    /// Tool is running or streaming incremental output.
    InProgress,
    /// Tool completed without error.
    Completed,
    /// Tool failed or was denied/cancelled.
    Failed,
}

/// Normalized streaming event emitted across all 6 non-ACP provider codecs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NonAcpTurnUpdate {
    /// Provider session/thread established or reported active model.
    SessionStarted {
        /// Provider-native session or thread identifier.
        provider_session_id: String,
        /// Reported model identifier if available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Incremental assistant text output delta.
    AssistantTextDelta {
        /// Text delta chunk.
        text: String,
    },
    /// Incremental reasoning or thinking summary delta.
    ReasoningDelta {
        /// Reasoning text delta chunk.
        text: String,
    },
    /// Tool call started with initial arguments.
    ToolCallStarted {
        /// Provider-assigned tool call identifier.
        call_id: String,
        /// Tool or command name.
        name: String,
        /// Structured tool arguments JSON.
        input_json: Value,
    },
    /// Tool call finished or produced structured completion output.
    ToolCallCompleted {
        /// Provider-assigned tool call identifier matching `ToolCallStarted`.
        call_id: String,
        /// Structured output JSON payload.
        output_json: Value,
        /// Whether the tool ended with an error.
        is_error: bool,
    },
    /// Agent paused and requested interactive tool or plan approval.
    PermissionRequested {
        /// Request identifier used to correlate the client reply.
        request_id: String,
        /// Target tool or approval operation name.
        tool_name: String,
        /// Detailed arguments, diff, or command preview JSON.
        detail: Value,
    },
    /// Cumulative or step token usage counters reported by the provider.
    UsageReported {
        /// Input/prompt tokens consumed.
        input_tokens: u64,
        /// Output/completion tokens generated.
        output_tokens: u64,
    },
    /// Turn completed gracefully.
    TurnCompleted {
        /// Optional provider-supplied stop reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    /// Turn terminated with a failure.
    TurnFailed {
        /// Redacted or raw failure diagnostic message.
        message: String,
    },
}

/// Self-contained HTTP request blueprint shared by HTTP/SSE provider drivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NonAcpHttpRequestSpec {
    /// HTTP verb (`"GET"`, `"POST"`, etc.).
    pub method: String,
    /// Full destination endpoint URL.
    pub url: String,
    /// Ordered HTTP headers including authentication and content type.
    pub headers: IndexMap<String, String>,
    /// Optional JSON request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

pub(crate) fn str_field<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a str> {
    value.and_then(|v| v.get(key)).and_then(Value::as_str)
}

pub(crate) fn u64_field(value: Option<&Value>, key: &str) -> Option<u64> {
    value.and_then(|v| v.get(key)).and_then(Value::as_u64)
}

pub(crate) fn id_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}
