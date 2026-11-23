#![forbid(unsafe_code)]
//! Types, constants, request payloads, and error types for local-only agent feedback (`ADR-013`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::trace_types::{FeedbackKind, FeedbackTrace, TraceItemInput, TraceRunInput};

/// Maximum unprompted reports or approval cards permitted per agent turn.
pub const MAX_REPORTS_PER_TURN: i64 = 3;
/// Maximum reports outside any turn (CLI `legion feedback`) per rolling hour.
pub const MAX_REPORTS_OUTSIDE_TURNS_PER_HOUR: i64 = 10;
/// Maximum reports returned by `FeedbackStore::list_reports`.
pub const FEEDBACK_LIST_LIMIT: usize = 50;

/// Guidance returned to the agent when a requested report awaits human approval.
pub const FEEDBACK_AWAITING_APPROVAL_NOTE: &str = "The user sees this report in a card and chooses there whether to save it locally and whether to include the conversation. Nothing is stored until they approve it, and you will not learn their choice.";
/// Message returned when feedback is disabled in Labs/settings.
pub const FEEDBACK_OFF_MESSAGE: &str =
    "Agent feedback is off. Turn it on in Settings > Labs, then try again.";

/// Server-wide feedback collection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackMode {
    Off,
    OnRequest,
    Auto,
}

/// Origin surface of a feedback submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackSource {
    Mcp,
    Cli,
}

impl FeedbackSource {
    /// Canonical SQL/wire string for the submission surface.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Cli => "cli",
        }
    }

    /// Parses SQL/wire source text.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "mcp" => Some(Self::Mcp),
            "cli" => Some(Self::Cli),
            _ => None,
        }
    }
}

/// Scrubbed feedback report preview shown on approval cards and stored in `agent_feedback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackPreview {
    pub kind: FeedbackKind,
    pub subject: String,
    pub expected: String,
    pub actual: String,
    pub workaround: String,
    pub trace: Option<FeedbackTrace>,
}

/// Persisted row inside `agent_feedback` (`sent` is strictly `false` / `0` under ADR-013).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackReport {
    pub id: String,
    pub thread_id: Option<String>,
    pub created_at: String,
    pub kind: FeedbackKind,
    pub subject: String,
    pub expected: String,
    pub actual: String,
    pub workaround: String,
    pub user_requested: bool,
    pub trace: Option<FeedbackTrace>,
    pub source: FeedbackSource,
    /// Strictly `false` (SQL `sent = 0`) because `LegionCode` stores agent feedback locally only.
    pub sent: bool,
}

/// Status of an approval request card displayed in a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackRequestStatus {
    Pending,
    Sent,
    Declined,
    Cancelled,
}

/// Pending approval card stored while awaiting `agentFeedback.answerRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingFeedbackCard {
    pub report_id: String,
    pub thread_id: String,
    pub run_id: String,
    pub turn_item_id: String,
    pub preview: FeedbackPreview,
    pub conversation_trace: Option<FeedbackTrace>,
    pub status: FeedbackRequestStatus,
}

/// User decision on a pending approval card (`agentFeedback.answerRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedbackAnswerChoice {
    /// Approve local persistence (`send` / approve wire choice; stores with `sent = 0`).
    Send,
    Decline,
}

/// Payload for `agentFeedback.answerRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFeedbackAnswerRequest {
    pub thread_id: String,
    pub turn_item_id: String,
    pub answer: FeedbackAnswerChoice,
    #[serde(default)]
    pub include_conversation: bool,
}

/// Raw submission request from `legion_feedback` / `t3_feedback` / CLI.
#[derive(Debug, Clone)]
pub struct SubmitFeedbackInput<'a> {
    pub kind: FeedbackKind,
    pub subject: &'a str,
    pub expected: &'a str,
    pub actual: &'a str,
    pub workaround: &'a str,
    pub user_requested: bool,
    pub turns: usize,
    pub source: FeedbackSource,
    pub thread_id: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub sensitive: &'a [&'a str],
    pub registered_tools: &'a [&'a str],
    pub cli_commands: &'a [&'a str],
    pub trace_runs: &'a [TraceRunInput],
    pub trace_items: &'a [TraceItemInput],
}

/// Result returned from `FeedbackStore::submit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackSubmitOutcome {
    /// Stored immediately in `agent_feedback` with `sent = 0` (`auto` mode unrequested report).
    Stored(FeedbackReport),
    /// Approval card recorded in pending state awaiting user decision via `answer_request`.
    AwaitingApproval {
        report_id: String,
        turn_item_id: String,
        preview: FeedbackPreview,
        conversation_trace: Option<FeedbackTrace>,
        note: &'static str,
    },
}

/// Errors produced by local feedback validation, mode gating, limits, or approval handling.
#[derive(Debug, Error)]
pub enum FeedbackError {
    #[error("{FEEDBACK_OFF_MESSAGE}")]
    Off,
    #[error("Agent feedback is set to When I ask: send a report only when the user asks for one.")]
    NotRequested { origin: FeedbackSource },
    #[error("Set subject to the exact tool name, the CLI command, or `general`.")]
    InvalidSubject,
    #[error("Requested feedback requires an active thread turn.")]
    ApprovalContextMissing,
    #[error("Feedback rate limit reached ({scope}).")]
    LimitReached { scope: &'static str },
    #[error("Could not load the feedback report.")]
    LoadFailed,
    #[error("This feedback report no longer exists.")]
    NotFound,
    #[error("This feedback report was already answered.")]
    AlreadyAnswered,
    #[error("Agent feedback is off, so this report was not saved.")]
    RequestFeedbackOff,
    #[error("Could not record agent feedback: {0}")]
    Storage(String),
}

pub(crate) type RawFeedbackRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    String,
    i64,
);

pub(crate) fn row_from_sqlite(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawFeedbackRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

pub(crate) fn decode_feedback_row(raw: RawFeedbackRow) -> Result<FeedbackReport, FeedbackError> {
    let (
        id,
        thread_id,
        created_at,
        kind_str,
        subject,
        expected,
        actual,
        workaround,
        user_req,
        trace_raw,
        src_str,
        sent_int,
    ) = raw;
    let kind = crate::trace_types::FeedbackKind::parse(&kind_str)
        .ok_or_else(|| FeedbackError::Storage(format!("invalid feedback kind '{kind_str}'")))?;
    let source = FeedbackSource::parse(&src_str)
        .ok_or_else(|| FeedbackError::Storage(format!("invalid feedback source '{src_str}'")))?;
    let trace: Option<FeedbackTrace> = trace_raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| FeedbackError::Storage(e.to_string()))?;
    Ok(FeedbackReport {
        id,
        thread_id,
        created_at,
        kind,
        subject,
        expected,
        actual,
        workaround,
        user_requested: user_req == 1,
        trace,
        source,
        sent: sent_int == 1,
    })
}

pub(crate) fn check_allowed(
    mode: FeedbackMode,
    user_requested: bool,
    origin: FeedbackSource,
) -> Result<(), FeedbackError> {
    match mode {
        FeedbackMode::Off => Err(FeedbackError::Off),
        FeedbackMode::OnRequest if !user_requested => Err(FeedbackError::NotRequested { origin }),
        FeedbackMode::OnRequest | FeedbackMode::Auto => Ok(()),
    }
}

pub(crate) fn resolve_subject(
    raw: &str,
    registered_tools: &[&str],
    cli_commands: &[&str],
) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed == "general" || cli_commands.contains(&trimmed) {
        return Some(trimmed.to_owned());
    }
    crate::trace_types::resolve_feedback_tool_name(trimmed, registered_tools)
}
