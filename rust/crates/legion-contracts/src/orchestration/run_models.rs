//! Run, run-attempt, execution-node, and subagent domain models (`orchestrationV2.ts`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{
    CheckpointId, CheckpointScopeId, ContextHandoffId, MessageId, NodeId, ProviderDriverKind,
    ProviderInstanceId, ProviderThreadId, ProviderTurnId, RunAttemptId, RunId, RuntimeRequestId,
    ThreadId,
};

use super::common::{OrchestrationActor, ProviderRef};
use super::model_selection::{ModelSelection, WorkspaceStrategy};
use super::run_output::{RunOutputPayload, RunOutputSchema};

/// Lifecycle state of an orchestration run (`orchestrationV2.ts:680-691`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Preparing worktree or setup scripts.
    Preparing,
    /// Queued behind active runs.
    Queued,
    /// Starting provider session/turn.
    Starting,
    /// Actively executing.
    Running,
    /// Waiting on user input or background work.
    Waiting,
    /// Completed successfully.
    Completed,
    /// Interrupted by user or steer restart.
    Interrupted,
    /// Failed with an error.
    Failed,
    /// Cancelled before completion.
    Cancelled,
    /// Reverted via checkpoint rollback.
    RolledBack,
}

/// Turn run record (`orchestrationV2.ts:766-804`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// Run ID.
    pub id: RunId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// 1-based ordinal within the thread.
    pub ordinal: u64,
    /// Provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Model selection for this run.
    pub model_selection: ModelSelection,
    /// Provider thread ID, or `null`.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// User message ID that started this run.
    pub user_message_id: MessageId,
    /// Root execution node ID, or `null`.
    pub root_node_id: Option<NodeId>,
    /// Active run attempt ID, or `null`.
    pub active_attempt_id: Option<RunAttemptId>,
    /// Run lifecycle status.
    pub status: RunStatus,
    /// Optional queue position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<Option<u64>>,
    /// Whether queue progression is held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_held: Option<bool>,
    /// Target run ID for promoted steering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_when_ready_run_id: Option<RunId>,
    /// Enqueue timestamp.
    pub requested_at: String,
    /// Execution start timestamp, or `null`.
    pub started_at: Option<String>,
    /// Completion timestamp, or `null`.
    pub completed_at: Option<String>,
    /// Captured checkpoint ID, or `null`.
    pub checkpoint_id: Option<CheckpointId>,
    /// Context handoff ID, or `null`.
    pub context_handoff_id: Option<ContextHandoffId>,
    /// Run ID superseded by an automatic restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_continuation_of_run_id: Option<RunId>,
    /// Run ID superseded by manual continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_continuation_of_run_id: Option<RunId>,
    /// Run ID continued after usage-limit recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit_continuation_of_run_id: Option<RunId>,
    /// Parent delegated task node ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_task_id: Option<NodeId>,
    /// Timestamp when active provider work began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_started_at: Option<String>,
    /// Background tasks cancelled when restarting this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_cancelled_background_work: Option<Vec<Value>>,
    /// Source plan reference, if launched from a plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_plan_ref: Option<Value>,
    /// Delegated completion cohort batch metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_completion: Option<Value>,
    /// Workspace preparation strategy override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_preparation: Option<WorkspaceStrategy>,
    /// Whether this run started with a fresh provider context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_context: Option<bool>,
    /// JSON Schema describing structured output requested from this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<RunOutputSchema>,
    /// Accepted structured output payload submitted for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<RunOutputPayload>,
}

/// Run background-work cancellation payload (`orchestrationV2.ts:737-740`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBackgroundWorkCancelled {
    /// Target run ID.
    pub run_id: RunId,
    /// Background tasks cancelled when restarting the run.
    pub restart_cancelled_background_work: Vec<Value>,
}

/// Attempt record within a [`Run`] (`orchestrationV2.ts:812-834`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAttempt {
    /// Attempt ID.
    pub id: RunAttemptId,
    /// Native destination thread ID after recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_thread_id: Option<String>,
    /// Parent run ID.
    pub run_id: RunId,
    /// 1-based attempt ordinal.
    pub attempt_ordinal: u64,
    /// Root execution node ID.
    pub root_node_id: NodeId,
    /// Provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Provider thread ID.
    pub provider_thread_id: ProviderThreadId,
    /// Provider turn ID, or `null`.
    pub provider_turn_id: Option<ProviderTurnId>,
    /// Attempt creation reason.
    pub reason: String,
    /// Attempt status.
    pub status: String,
    /// Start timestamp, or `null`.
    pub started_at: Option<String>,
    /// Completion timestamp, or `null`.
    pub completed_at: Option<String>,
}

/// Execution tree node (`orchestrationV2.ts:837-875`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionNode {
    /// Node ID.
    pub id: NodeId,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Owning run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Parent node ID, or `null`.
    pub parent_node_id: Option<NodeId>,
    /// Root node ID.
    pub root_node_id: NodeId,
    /// Node kind.
    pub kind: String,
    /// Node execution status.
    pub status: String,
    /// Whether this node counts toward run active state.
    pub counts_for_run: bool,
    /// Provider thread ID, or `null`.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// Provider turn ID, or `null`.
    pub provider_turn_id: Option<ProviderTurnId>,
    /// Native item ref, or `null`.
    pub native_item_ref: Option<ProviderRef>,
    /// Runtime request ID, or `null`.
    pub runtime_request_id: Option<RuntimeRequestId>,
    /// Checkpoint scope ID, or `null`.
    pub checkpoint_scope_id: Option<CheckpointScopeId>,
    /// Start timestamp, or `null`.
    pub started_at: Option<String>,
    /// Completion timestamp, or `null`.
    pub completed_at: Option<String>,
}

/// Subagent record (`orchestrationV2.ts:963-1004`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subagent {
    /// Subagent node ID.
    pub id: NodeId,
    /// Parent thread ID.
    pub thread_id: ThreadId,
    /// Owning run ID, or `null`.
    pub run_id: Option<RunId>,
    /// Parent execution node ID.
    pub parent_node_id: NodeId,
    /// Origin (`provider_native` or `app_owned`).
    pub origin: String,
    /// Creator actor.
    pub created_by: OrchestrationActor,
    /// Provider driver kind.
    pub driver: ProviderDriverKind,
    /// Provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Provider thread ID, or `null`.
    pub provider_thread_id: Option<ProviderThreadId>,
    /// Local child thread ID, or `null`.
    pub child_thread_id: Option<ThreadId>,
    /// Remote child thread descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_child: Option<Value>,
    /// Native task reference, or `null`.
    pub native_task_ref: Option<ProviderRef>,
    /// Subagent prompt.
    pub prompt: String,
    /// Subagent title, or `null`.
    pub title: Option<String>,
    /// Requested model, or `null`.
    pub model: Option<String>,
    /// Model selection override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelSelection>,
    /// Executed model reported by provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executed_model: Option<String>,
    /// Wake policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_wake: Option<String>,
    /// Completion delivery tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_delivery: Option<Value>,
    /// Run ID whose stop cancelled this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_run_id: Option<RunId>,
    /// Subagent status.
    pub status: String,
    /// Live progress summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Multi-phase workflow metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<Value>,
    /// Final result text, or `null`.
    pub result: Option<String>,
    /// Start timestamp, or `null`.
    pub started_at: Option<String>,
    /// Completion timestamp, or `null`.
    pub completed_at: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
    /// Whether the subagent prompt was omitted from the wire payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_omitted: Option<bool>,
    /// Whether the subagent result was omitted from the wire payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_omitted: Option<bool>,
}
