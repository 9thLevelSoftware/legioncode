//! Wire JSON structs for thread projections, bounded snapshots, history pages, and transcripts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Projected timeline item row (`OrchestrationV2ProjectedTurnItem`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProjectedTurnItem {
    /// 0-based position in the returned slice.
    pub position: usize,
    /// Visibility tier (`"local"`, `"inherited"`, or `"synthetic"`).
    pub visibility: String,
    /// Owning thread ID.
    pub source_thread_id: String,
    /// Turn item ID on `source_thread_id`.
    pub source_item_id: String,
    /// Wire-projected turn item JSON.
    pub item: Value,
}

/// Full canonical thread projection (`OrchestrationV2ThreadProjection`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireThreadProjection {
    /// Thread metadata record.
    pub thread: Value,
    /// Ordered thread runs.
    pub runs: Vec<Value>,
    /// Ordered run attempts.
    pub attempts: Vec<Value>,
    /// Execution nodes.
    pub nodes: Vec<Value>,
    /// Subagent records.
    pub subagents: Vec<Value>,
    /// Provider sessions.
    pub provider_sessions: Vec<Value>,
    /// Provider threads.
    pub provider_threads: Vec<Value>,
    /// Provider turns.
    pub provider_turns: Vec<Value>,
    /// Runtime requests.
    pub runtime_requests: Vec<Value>,
    /// Ordered conversation messages.
    pub messages: Vec<Value>,
    /// Plan artifacts.
    pub plans: Vec<Value>,
    /// Thread-local turn items.
    pub turn_items: Vec<Value>,
    /// Checkpoint scopes.
    pub checkpoint_scopes: Vec<Value>,
    /// Checkpoints.
    pub checkpoints: Vec<Value>,
    /// Context handoffs.
    pub context_handoffs: Vec<Value>,
    /// Context transfers.
    pub context_transfers: Vec<Value>,
    /// Projected timeline rows including inherited fork items.
    pub visible_turn_items: Vec<WireProjectedTurnItem>,
    /// Latest projection update timestamp.
    pub updated_at: String,
}

/// Full thread detail snapshot (`OrchestrationV2ThreadDetailSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireThreadDetailSnapshot {
    /// Latest sequence on this thread's event stream.
    pub snapshot_sequence: i64,
    /// Wire-projected thread state.
    pub projection: WireThreadProjection,
    /// Optional opaque history pagination cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_cursor: Option<Option<String>>,
    /// Whether older history remains before the cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_more_history: Option<bool>,
    /// Highest local turn ordinal covered by the snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_local_turn_ordinal: Option<Option<u64>>,
}

/// Bounded thread snapshot (`OrchestrationV2ThreadBoundedSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireThreadBoundedSnapshot {
    /// Latest sequence on this thread's event stream.
    pub snapshot_sequence: i64,
    /// Bounded wire-projected thread state.
    pub projection: WireThreadProjection,
    /// Opaque base64url cursor for fetching older history, or `null`.
    pub history_cursor: Option<String>,
    /// Whether older timeline items exist before `history_cursor`.
    pub has_more_history: bool,
    /// Highest local turn ordinal retained in this window, or `null`.
    pub latest_local_turn_ordinal: Option<u64>,
}

/// Thread history page (`OrchestrationV2ThreadHistoryPage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireThreadHistoryPage {
    /// Snapshot sequence captured for the page.
    pub snapshot_sequence: i64,
    /// Ordered older timeline items on this page.
    pub items: Vec<WireProjectedTurnItem>,
    /// Next base64url cursor, or `null` at the start of history.
    pub next_cursor: Option<String>,
    /// Whether additional older items precede this page.
    pub has_more_history: bool,
    /// Referenced subagent records for subagent items on this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<Value>>,
}

/// Thread transcript response matching `OrchestrationV2ThreadTranscript`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireThreadTranscript {
    /// Thread ID.
    pub thread_id: String,
    /// Thread title.
    pub title: String,
    /// Latest projection update timestamp.
    pub updated_at: String,
    /// Visible projected turn items in chronological order.
    pub items: Vec<WireProjectedTurnItem>,
}
