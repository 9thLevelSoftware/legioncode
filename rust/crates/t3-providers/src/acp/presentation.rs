//! ACP tool-call state merging, coalescing, and presentation mapping.
//!
//! Normalizes streaming `tool_call` and `tool_call_update` payloads into
//! bounded [`AcpToolCallState`] snapshots and maps them into T3 Code UI
//! [`ToolCallPresentation`] summaries.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use t3_shared::utf16::utf8_len_to_utf16;

use super::render::render_content_preview;
pub use super::render::{ToolActivityCategory, ToolCallPresentation, map_tool_call_presentation};

/// Maximum characters retained in rendered tool-call textual content.
pub const TOOL_CALL_CONTENT_MAX_CHARS: usize = 8_000;

/// Minimum character delta before emitting an intermediate tool-call content update.
pub const TOOL_CALL_MIN_GROWTH_CHARS: usize = 256;

/// Maximum consecutive non-terminal updates coalesced before forcing emission.
pub const TOOL_CALL_MAX_COALESCED_UPDATES: usize = 10;

/// Canonical ACP tool-call lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Queued or awaiting approval.
    Pending,
    /// Actively executing.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Terminated with failure or denial.
    Failed,
    /// Forward-compatible unrecognized status tag.
    #[serde(untagged)]
    Unknown(String),
}

impl ToolCallStatus {
    /// Returns `true` when the tool call has reached a terminal state.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// Semantic category assigned by an ACP agent to a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallKind {
    /// Reading files or directory listings.
    Read,
    /// Modifying existing files or applying patches.
    Edit,
    /// Removing files or directories.
    Delete,
    /// Renaming or relocating files.
    Move,
    /// Codebase or workspace search.
    Search,
    /// Shell or subprocess execution.
    Execute,
    /// Internal reasoning step.
    Think,
    /// Network or web retrieval.
    Fetch,
    /// Agent interaction mode transition.
    SwitchMode,
    /// Generic unclassified tool call.
    Other,
    /// Forward-compatible unrecognized tool kind.
    #[serde(untagged)]
    Unknown(String),
}

/// Source path and optional 1-based line touched by a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallLocation {
    /// Target file or resource path.
    pub path: String,
    /// Optional 1-based line number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

/// Extracted MCP server/tool identifier when a tool call targets an MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct McpToolIdentity {
    /// MCP server name.
    pub server: String,
    /// Tool name on that server.
    pub tool: String,
}

/// Extracts `(server, tool)` from common MCP tool naming conventions.
///
/// Recognizes `mcp:<server>:<tool>`, `mcp__<server>__<tool>`, and `<server>/<tool>`.
///
/// # Examples
///
/// ```
/// use t3_providers::acp::extract_mcp_tool_identity;
///
/// let parsed = extract_mcp_tool_identity("mcp__github__list_pull_requests")
///     .expect("valid mcp double-underscore identifier");
/// assert_eq!(parsed.server, "github");
/// assert_eq!(parsed.tool, "list_pull_requests");
/// assert!(extract_mcp_tool_identity("bash").is_none());
/// ```
#[must_use]
pub fn extract_mcp_tool_identity(raw_name: &str) -> Option<McpToolIdentity> {
    let trimmed = raw_name.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix("mcp:") {
        let (server, tool) = rest.split_once(':')?;
        return build_mcp_identity(server, tool);
    }

    if let Some(rest) = trimmed.strip_prefix("mcp__") {
        let (server, tool) = rest.split_once("__")?;
        return build_mcp_identity(server, tool);
    }

    if let Some((server, tool)) = trimmed.split_once('/') {
        let valid_bare_slash = !trimmed.starts_with('/')
            && !trimmed.contains('\\')
            && !server.contains('.')
            && !tool.contains(['.', '/']);
        if valid_bare_slash {
            return build_mcp_identity(server, tool);
        }
    }

    None
}

pub(super) fn build_mcp_identity(server: &str, tool: &str) -> Option<McpToolIdentity> {
    let server = server.trim();
    let tool = tool.trim();
    if server.is_empty()
        || tool.is_empty()
        || server.chars().any(char::is_whitespace)
        || tool.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(McpToolIdentity {
        server: server.to_owned(),
        tool: tool.to_owned(),
    })
}

