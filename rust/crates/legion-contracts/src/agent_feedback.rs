//! Agent feedback request preview, conversation trace, and answer contracts (`agentFeedback.ts`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{ThreadId, TurnItemId};

/// Category of an agent feedback report (`AgentFeedbackKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFeedbackKind {
    /// Software defect or unexpected behavior.
    Bug,
    /// Unclear UX, naming, or error behavior.
    Confusing,
    /// Missing feature or tool capability.
    MissingCapability,
    /// Inaccurate or misleading documentation.
    WrongDocs,
    /// Any other report category.
    Other,
}

/// Entry role in a scrubbed [`AgentFeedbackTrace`] (`entries[].type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentFeedbackTraceEntryType {
    /// User prompt turn.
    User,
    /// Assistant message turn.
    Assistant,
    /// Tool execution entry.
    Tool,
}

/// Outcome recorded on a tool entry in [`AgentFeedbackTrace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentFeedbackTraceOutcome {
    /// Tool finished successfully.
    Ok,
    /// Tool failed with an error.
    Error,
    /// Tool did not finish before tracing.
    Unfinished,
}

/// One timeline entry inside an [`AgentFeedbackTrace`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFeedbackTraceEntry {
    /// 1-based turn index.
    pub turn: i64,
    /// Entry category.
    #[serde(rename = "type")]
    pub entry_type: AgentFeedbackTraceEntryType,
    /// Tool name when `entry_type == Tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Tool outcome when `entry_type == Tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentFeedbackTraceOutcome>,
    /// Tool duration in milliseconds when measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Character count of the original message text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<i64>,
    /// Scrubbed message text when conversation inclusion was enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Server-built redacted conversation outline or trace (`AgentFeedbackTrace`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFeedbackTrace {
    /// Total turn count inspected.
    pub turns: i64,
    /// Count of turns with text messages.
    pub text_turns: i64,
    /// Whether caps dropped entries or truncated message text.
    pub truncated: bool,
    /// Ordered trace entries.
    pub entries: Vec<AgentFeedbackTraceEntry>,
}

/// Scrubbed feedback report shown for user confirmation before sending (`AgentFeedbackPreview`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFeedbackPreview {
    /// Feedback classification.
    pub kind: AgentFeedbackKind,
    /// Short subject summary.
    pub subject: String,
    /// Expected behavior.
    pub expected: String,
    /// Observed actual behavior.
    pub actual: String,
    /// Workaround attempted, if any.
    pub workaround: String,
    /// Optional attached conversation trace, or `None`.
    pub trace: Option<AgentFeedbackTrace>,
}

/// Lifecycle state of a `feedback_request` turn item (`AgentFeedbackRequestStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentFeedbackRequestStatus {
    /// Awaiting user decision.
    Pending,
    /// Approved and sent.
    Sent,
    /// Declined by user.
    Declined,
    /// Cancelled before user response.
    Cancelled,
}

/// User decision on a feedback request (`AgentFeedbackAnswerInput.answer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentFeedbackAnswerChoice {
    /// Send the feedback report.
    Send,
    /// Decline sending the report.
    Decline,
}

/// RPC payload for `agentFeedback.answerRequest` (`AgentFeedbackAnswerInput`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFeedbackAnswerInput {
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Target `feedback_request` turn item ID.
    pub turn_item_id: TurnItemId,
    /// User answer (`send` or `decline`).
    pub answer: AgentFeedbackAnswerChoice,
    /// Include message text trace rather than outline only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_conversation: Option<bool>,
}

/// Failure reasons returned by `agentFeedback.answerRequest` (`AgentFeedbackRequestError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFeedbackFailureReason {
    /// Reading the stored request failed.
    #[error("Could not load the feedback report.")]
    LoadFailed,
    /// Target turn item does not exist.
    #[error("This feedback report no longer exists.")]
    NotFound,
    /// Request was already resolved.
    #[error("This feedback report was already answered.")]
    AlreadyAnswered,
    /// Server `agentFeedback` setting is `off`.
    #[error("Agent feedback is off, so this report was not sent.")]
    FeedbackOff,
    /// Recording the resolution command failed.
    #[error("Could not record your answer.")]
    RecordFailed,
}

/// Tagged contract error for `agentFeedback.answerRequest` (`AgentFeedbackRequestError`).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{reason}")]
pub struct AgentFeedbackRequestError {
    /// Structured failure reason code.
    pub reason: AgentFeedbackFailureReason,
}
