#![forbid(unsafe_code)]
//! Feedback trace DTOs, input turn records, and LegionCode/T3 MCP tool-name resolution.

use legion_contracts::brand::canonical_mcp_tool_suffix;
use serde::{Deserialize, Serialize};

/// Minimum recent turns outlined on requested feedback cards (`FEEDBACK_TRACE_TURNS`).
pub const FEEDBACK_TRACE_TURNS: usize = 5;
/// Maximum trace entries retained before dropping older tool entries (`MAX_ENTRIES`).
pub const MAX_TRACE_ENTRIES: usize = 80;
/// Maximum characters retained per scrubbed conversation message in a trace.
pub const MAX_TRACE_TEXT_CHARS: usize = 1_000;
/// Soft serialized byte limit for an `AgentFeedbackTrace` payload (`16,000` bytes).
pub const FEEDBACK_TRACE_MAX_BYTES: usize = 16_000;
pub(crate) const OTHER_TOOL: &str = "mcp";

/// Feedback category matching `AgentFeedbackKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    Bug,
    Confusing,
    MissingCapability,
    WrongDocs,
    Other,
}

impl FeedbackKind {
    /// Returns the SQL/wire string for this category.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Confusing => "confusing",
            Self::MissingCapability => "missing_capability",
            Self::WrongDocs => "wrong_docs",
            Self::Other => "other",
        }
    }

    /// Parses a wire/SQL feedback category.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "bug" => Some(Self::Bug),
            "confusing" => Some(Self::Confusing),
            "missing_capability" => Some(Self::MissingCapability),
            "wrong_docs" => Some(Self::WrongDocs),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// Entry kind inside [`FeedbackTrace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackTraceEntryKind {
    User,
    Assistant,
    Tool,
}

/// Tool invocation outcome inside [`FeedbackTrace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackTraceOutcome {
    Ok,
    Error,
    Unfinished,
}

/// One outlined user message, assistant reply, or tool call inside [`FeedbackTrace`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackTraceEntry {
    /// 1-based turn index within the traced window.
    pub turn: usize,
    /// Entry type (`user`, `assistant`, or `tool`).
    #[serde(rename = "type")]
    pub kind: FeedbackTraceEntryKind,
    /// Resolved tool name (`shell`, `mcp`, `legion_*`, `t3_*`, or built-in tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Tool execution outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<FeedbackTraceOutcome>,
    /// Execution duration in milliseconds when both timestamps exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Original Unicode scalar count of a user/assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    /// Scrubbed message body (present only when `with_text` included this turn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Server-built redacted conversation/outline trace attached to a feedback report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackTrace {
    /// Number of started turns in the traced window.
    pub turns: usize,
    /// Number of turns that include scrubbed message text.
    pub text_turns: usize,
    /// True if any cap truncated entries or text.
    pub truncated: bool,
    /// Ordered entries across the traced window.
    pub entries: Vec<FeedbackTraceEntry>,
}

/// Input turn metadata consumed by `build_feedback_trace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRunInput {
    pub id: String,
    pub ordinal: i64,
    pub started_at: Option<String>,
    pub driver: Option<String>,
}

/// Input turn item kind consumed by `build_feedback_trace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceItemKind {
    UserMessage,
    AssistantMessage,
    DynamicTool,
    CommandExecution,
    FileChange,
    FileSearch,
    WebSearch,
    Subagent,
}

/// Input turn item status consumed by `build_feedback_trace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceItemStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Input turn item consumed by `build_feedback_trace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceItemInput {
    pub run_id: Option<String>,
    pub parent_item_id: Option<String>,
    pub ordinal: i64,
    pub kind: TraceItemKind,
    pub status: TraceItemStatus,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_is_integration: bool,
    pub exit_code: Option<i32>,
    pub output_indicates_failure: bool,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// Inputs to `build_feedback_trace`.
pub struct BuildFeedbackTraceInput<'a> {
    pub runs: &'a [TraceRunInput],
    pub items: &'a [TraceItemInput],
    pub turns: usize,
    pub with_text: bool,
    pub sensitive: &'a [&'a str],
    pub registered_tools: &'a [&'a str],
    pub submitting_run_id: &'a str,
}

/// Computes how many started turns a trace window covers (`5..=80`).
pub fn feedback_trace_window(turns: usize) -> usize {
    turns.clamp(FEEDBACK_TRACE_TURNS, MAX_TRACE_ENTRIES)
}

/// Resolves a provider-namespaced MCP tool name (`legion_*`, `t3_*`, `mcp__legion_code__*`,
/// `mcp__t3_code__*`, `OpenCode` `legion-code-<thread>_*` or `t3-code-<thread>_*`) to its bare
/// registered name if present in `registered_tools`.
pub fn resolve_feedback_tool_name(name: &str, registered_tools: &[&str]) -> Option<String> {
    let bare_contract = canonical_mcp_tool_suffix(name);
    if registered_tools.contains(&bare_contract) {
        return Some(bare_contract.to_owned());
    }
    let stripped_t3 = strip_brand_prefix(name);
    if registered_tools.contains(&stripped_t3) {
        return Some(stripped_t3.to_owned());
    }
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("legion-code-") || lower.starts_with("t3-code-") {
        let mut matches: Vec<&str> = registered_tools
            .iter()
            .copied()
            .filter(|cand| name.ends_with(&format!("_{cand}")))
            .collect();
        matches.sort_by_key(|cand| std::cmp::Reverse(cand.len()));
        return matches.first().map(|s| (*s).to_owned());
    }
    None
}

fn strip_brand_prefix(name: &str) -> &str {
    for prefix in [
        "mcp__legion_code_direct__",
        "mcp__legion-code-direct__",
        "mcp__legion_code__",
        "mcp__legion-code__",
        "mcp__t3_code_direct__",
        "mcp__t3-code-direct__",
        "mcp__t3_code__",
        "mcp__t3-code__",
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
        if name.len() > prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return &name[prefix.len()..];
        }
    }
    name
}

pub(crate) fn looks_random(value: &str) -> bool {
    let digits = value.chars().filter(char::is_ascii_digit).count();
    let has_alpha = value.chars().any(|c| c.is_ascii_alphabetic());
    digits >= 2 && has_alpha && shannon_entropy(value) >= 3.5
}

fn shannon_entropy(value: &str) -> f64 {
    let mut counts: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    let mut len = 0usize;
    for ch in value.chars() {
        *counts.entry(ch).or_insert(0) += 1;
        len += 1;
    }
    if len == 0 {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "feedback scan window length fits f64 without precision impact"
    )]
    let n = len as f64;
    let mut bits = 0.0;
    for count in counts.values() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "char count <= window length fits f64"
        )]
        let p = (*count as f64) / n;
        bits -= p * p.log2();
    }
    bits
}

pub(crate) fn utf8_char_len(s: &str, idx: usize) -> usize {
    s[idx..].chars().next().map_or(1, char::len_utf8)
}

pub(crate) fn next_char_boundary(s: &str, idx: usize) -> usize {
    idx + utf8_char_len(s, idx)
}
