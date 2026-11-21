//! Typed wire models for [`super`] matching `packages/contracts/src/orchestratorMcp.ts`.

use serde::{Deserialize, Serialize};

/// Message delivery mode for `t3_thread_send` (`orchestratorMcp.ts:557`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadSendMode {
    /// Let server choose start/queue/steer.
    Auto,
    /// Queue message behind any active run (`t3 thread send` default).
    Queue,
    /// Steer the currently running turn.
    Steer,
    /// Interrupt current run and restart with new prompt.
    Restart,
}

/// Delivery outcome returned by `t3_thread_send` (`orchestratorMcp.ts:575`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadSendDelivery {
    /// Started a new turn immediately.
    Started,
    /// Queued behind an active turn.
    Queued,
    /// Steered the running turn.
    Steered,
    /// Restarted the running turn.
    Restarted,
}

/// Request arguments for `t3_thread_list` (`orchestratorMcp.ts:360-373`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListRequest {
    /// Target linked environment ID, or `None` for local.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Target project ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Include threads across all projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_projects: Option<bool>,
    /// Include delegated child/subagent threads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_subagents: Option<bool>,
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u32>,
    /// Maximum rows per page (`1..=100`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Summary row returned inside `t3_thread_list` (`orchestratorMcp.ts:398-433`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListItem {
    /// Unique thread ID.
    pub thread_id: String,
    /// Owning project ID.
    pub project_id: String,
    /// Display title.
    pub title: String,
    /// Current status (`"idle"`, `"running"`, `"completed"`, etc.).
    pub status: String,
    /// Latest run ID if any.
    pub latest_run_id: Option<String>,
    /// Provider instance identifier.
    pub provider_instance_id: String,
    /// Active model ID.
    pub model: String,
    /// Timeline item count.
    pub item_count: u64,
    /// Creation timestamp (ISO-8601).
    pub created_at: String,
    /// Update timestamp (ISO-8601).
    pub updated_at: String,
}

/// Paged response returned by `t3_thread_list` (`orchestratorMcp.ts:436-443`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResult {
    /// Filtered project ID, if single-project query.
    pub project_id: Option<String>,
    /// Listed threads.
    pub threads: Vec<ThreadListItem>,
    /// Next pagination cursor, or `null`.
    pub next_cursor: Option<u32>,
    /// Total matching thread count.
    pub total: u64,
}

/// Request arguments for `t3_thread_read` (`orchestratorMcp.ts:446-459`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadRequest {
    /// Target thread ID.
    pub thread_id: String,
    /// Specific item ID for reading truncated text chunks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// UTF-16 character offset inside `item_id` text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_offset: Option<u64>,
    /// Timeline view (`"messages"` or `"activity"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Exclusive lower bound position (`>= -1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_position: Option<i64>,
    /// Exclusive upper bound position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_position: Option<u64>,
    /// Maximum items (`1..=100`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Maximum recent runs (`1..=50`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_limit: Option<u32>,
    /// Maximum characters per item (`1..=20_000`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_chars_per_item: Option<u32>,
}

/// Detailed thread metadata from `t3_thread_read` (`orchestratorMcp.ts:462-505`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    /// Thread ID.
    pub thread_id: String,
    /// Project ID.
    pub project_id: String,
    /// Thread title.
    pub title: String,
    /// Current status.
    pub status: String,
    /// Provider instance ID.
    pub provider_instance_id: String,
    /// Model ID.
    pub model: String,
    /// Active git branch if worktree-backed.
    pub branch: Option<String>,
    /// Active git worktree path if any.
    pub worktree_path: Option<String>,
    /// Pending approval/question count blocking completion.
    pub pending_request_count: u64,
    /// Total timeline items.
    pub item_count: u64,
}

/// Run summary returned inside `t3_thread_read` (`orchestratorMcp.ts:508-518`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRunSummary {
    /// Run identifier.
    pub run_id: String,
    /// Monotonic run ordinal on this thread.
    pub ordinal: u64,
    /// Run status string.
    pub status: String,
}

/// Timeline item returned inside `t3_thread_read` (`orchestratorMcp.ts:521-539`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTimelineItem {
    /// Zero-based timeline position.
    pub position: u64,
    /// Turn item ID.
    pub item_id: String,
    /// Associated run ID, if any.
    pub run_id: Option<String>,
    /// Item type (`"user_message"`, `"assistant_message"`, `"error"`, etc.).
    #[serde(rename = "type")]
    pub item_type: String,
    /// Optional item text body.
    pub text: Option<String>,
    /// Whether `text` was truncated at `max_chars_per_item`.
    pub text_truncated: bool,
    /// Continuation character offset when `text_truncated == true`.
    #[serde(default)]
    pub next_text_offset: Option<u64>,
}

/// Result returned by `t3_thread_read` (`orchestratorMcp.ts:542-550`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadResult {
    /// Thread metadata snapshot.
    pub thread: ThreadDetail,
    /// Recent runs on this thread.
    pub recent_runs: Vec<ThreadRunSummary>,
    /// Requested slice of timeline items.
    pub items: Vec<ThreadTimelineItem>,
    /// Cursor position for forward paging.
    pub next_position: Option<u64>,
    /// Whether more items exist beyond this page.
    pub has_more: bool,
}

/// Request arguments for `t3_thread_send` (`orchestratorMcp.ts:553-565`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSendRequest {
    /// Target thread ID.
    pub thread_id: String,
    /// Prompt text to deliver.
    pub message: String,
    /// Optional delivery strategy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ThreadSendMode>,
}

/// Result returned by `t3_thread_send` (`orchestratorMcp.ts:568-576`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSendResult {
    /// Target thread ID.
    pub thread_id: String,
    /// Created user message ID.
    pub message_id: String,
    /// Scheduled or affected run ID.
    pub run_id: String,
    /// Initial run status.
    pub status: String,
    /// Delivery classification.
    pub delivery: ThreadSendDelivery,
}

/// Request arguments for `t3_thread_wait` (`orchestratorMcp.ts:579-590`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadWaitRequest {
    /// Target thread ID.
    pub thread_id: String,
    /// Specific run ID to await.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Bounded wait timeout in milliseconds (`max 45_000` on `/mcp`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Also await delegated/background tasks settling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until_delegated_work_settled: Option<bool>,
}

/// Result returned by `t3_thread_wait` (`orchestratorMcp.ts:593-601`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadWaitResult {
    /// Target thread ID.
    pub thread_id: String,
    /// Settled or latest run ID.
    pub run_id: Option<String>,
    /// Thread/run status.
    pub status: String,
    /// True when the wait slice timed out before terminal completion.
    pub timed_out: bool,
}
