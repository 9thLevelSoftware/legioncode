//! Core command payloads for thread creation, metadata updates, and message dispatch.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{
    CommandId, GuestMessageId, MessageId, ProjectId, ProviderDriverKind, RunId, ScheduledTaskId,
    ThreadId,
};
use crate::policy::RuntimeMode;
use crate::pull_requests::ThreadLinkedPullRequest;

use super::common::{
    CreationSource, DelegatedCompletionRef, DelegatedFrom, LinkOrigin, LinkedSender,
    OrchestrationActor, PatchValue,
};
use super::model_selection::{
    LimitRecoveryUpdate, ModelSelection, OrchestrationNotification, WorkspaceStrategy,
};
use super::run_output::RunOutputSchema;
use super::state_models::WorkflowOrigin;

/// Provider interaction mode (`providerPolicy.ts:45`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderInteractionMode {
    /// Standard execution mode.
    Default,
    /// Planning-only mode.
    Plan,
}

/// Strong native thread reference inside `importedNativeThread`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedNativeThreadRef {
    /// Provider driver kind.
    pub driver: ProviderDriverKind,
    /// Non-empty native thread ID.
    pub native_id: String,
    /// Always `"strong"`.
    pub strength: String,
}

/// Exact wire representation of `importedNativeThread` (`ref` field name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedNativeThreadPayload {
    /// Native thread reference (`ref` on the wire).
    #[serde(rename = "ref")]
    pub reference: ImportedNativeThreadRef,
    /// Optional provider-native metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Server-populated initial message on `thread.create` (`orchestrationV2.ts:3167-3178`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialThreadMessage {
    /// Message identifier.
    pub message_id: MessageId,
    /// Prompt text.
    pub text: String,
    /// Attached chat files/images.
    #[serde(default)]
    pub attachments: Vec<Value>,
    /// Optional inline composer context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Optional seed for auto-title generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_seed: Option<String>,
    /// Scheduled task that triggered this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_task_id: Option<ScheduledTaskId>,
    /// Local sending thread ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_thread_id: Option<ThreadId>,
    /// Optional JSON Schema requiring a typed structured output for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<RunOutputSchema>,
    /// Optional workspace preparation strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_strategy: Option<WorkspaceStrategy>,
}

/// Wire payload for `thread.create` (`orchestrationV2.ts:3138-3179`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCreateCommand {
    /// Actor that initiated thread creation.
    pub created_by: OrchestrationActor,
    /// Client/surface that initiated creation.
    pub creation_source: CreationSource,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Owning project ID.
    pub project_id: ProjectId,
    /// Initial thread title.
    pub title: String,
    /// Whether to generate an LLM title asynchronously after the initial turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generate_title: Option<bool>,
    /// Initial model selection.
    pub model_selection: ModelSelection,
    /// Initial runtime permission mode.
    pub runtime_mode: RuntimeMode,
    /// Initial provider interaction mode.
    pub interaction_mode: ProviderInteractionMode,
    /// Git branch name, or `null`.
    pub branch: Option<String>,
    /// Worktree path, or `null`.
    pub worktree_path: Option<String>,
    /// MCP link provenance, if created by a linked environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_origin: Option<LinkOrigin>,
    /// Remote parent provenance, if delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_from: Option<DelegatedFrom>,
    /// Optional native thread import descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_native_thread: Option<ImportedNativeThreadPayload>,
    /// Optional atomic initial message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<InitialThreadMessage>,
}

/// Wire-extended `thread.create` payload carrying `titleRenamed` and `workflowRun`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowThreadCreateCommand {
    /// Core `thread.create` command fields.
    #[serde(flatten)]
    pub base: ThreadCreateCommand,
    /// Whether the thread title was explicitly chosen by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_renamed: Option<bool>,
    /// Workflow run provenance for workflow agent steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<WorkflowOrigin>,
}

/// Queued continuation message embedded in `thread.metadata.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataContinuation {
    /// Message identifier.
    pub message_id: MessageId,
    /// Prompt text.
    pub text: String,
}