/// Incremental update payload delivered inside ACP `tool_call` or `tool_call_update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdatePayload {
    /// Stable tool-call identifier within the session.
    pub tool_call_id: String,
    /// Updated display title, if non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Semantic tool kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolCallKind>,
    /// Current lifecycle status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    /// File or URI locations touched by this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<ToolCallLocation>>,
    /// Rendered output/diff/terminal content items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Value>>,
    /// Raw structured tool arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    /// Raw structured tool output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
}

/// Merged tool-call state accumulated across ACP streaming notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpToolCallState {
    /// Stable tool-call identifier.
    pub tool_call_id: String,
    /// Best known human-readable tool title.
    pub title: String,
    /// Best known tool kind.
    pub kind: ToolCallKind,
    /// Latest lifecycle status.
    pub status: ToolCallStatus,
    /// Last non-empty list of affected file locations.
    pub locations: Vec<ToolCallLocation>,
    /// Accumulated text preview capped at [`TOOL_CALL_CONTENT_MAX_CHARS`].
    pub content_preview: String,
    /// Latest raw input arguments if supplied.
    pub raw_input: Option<Value>,
    /// Latest raw output payload if supplied.
    pub raw_output: Option<Value>,
    /// Total updates merged into this state.
    pub update_count: usize,
    /// UTF-16 length of `content_preview` when last emitted to UI listeners.
    pub last_emitted_chars: usize,
}

impl AcpToolCallState {
    /// Creates an initial [`AcpToolCallState`] from the first `tool_call` or `tool_call_update`.
    #[must_use]
    pub fn from_payload(payload: ToolCallUpdatePayload) -> Self {
        let content_preview = payload
            .content
            .as_deref()
            .map_or_default(render_content_preview);
        let last_emitted_chars = utf8_len_to_utf16(&content_preview);
        Self {
            tool_call_id: payload.tool_call_id,
            title: payload
                .title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Tool call".to_owned()),
            kind: payload.kind.unwrap_or(ToolCallKind::Other),
            status: payload.status.unwrap_or(ToolCallStatus::Pending),
            locations: payload.locations.unwrap_or_default(),
            content_preview,
            raw_input: payload.raw_input,
            raw_output: payload.raw_output,
            update_count: 1,
            last_emitted_chars,
        }
    }

    /// Merges an incremental [`ToolCallUpdatePayload`] while preserving non-empty fields.
    ///
    /// Returns `true` when the caller should emit a downstream progress update based
    /// on terminal status changes, content growth thresholds, or coalescing limits.
    pub fn merge(&mut self, update: ToolCallUpdatePayload) -> bool {
        let prev_status = self.status.clone();
        self.update_count = self.update_count.saturating_add(1);

        if let Some(title) = update.title
            && !title.trim().is_empty()
        {
            self.title = title;
        }
        if let Some(kind) = update.kind {
            self.kind = kind;
        }
        if let Some(status) = update.status {
            self.status = status;
        }
        if let Some(locations) = update.locations
            && !locations.is_empty()
        {
            self.locations = locations;
        }
        if let Some(content) = update.content {
            let rendered = render_content_preview(&content);
            if !rendered.is_empty() {
                self.content_preview = rendered;
            }
        }
        if let Some(raw_input) = update.raw_input {
            self.raw_input = Some(raw_input);
        }
        if let Some(raw_output) = update.raw_output {
            self.raw_output = Some(raw_output);
        }

        let current_chars = utf8_len_to_utf16(&self.content_preview);
        let status_changed = prev_status != self.status || self.status.is_terminal();
        let emit = should_emit_tool_call_update(
            self.last_emitted_chars,
            current_chars,
            self.update_count,
            status_changed,
        );
        if emit {
            self.last_emitted_chars = current_chars;
        }
        emit
    }
}

/// Decides whether an intermediate tool-call update should be pushed to UI subscribers.
#[must_use]
pub const fn should_emit_tool_call_update(
    prev_chars: usize,
    next_chars: usize,
    update_count: usize,
    status_changed: bool,
) -> bool {
    if status_changed {
        return true;
    }
    if next_chars.saturating_sub(prev_chars) >= TOOL_CALL_MIN_GROWTH_CHARS {
        return true;
    }
    update_count.is_multiple_of(TOOL_CALL_MAX_COALESCED_UPDATES)
}
