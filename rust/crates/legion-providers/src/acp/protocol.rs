//! Typed ACP JSON-RPC methods, session update variants, and permission policy.
//!
//! Defines strongly typed wire structures for `initialize`, `session/new`,
//! `session/prompt`, `session/cancel`, `session/update`, and
//! `session/request_permission` alongside [`select_permission_option`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::codec::{AcpCodecError, JSON_RPC_VERSION, JsonRpcEnvelope, JsonRpcNotification};
pub use super::permissions::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, select_permission_option,
};
pub use super::updates::{AvailableCommand, SessionUpdateKind, SessionUpdateNotification};

/// ACP protocol version supported by `LegionCode` adapters.
pub const ACP_PROTOCOL_VERSION: u32 = 1;

/// ACP `initialize` method name.
pub const METHOD_INITIALIZE: &str = "initialize";
/// ACP `session/new` method name.
pub const METHOD_SESSION_NEW: &str = "session/new";
/// ACP `session/load` method name.
pub const METHOD_SESSION_LOAD: &str = "session/load";
/// ACP `session/resume` method name.
pub const METHOD_SESSION_RESUME: &str = "session/resume";
/// ACP `session/fork` method name.
pub const METHOD_SESSION_FORK: &str = "session/fork";
/// ACP `session/prompt` method name.
pub const METHOD_SESSION_PROMPT: &str = "session/prompt";
/// ACP `session/cancel` method name.
pub const METHOD_SESSION_CANCEL: &str = "session/cancel";
/// ACP `session/update` notification method name.
pub const METHOD_SESSION_UPDATE: &str = "session/update";
/// ACP `session/request_permission` agent-to-client request method name.
pub const METHOD_SESSION_REQUEST_PERMISSION: &str = "session/request_permission";

/// Client filesystem capabilities advertised during `initialize`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemCapabilities {
    /// Whether the client allows `fs/read_text_file`.
    pub read_text_file: bool,
    /// Whether the client allows `fs/write_text_file`.
    pub write_text_file: bool,
}

/// Client capabilities advertised during `initialize`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Filesystem read/write capabilities.
    #[serde(default)]
    pub fs: FileSystemCapabilities,
    /// Whether the client hosts terminal execution RPCs.
    #[serde(default)]
    pub terminal: bool,
}

/// Client or agent implementation metadata block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementationInfo {
    /// Program identifier (for example `"legioncode"`).
    pub name: String,
    /// Optional human-readable display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Semantic version string.
    pub version: String,
}

/// Request parameters for ACP `initialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    /// Negotiated ACP protocol version (typically `1`).
    pub protocol_version: u32,
    /// Capabilities exposed by the client harness.
    pub client_capabilities: ClientCapabilities,
    /// Client implementation identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<ImplementationInfo>,
    /// Optional provider-specific `_meta` extensions.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Result payload returned from ACP `initialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    /// Negotiated ACP protocol version.
    pub protocol_version: u32,
    /// Optional agent identity metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<ImplementationInfo>,
    /// Capabilities advertised by the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_capabilities: Option<Value>,
    /// Supported authentication methods.
    #[serde(default)]
    pub auth_methods: Vec<Value>,
    /// Optional session config option descriptors.
    #[serde(default)]
    pub config_options: Vec<Value>,
    /// Optional provider `_meta` extensions (such as Grok slash commands).
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Environment entry passed inside stdio MCP server configs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvNameValue {
    /// Variable name.
    pub name: String,
    /// Variable value.
    pub value: String,
}

/// HTTP header entry passed inside HTTP/SSE MCP server configs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHeaderEntry {
    /// Header name.
    pub name: String,
    /// Header value.
    pub value: String,
}

/// MCP server transport definition forwarded on `session/new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerSpec {
    /// Local child process talking MCP over stdin/stdout.
    Stdio {
        /// Unique server name.
        name: String,
        /// Executable path or command.
        command: String,
        /// Argument list.
        #[serde(default)]
        args: Vec<String>,
        /// Environment variables supplied to the MCP process.
        #[serde(default)]
        env: Vec<EnvNameValue>,
    },
    /// Streamable HTTP MCP endpoint.
    Http {
        /// Unique server name.
        name: String,
        /// Endpoint URL.
        url: String,
        /// HTTP request headers.
        #[serde(default)]
        headers: Vec<HttpHeaderEntry>,
    },
    /// Legacy Server-Sent Events MCP endpoint.
    Sse {
        /// Unique server name.
        name: String,
        /// Endpoint URL.
        url: String,
        /// HTTP request headers.
        #[serde(default)]
        headers: Vec<HttpHeaderEntry>,
    },
}

/// Request parameters for ACP `session/new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionRequest {
    /// Working directory for the agent session.
    pub cwd: String,
    /// Configured MCP servers available during the session.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerSpec>,
    /// Optional provider `_meta` extensions.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Result payload returned from ACP `session/new` / `session/load`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResponse {
    /// Provider-assigned session identifier.
    pub session_id: String,
    /// Available and active mode state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<Value>,
    /// Available and active model state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Value>,
    /// Session-scoped configuration options.
    #[serde(default)]
    pub config_options: Vec<Value>,
    /// Optional provider `_meta` block.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Prompt content block transmitted inside `session/prompt` or chunk updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptContentBlock {
    /// Plain text or Markdown segment.
    Text {
        /// Text content.
        text: String,
    },
    /// Base64-encoded image attachment.
    Image {
        /// MIME media type (for example `"image/png"`).
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// Base64 image payload.
        data: String,
    },
    /// Workspace or URI resource reference.
    ResourceLink {
        /// Target URI.
        uri: String,
        /// Display name.
        name: String,
        /// Optional MIME type.
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    /// Embedded resource payload preserved verbatim.
    Resource {
        /// Embedded resource JSON object.
        resource: Value,
    },
}

/// Request parameters for ACP `session/prompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    /// Active ACP session ID.
    pub session_id: String,
    /// Ordered prompt blocks supplied for the turn.
    pub prompt: Vec<PromptContentBlock>,
    /// Optional provider `_meta` extensions.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Reason reported by the agent when finishing a `session/prompt` turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Normal model completion.
    EndTurn,
    /// Output token cap reached.
    MaxTokens,
    /// Internal agent turn-loop ceiling reached.
    MaxTurnRequests,
    /// Model safety/policy refusal.
    Refusal,
    /// Aborted via `session/cancel`.
    Cancelled,
    /// Unrecognized future stop reason preserved as string.
    #[serde(untagged)]
    Unknown(String),
}

/// Result payload returned from ACP `session/prompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    /// Turn termination cause.
    pub stop_reason: StopReason,
    /// Optional token usage summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    /// Optional provider `_meta` extensions.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Notification parameters for ACP `session/cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelNotification {
    /// Active ACP session ID to interrupt.
    pub session_id: String,
    /// Optional provider metadata (such as Grok `{ "cancelTrigger": "ctrl_c" }`).
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<BTreeMap<String, Value>>,
}

impl CancelNotification {
    /// Converts this cancellation into a JSON-RPC 2.0 wire notification.
    pub fn into_envelope(self) -> Result<JsonRpcEnvelope, AcpCodecError> {
        Ok(JsonRpcEnvelope::Notification(JsonRpcNotification {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: METHOD_SESSION_CANCEL.to_owned(),
            params: Some(serde_json::to_value(self)?),
        }))
    }
}
