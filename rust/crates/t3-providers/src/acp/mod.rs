//! Agent Client Protocol (ACP) JSON-RPC 2.0 wire protocol, stdio framing, and
//! tool-call presentation normalization.
//!
//! All NDJSON transport framing delegates to
//! [`t3_protocol::provider_env::NdjsonLineCodec`] via [`AcpStdioCodec`].

pub mod codec;
pub mod permissions;
pub mod presentation;
pub mod protocol;
pub mod render;
pub mod updates;

pub use codec::{
    AcpCodecError, AcpStdioCodec, JSON_RPC_VERSION, JsonRpcEnvelope, JsonRpcErrorObject, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RPC_REQUEST_ID_START,
};
pub use permissions::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, select_permission_option,
};
pub use presentation::{
    AcpToolCallState, McpToolIdentity, TOOL_CALL_CONTENT_MAX_CHARS,
    TOOL_CALL_MAX_COALESCED_UPDATES, TOOL_CALL_MIN_GROWTH_CHARS, ToolCallKind, ToolCallLocation,
    ToolCallStatus, ToolCallUpdatePayload, extract_mcp_tool_identity, should_emit_tool_call_update,
};
pub use protocol::{
    ACP_PROTOCOL_VERSION, CancelNotification, ClientCapabilities, EnvNameValue,
    FileSystemCapabilities, HttpHeaderEntry, ImplementationInfo, InitializeRequest,
    InitializeResponse, METHOD_INITIALIZE, METHOD_SESSION_CANCEL, METHOD_SESSION_FORK,
    METHOD_SESSION_LOAD, METHOD_SESSION_NEW, METHOD_SESSION_PROMPT,
    METHOD_SESSION_REQUEST_PERMISSION, METHOD_SESSION_RESUME, METHOD_SESSION_UPDATE, McpServerSpec,
    NewSessionRequest, NewSessionResponse, PromptContentBlock, PromptRequest, PromptResponse,
    StopReason,
};
pub use render::{ToolActivityCategory, ToolCallPresentation, map_tool_call_presentation};
pub use updates::{AvailableCommand, SessionUpdateKind, SessionUpdateNotification};
