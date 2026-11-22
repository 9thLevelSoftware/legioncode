//! UI presentation bucket mapping and bounded preview rendering for ACP tool calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use t3_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};

use super::presentation::{
    AcpToolCallState, McpToolIdentity, TOOL_CALL_CONTENT_MAX_CHARS, ToolCallKind, ToolCallStatus,
    build_mcp_identity, extract_mcp_tool_identity,
};

/// Semantic UI presentation bucket aligned with `t3_contracts::orchestration::KNOWN_TURN_ITEM_TYPES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolActivityCategory {
    /// Shell or subprocess command execution (`"command_execution"`).
    CommandExecution,
    /// File creation, edit, move, or deletion (`"file_change"`).
    FileChange,
    /// File or codebase search (`"file_search"`).
    FileSearch,
    /// Web fetch or web search (`"web_search"`).
    WebSearch,
    /// Internal reasoning step (`"reasoning"`).
    Reasoning,
    /// MCP server tool invocation (`"dynamic_tool"`).
    McpTool,
    /// Generic dynamic tool invocation (`"dynamic_tool"`).
    DynamicTool,
}

impl ToolActivityCategory {
    /// Returns the matching discriminant in `t3_contracts::orchestration::KNOWN_TURN_ITEM_TYPES`.
    #[must_use]
    pub const fn turn_item_type(self) -> &'static str {
        match self {
            Self::CommandExecution => "command_execution",
            Self::FileChange => "file_change",
            Self::FileSearch => "file_search",
            Self::WebSearch => "web_search",
            Self::Reasoning => "reasoning",
            Self::McpTool | Self::DynamicTool => "dynamic_tool",
        }
    }
}

/// Normalized UI presentation derived from an [`AcpToolCallState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallPresentation {
    /// Canonical T3 activity bucket.
    pub category: ToolActivityCategory,
    /// Matching wire `TurnItem` `"type"` discriminant (`"file_change"`, `"command_execution"`, etc.).
    pub turn_item_type: &'static str,
    /// Concise primary label.
    pub title: String,
    /// Optional secondary label (file path or MCP `server/tool`).
    pub subtitle: Option<String>,
    /// Optional bounded output/content preview.
    pub preview: Option<String>,
    /// Extracted MCP server/tool pair when applicable.
    pub mcp: Option<McpToolIdentity>,
    /// Whether the tool call ended with failure.
    pub failed: bool,
}

/// Maps an [`AcpToolCallState`] to a [`ToolCallPresentation`] ready for client rendering.
///
/// # Examples
///
/// ```
/// use t3_contracts::orchestration::is_known_turn_item_type;
/// use t3_providers::acp::{
///     AcpToolCallState, ToolActivityCategory, ToolCallKind, ToolCallLocation,
///     ToolCallStatus, ToolCallUpdatePayload, map_tool_call_presentation,
/// };
///
/// let state = AcpToolCallState::from_payload(ToolCallUpdatePayload {
///     tool_call_id: "tc-1".to_owned(),
///     title: Some("Edit src/lib.rs".to_owned()),
///     kind: Some(ToolCallKind::Edit),
///     status: Some(ToolCallStatus::Completed),
///     locations: Some(vec![ToolCallLocation { path: "src/lib.rs".to_owned(), line: Some(12) }]),
///     content: None,
///     raw_input: None,
///     raw_output: None,
/// });
/// let view = map_tool_call_presentation(&state);
/// assert_eq!(view.category, ToolActivityCategory::FileChange);
/// assert!(is_known_turn_item_type(view.turn_item_type));
/// assert_eq!(view.subtitle.as_deref(), Some("src/lib.rs:12"));
/// ```
#[must_use]
pub fn map_tool_call_presentation(state: &AcpToolCallState) -> ToolCallPresentation {
    let mcp_target = extract_mcp_tool_call_identity(&state.title, state.raw_input.as_ref(), None);
    let mcp = mcp_target.as_ref().map(|target| target.identity.clone());
    let category = if mcp.is_some() {
        ToolActivityCategory::McpTool
    } else {
        match state.kind {
            ToolCallKind::Execute => ToolActivityCategory::CommandExecution,
            ToolCallKind::Edit | ToolCallKind::Delete | ToolCallKind::Move => {
                ToolActivityCategory::FileChange
            }
            ToolCallKind::Read | ToolCallKind::Search => ToolActivityCategory::FileSearch,
            ToolCallKind::Fetch => ToolActivityCategory::WebSearch,
            ToolCallKind::Think => ToolActivityCategory::Reasoning,
            ToolCallKind::SwitchMode | ToolCallKind::Other | ToolCallKind::Unknown(_) => {
                ToolActivityCategory::DynamicTool
            }
        }
    };

    let subtitle = state
        .locations
        .first()
        .map(|loc| match loc.line {
            Some(line) => format!("{}:{line}", loc.path),
            None => loc.path.clone(),
        })
        .or_else(|| mcp.as_ref().map(|m| format!("{}/{}", m.server, m.tool)));

    let preview = (!state.content_preview.is_empty()).then(|| state.content_preview.clone());
    ToolCallPresentation {
        category,
        turn_item_type: category.turn_item_type(),
        title: state.title.clone(),
        subtitle,
        preview,
        mcp,
        failed: state.status == ToolCallStatus::Failed,
    }
}