/// Wire payload for `thread.metadata.update` (`orchestrationV2.ts:3295-3311`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMetadataUpdateCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Updated title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Marks title regeneration in flight or complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regenerate_title: Option<bool>,
    /// Branch patch (absent/null/value).
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub branch: PatchValue<String>,
    /// Worktree path patch (absent/null/value).
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub worktree_path: PatchValue<String>,
    /// Compare-and-swap precondition for `worktreePath`.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub expected_worktree_path: PatchValue<String>,
    /// Compare-and-swap precondition for `branch`.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub expected_branch: PatchValue<String>,
    /// Atomic continuation message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<MetadataContinuation>,
    /// Require the thread to have zero runs and zero messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_empty: Option<bool>,
    /// Limit recovery patch (absent/null/value).
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub limit_recovery: PatchValue<LimitRecoveryUpdate>,
    /// Legacy pull-request link patch (absent/null/value).
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub linked_pull_request: PatchValue<ThreadLinkedPullRequest>,
}

/// Dispatch mode for `message.dispatch` (`orchestrationV2.ts:3454-3463`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DispatchMode {
    /// Create run in `preparing` until workspace setup finishes.
    #[serde(rename_all = "camelCase")]
    DeferStart {
        /// Optional workspace launch strategy.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_strategy: Option<WorkspaceStrategy>,
    },
    /// Steer the currently active run.
    #[serde(rename_all = "camelCase")]
    SteerActive {
        /// Target active run ID.
        target_run_id: RunId,
    },
    /// Interrupt and restart the active run with this message.
    #[serde(rename_all = "camelCase")]
    RestartActive {
        /// Target active run ID.
        target_run_id: RunId,
    },
    /// Enqueue behind active work.
    QueueAfterActive,
    /// Start execution immediately.
    StartImmediately,
}

/// Wire payload for `message.dispatch` (`orchestrationV2.ts:3420-3464`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDispatchCommand {
    /// Optional background-work notification metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<OrchestrationNotification>,
    /// Pending guest message ID when promoting a guest message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_message_id: Option<GuestMessageId>,
    /// Message creator actor.
    pub created_by: OrchestrationActor,
    /// Client/surface that originated the message.
    pub creation_source: CreationSource,
    /// Originating scheduled task ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_task_id: Option<ScheduledTaskId>,
    /// Local sending thread ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_thread_id: Option<ThreadId>,
    /// Cross-environment sender attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_sender: Option<LinkedSender>,
    /// Idle compaction trigger reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_compaction: Option<String>,
    /// Start a fresh provider session without prior thread history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_context: Option<bool>,
    /// Optional JSON Schema requiring a typed structured output from this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<RunOutputSchema>,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Target message ID.
    pub message_id: MessageId,
    /// Prompt text.
    pub text: String,
    /// Optional inline composer context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Chat attachments.
    #[serde(default)]
    pub attachments: Vec<Value>,
    /// Optional title generation seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_seed: Option<String>,
    /// Optional model selection override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelSelection>,
    /// Source plan reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_plan_ref: Option<super::common::SourcePlanRef>,
    /// Continuation references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_continuation_of_run_id: Option<RunId>,
    /// Usage-limit continuation reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit_continuation_of_run_id: Option<RunId>,
    /// Manual continuation reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_continuation_of_run_id: Option<RunId>,
    /// Steer follow-up run ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_follow_up_of_run_id: Option<RunId>,
    /// Steer follow-up run ordinal threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_follow_up_after_run_ordinal: Option<u64>,
    /// Usage limit recovery request ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit_recovery_request_id: Option<CommandId>,
    /// Requested delivery intent (`auto`, `steer`, `restart`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_intent: Option<String>,
    /// Delegated task completion batch reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_completion: Option<DelegatedCompletionRef>,
    /// Execution scheduling mode.
    pub dispatch_mode: DispatchMode,
}
