//! Plan artifact, checkpoint, context-handoff, and context-transfer domain models.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{
    CheckpointId, CheckpointRef, CheckpointScopeId, ContextHandoffId, ContextTransferId, MessageId,
    NodeId, PlanId, ProviderInstanceId, ProviderThreadId, RunId, ThreadId,
};

use super::common::OrchestrationActor;
use super::items::CheckpointFileSummary;
use super::model_selection::ContextSourcePoint;

/// Plan or todo-list artifact (`orchestrationV2.ts:1504-1516`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanArtifact {
    /// Plan ID.
    pub id: PlanId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Owning run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Owning execution node ID.
    pub node_id: NodeId,
    /// Plan status (`draft`, `active`, `completed`, `superseded`).
    pub status: String,
    /// Whether detail lives in the corresponding turn item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_in_turn_item: Option<bool>,
    /// Plan artifact kind (`proposed_plan` or `todo_list`).
    pub kind: String,
    /// Markdown plan content when `kind == "proposed_plan"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Checklist steps when `kind == "todo_list"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Value>>,
    /// Optional explanation when `kind == "todo_list"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

/// Checkpoint scope metadata (`orchestrationV2.ts:1013-1025`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointScope {
    /// Scope ID.
    pub id: CheckpointScopeId,
    /// Thread ID.
    pub thread_id: ThreadId,
    /// Run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Execution node ID.
    pub node_id: NodeId,
    /// Parent checkpoint scope ID, or `null`.
    pub parent_scope_id: Option<CheckpointScopeId>,
    /// Provider thread ID, or `null`.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// Scope kind (`root_run`, `subagent`, `tool`, `provider_thread`, `manual`).
    pub kind: String,
    /// Ordinal within parent scope.
    pub ordinal_within_parent: u64,
    /// Whether this scope advances the app run count.
    pub advances_app_run_count: bool,
    /// Working directory.
    pub cwd: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// Captured filesystem/conversation checkpoint (`orchestrationV2.ts:1536-1550`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// Checkpoint ID.
    pub id: CheckpointId,
    /// Thread ID.
    pub thread_id: ThreadId,
    /// Checkpoint scope ID.
    pub scope_id: CheckpointScopeId,
    /// Run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Execution node ID.
    pub node_id: NodeId,
    /// Parent checkpoint ID, or `null`.
    pub parent_checkpoint_id: Option<CheckpointId>,
    /// Ordinal within scope.
    pub ordinal_within_scope: u64,
    /// App run ordinal, or `null`.
    pub app_run_ordinal: Option<u64>,
    /// Hidden git checkpoint ref.
    pub r#ref: CheckpointRef,
    /// Checkpoint status (`ready`, `missing`, `error`, `stale`).
    pub status: String,
    /// Changed file summaries.
    pub files: Vec<CheckpointFileSummary>,
    /// Aggregate totals when truncated on wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_total: Option<Value>,
    /// Capture timestamp.
    pub captured_at: String,
}

/// Payload for `checkpoint.rollback-requested` (`orchestrationV2.ts:1553-1557`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRollbackRequest {
    /// Target checkpoint scope ID.
    pub scope_id: CheckpointScopeId,
    /// Target checkpoint ID.
    pub checkpoint_id: CheckpointId,
    /// Request timestamp.
    pub requested_at: String,
}

/// Portable context handoff artifact (`orchestrationV2.ts:1222-1263`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextHandoff {
    /// Handoff ID.
    pub id: ContextHandoffId,
    /// Associated context transfer ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_id: Option<Option<ContextTransferId>>,
    /// Thread ID.
    pub thread_id: ThreadId,
    /// Target run ID.
    pub target_run_id: RunId,
    /// Source provider thread IDs.
    pub from_provider_thread_ids: Vec<ProviderThreadId>,
    /// Destination provider thread ID.
    pub to_provider_thread_id: ProviderThreadId,
    /// Covered run ordinal bounds.
    pub covered_run_ordinals: Value,
    /// Summary strategy.
    pub strategy: String,
    /// Status (`pending`, `ready`, `failed`, `superseded`).
    pub status: String,
    /// Summary message ID, or `null`.
    pub summary_message_id: Option<MessageId>,
    /// Handoff summary text.
    pub summary_text: String,
    /// Optional structured history window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Value>,
    /// Optional delivery status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Value>,
    /// Whether full handoff detail is stored in a turn item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_in_turn_item: Option<bool>,
    /// Creator provider instance ID, or `null`.
    pub created_by_provider_instance_id: Option<ProviderInstanceId>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Context transfer record (`orchestrationV2.ts:247-272`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTransfer {
    /// Transfer ID.
    pub id: ContextTransferId,
    /// Transfer kind.
    #[serde(rename = "type")]
    pub transfer_type: String,
    /// Source thread ID.
    pub source_thread_id: ThreadId,
    /// Target thread ID.
    pub target_thread_id: ThreadId,
    /// Source provenance point.
    pub source_point: ContextSourcePoint,
    /// Optional base provenance point, or `null`.
    pub base_point: Option<ContextSourcePoint>,
    /// Source provider instance ID, or `null`.
    pub source_provider_instance_id: Option<ProviderInstanceId>,
    /// Optional source token usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_token_usage: Option<Value>,
    /// Target provider instance ID, or `null`.
    pub target_provider_instance_id: Option<ProviderInstanceId>,
    /// Target run ID, or `null`.
    pub target_run_id: Option<RunId>,
    /// Transfer status.
    pub status: String,
    /// Resolution payload, or `null`.
    pub resolution: Option<Value>,
    /// Creator actor.
    pub created_by: OrchestrationActor,
    /// Error message, or `null`.
    pub error: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Consumption timestamp, or `null`.
    pub consumed_at: Option<String>,
}