pub(super) fn render_content_preview(items: &[Value]) -> String {
    let mut buf = String::new();
    for item in items {
        let piece = extract_item_text(item);
        if piece.is_empty() {
            continue;
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(&piece);
        if utf8_len_to_utf16(&buf) >= TOOL_CALL_CONTENT_MAX_CHARS {
            break;
        }
    }
    if utf8_len_to_utf16(&buf) > TOOL_CALL_CONTENT_MAX_CHARS {
        utf16_slice_safe(&buf, 0, TOOL_CALL_CONTENT_MAX_CHARS).to_owned()
    } else {
        buf
    }
}

fn extract_item_text(item: &Value) -> String {
    if let Some(s) = item.as_str() {
        return s.to_owned();
    }
    let Some(obj) = item.as_object() else {
        return String::new();
    };
    if let Some(text) = obj
        .get("content")
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
    {
        return text.to_owned();
    }
    if let Some(text) = obj.get("text").and_then(Value::as_str) {
        return text.to_owned();
    }
    if let Some(path) = obj.get("path").and_then(Value::as_str) {
        return path.to_owned();
    }
    String::new()
}

/// Extracted MCP tool identity plus structured arguments when supplied (`7f928416b2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolCallTarget {
    /// Target `(server, tool)` identity.
    pub identity: McpToolIdentity,
    /// Structured MCP tool arguments when recovered from `rawInput`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
}

/// Extracts MCP identity from metadata (`_meta.is_mcp_tool_call`), Antigravity's
/// `call_mcp_tool` `rawInput.{ServerName, ToolName, Arguments}` (`7f928416b2`),
/// Kiro's `_meta.kiro.serverName` + `rawInput.tool_id`, or common title conventions.
#[must_use]
pub fn extract_mcp_tool_call_identity(
    title: &str,
    raw_input: Option<&Value>,
    meta: Option<&Value>,
) -> Option<McpToolCallTarget> {
    if let Some(meta_obj) = meta.and_then(Value::as_object) {
        let is_mcp = meta_obj.get("is_mcp_tool_call").and_then(Value::as_bool) == Some(true);
        if is_mcp
            && let (Some(server), Some(tool)) = (
                meta_obj.get("mcp_server_name").and_then(Value::as_str),
                meta_obj.get("mcp_tool_name").and_then(Value::as_str),
            )
            && let Some(identity) = build_mcp_identity(server, tool)
        {
            return Some(McpToolCallTarget {
                identity,
                input: None,
            });
        }
    }

    if let Some(input_obj) = raw_input.and_then(Value::as_object) {
        if let (Some(server), Some(tool)) = (
            input_obj.get("ServerName").and_then(Value::as_str),
            input_obj.get("ToolName").and_then(Value::as_str),
        ) && let Some(identity) = build_mcp_identity(server, tool)
        {
            let args = input_obj
                .get("Arguments")
                .filter(|v| v.is_object())
                .cloned();
            return Some(McpToolCallTarget {
                identity,
                input: args,
            });
        }

        if let Some(kiro_server) = meta
            .and_then(Value::as_object)
            .and_then(|m| m.get("kiro"))
            .and_then(Value::as_object)
            .and_then(|k| k.get("serverName"))
            .and_then(Value::as_str)
            && let Some(tool_id) = input_obj.get("tool_id").and_then(Value::as_str)
            && let Some((_, kiro_tool)) = tool_id.split_once("::")
            && let Some(identity) = build_mcp_identity(kiro_server, kiro_tool)
        {
            let args = input_obj
                .get("arguments")
                .filter(|v| v.is_object())
                .cloned();
            return Some(McpToolCallTarget {
                identity,
                input: args,
            });
        }
    }

    extract_mcp_tool_identity(title).map(|identity| McpToolCallTarget {
        identity,
        input: None,
    })
}

const GENERIC_COMMAND_TITLES: &[&str] = &["Ran command", "Run command", "Command"];

/// Returns the human-readable status label for a commandless `command_execution` item,
/// preventing generic provider titles such as `"Ran command"` from rendering `"Running Ran"` (`9294a9801e`).
#[must_use]
pub fn commandless_command_label(
    item_type: &str,
    command: Option<&str>,
    provider_title: Option<&str>,
    status: &ToolCallStatus,
) -> Option<&'static str> {
    if item_type != "command_execution" || command.is_some_and(|c| !c.trim().is_empty()) {
        return None;
    }
    let normalized = provider_title.map(str::trim).filter(|t| !t.is_empty());
    let is_generic = normalized.is_none_or(|t| GENERIC_COMMAND_TITLES.contains(&t));
    if !is_generic {
        return None;
    }
    Some(match status {
        ToolCallStatus::Pending | ToolCallStatus::InProgress => "Running command",
        ToolCallStatus::Failed => "Failed command",
        ToolCallStatus::Unknown(raw) if raw.eq_ignore_ascii_case("declined") => "Declined command",
        ToolCallStatus::Unknown(raw) if raw.eq_ignore_ascii_case("stopped") => "Stopped command",
        ToolCallStatus::Completed | ToolCallStatus::Unknown(_) => "Ran command",
    })
}
