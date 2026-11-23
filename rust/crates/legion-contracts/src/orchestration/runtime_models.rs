//! Provider session, turn, runtime request, and conversation message domain models.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{
    ContextHandoffId, MessageId, NodeId, ProviderDriverKind, ProviderInstanceId, ProviderSessionId,
    ProviderThreadId, ProviderTurnId, RunAttemptId, RunId, RuntimeRequestId, ScheduledTaskId,
    ThreadId,
};

use super::command_payloads_extra::ProviderApprovalDecision;
use super::common::{
    CreationSource, DelegatedCompletionRef, JsonRecord, LinkedSender, OrchestrationActor,
    ProviderRef,
};
use super::model_selection::OrchestrationNotification;

/// Active or historical provider session (`orchestrationV2.ts:1028-1047`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSession {
    /// Provider session ID.
    pub id: ProviderSessionId,
    /// Provider driver kind.
    pub driver: ProviderDriverKind,
    /// Provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Session status (`starting`, `ready`, `running`, `waiting`, `stopped`, `error`).
    pub status: String,
    /// Working directory.
    pub cwd: String,
    /// Active model slug, or `null`.
    pub model: Option<String>,
    /// Provider capabilities matrix.
    pub capabilities: Value,
    /// Extension status lines, defaulting to empty.
    #[serde(default)]
    pub extension_statuses: Vec<Value>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Last error string, or `null`.
    pub last_error: Option<String>,
}

/// Payload for `provider-session.detached` (`orchestrationV2.ts:1050-1054`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSessionDetached {
    /// Detached provider session ID.
    pub provider_session_id: ProviderSessionId,
    /// Detach timestamp.
    pub detached_at: String,
    /// Optional detach reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Provider-native thread mapping (`orchestrationV2.ts:1172-1206`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderThread {
    /// Provider thread ID.
    pub id: ProviderThreadId,
    /// Provider driver kind.
    pub driver: ProviderDriverKind,
    /// Provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Bound provider session ID, or `null`.
    pub provider_session_id: Option<ProviderSessionId>,
    /// Bound app thread ID, or `null`.
    pub app_thread_id: Option<ThreadId>,
    /// Owning execution node ID, or `null`.
    pub owner_node_id: Option<NodeId>,
    /// Native thread ref, or `null`.
    pub native_thread_ref: Option<ProviderRef>,
    /// Native conversation head ref, or `null`.
    pub native_conversation_head_ref: Option<ProviderRef>,
    /// Status (`not_loaded`, `idle`, `active`, `archived`, `closed`, `error`).
    pub status: String,
    /// First run ordinal executed on this provider thread, or `null`.
    pub first_run_ordinal: Option<u64>,
    /// Last run ordinal executed on this provider thread, or `null`.
    pub last_run_ordinal: Option<u64>,
    /// Handoff IDs applied to this thread.
    pub handoff_ids: Vec<ContextHandoffId>,
    /// Fork provenance, or `null`.
    pub forked_from: Option<Value>,
    /// Pending background tasks, defaulting to empty.
    #[serde(default)]
    pub pending_background_tasks: Vec<Value>,
    /// Token usage snapshot, defaulting to `null`.
    #[serde(default)]
    pub context_usage: Option<Value>,
    /// Provider-native metadata, defaulting to `null`.
    #[serde(default)]
    pub native_metadata: Option<Value>,
    /// Active provider goal state, defaulting to `null`.
    #[serde(default)]
    pub goal: Option<Value>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Provider-native turn record (`orchestrationV2.ts:1266-1290`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTurn {
    /// Provider turn ID.
    pub id: ProviderTurnId,
    /// Owning provider thread ID.
    pub provider_thread_id: ProviderThreadId,
    /// Owning execution node ID.
    pub node_id: NodeId,
    /// Associated run attempt ID, or `null`.
    pub run_attempt_id: Option<RunAttemptId>,
    /// Native turn reference, or `null`.
    pub native_turn_ref: Option<ProviderRef>,
    /// Native thread reference when threads migrate between sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_thread_ref: Option<ProviderRef>,
    /// 1-based ordinal.
    pub ordinal: u64,
    /// Turn status.
    pub status: String,
    /// Start timestamp, or `null`.
    pub started_at: Option<String>,
    /// Completion timestamp, or `null`.
    pub completed_at: Option<String>,
    /// Legacy token usage summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<Value>,
    /// Structured main-agent token usage summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_token_usage: Option<Value>,
    /// Actual model reported by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Wire-extended `ProviderTurn` carrying `promptDropped` without breaking downstream literals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTurnWire {
    /// Core provider turn record.
    #[serde(flatten)]
    pub base: ProviderTurn,
    /// `Some(true)` when the provider never received the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_dropped: Option<bool>,
}

/// Returns `true` when the turn's prompt was delivered to the provider (`isDeliveredProviderTurn`).
#[must_use]
pub const fn is_delivered_provider_turn(prompt_dropped: Option<bool>) -> bool {
    !matches!(prompt_dropped, Some(true))
}

/// Pending or resolved interactive runtime request (`orchestrationV2.ts:1322-1346`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRequest {
    /// Request ID.
    pub id: RuntimeRequestId,
    /// Execution node ID.
    pub node_id: NodeId,
    /// Associated provider turn ID, or `null`.
    pub provider_turn_id: Option<ProviderTurnId>,
    /// Native request ref, or `null`.
    pub native_request_ref: Option<ProviderRef>,
    /// Request kind.
    pub kind: String,
    /// Status (`pending`, `resolved`, `expired`, `cancelled`).
    pub status: String,
    /// How responses can be delivered (`live`, `message`, `not_resumable`).
    pub response_capability: Value,
    /// Creation timestamp.
    pub created_at: String,
    /// Resolution timestamp, or `null`.
    pub resolved_at: Option<String>,
    /// Recorded approval decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ProviderApprovalDecision>,
    /// Recorded user feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Recorded user-input answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<JsonRecord>,
}

/// Conversation message record (`orchestrationV2.ts:1432-1464`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    /// Optional background notification metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<OrchestrationNotification>,
    /// Creator actor.
    pub created_by: OrchestrationActor,
    /// Originating surface.
    pub creation_source: CreationSource,
    /// Scheduled task ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_task_id: Option<ScheduledTaskId>,
    /// Whether this scheduled message follows thread model selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows_thread_selection: Option<bool>,
    /// Local sending thread ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_thread_id: Option<ThreadId>,
    /// Cross-environment sender attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_sender: Option<LinkedSender>,
    /// Idle compaction trigger reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_compaction: Option<String>,
    /// Whether the message carries a provider-native workflow script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_script: Option<bool>,
    /// Message ID.
    pub id: MessageId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Owning run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Owning node ID, or `null`.
    pub node_id: Option<NodeId>,
    /// Role (`user`, `assistant`, `system`).
    pub role: String,
    /// Message text body.
    pub text: String,
    /// Optional inline composer context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Attached chat files/images.
    pub attachments: Vec<Value>,
    /// Whether the message is actively streaming.
    pub streaming: bool,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Delegated task completion cohort info.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_completion: Option<DelegatedCompletionRef>,
}
