//! Additional command payload structs keeping each orchestration module concise.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{
    CheckpointId, CheckpointScopeId, CommandId, IssueTrackerKind, MessageId, NodeId, ProjectId,
    ProviderSessionId, RunId, RuntimeRequestId, ThreadId,
};
use crate::policy::RuntimeMode;
use crate::pull_requests::ThreadPullRequestLinkSource;

use super::command_payloads::ProviderInteractionMode;
use super::common::{CreationSource, JsonRecord, OrchestrationActor, PatchValue};
use super::model_selection::{ModelSelection, ProviderFailure, ThreadForkSourcePoint};

/// Literal `"user"` reason required by `thread.unsettle` and `thread.unsnooze`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserActionReason {
    /// Explicit user action.
    User,
}

/// Workspace preparation phase for `prepared-run.progress`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreparedRunPhase {
    /// Git worktree creation phase.
    Worktree,
    /// Project setup script execution phase.
    Setup,
}

/// Wake timing policy for delegated subagent tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionWakePolicy {
    /// Wake the parent thread whenever the task completes.
    Always,
    /// Wake the parent thread only after sibling tasks settle.
    SettledOnly,
}

/// Provider approval decision on a `runtime-request.respond` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderApprovalDecision {
    /// Approve this request once.
    Accept,
    /// Approve for the current session.
    AcceptForSession,
    /// Always approve matching requests.
    AcceptAlways,
    /// Decline and let the model adapt.
    Decline,
    /// Cancel the operation.
    Cancel,
}

/// Outcome payload on `thread.title.regeneration.complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTitleRegenerationOutcome {
    /// Outcome status (`renamed`, `unchanged`, `failed`).
    pub status: String,
    /// Optional status detail message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Payload for `thread.unsettle`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUnsettleCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Always `"user"`.
    pub reason: UserActionReason,
    /// Optional timestamp restoring the prior unsettled anchor.
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub restore_unsettled_at: PatchValue<String>,
}

/// Payload for `thread.visit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadVisitCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// ISO visit timestamp.
    pub visited_at: String,
    /// Whether the user opened the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened: Option<bool>,
    /// Optional palette search query that opened the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_query: Option<String>,
}

/// Payload for `thread.pull-request.link`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestLinkCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Normalized forge host.
    pub host: String,
    /// Canonical `owner/repo`.
    pub repository: String,
    /// Positive PR number.
    pub number: u32,
    /// Web URL.
    pub url: String,
    /// Link origin source.
    pub source: ThreadPullRequestLinkSource,
    /// Optional repository web URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_web_url: Option<String>,
}

/// Issue link source (`issue.ts:494`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadIssueLinkSource {
    /// Linked manually by the user.
    Manual,
    /// Linked by an agent.
    Agent,
}

/// Payload for `thread.issue.link`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadIssueLinkCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Tracker kind slug.
    pub tracker: IssueTrackerKind,
    /// Tracker host.
    pub host: String,
    /// Workspace/org scope.
    pub scope: String,
    /// Issue key.
    pub key: String,
    /// Owning project ID.
    pub project_id: ProjectId,
    /// Issue web URL.
    pub url: String,
    /// Human-readable key.
    pub display_key: String,
    /// Issue title.
    pub title: String,
    /// Link source.
    pub source: ThreadIssueLinkSource,
}

/// Payload for `thread.issue.unlink`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadIssueUnlinkCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Tracker kind slug.
    pub tracker: IssueTrackerKind,
    /// Tracker host.
    pub host: String,
    /// Workspace/org scope.
    pub scope: String,
    /// Issue key.
    pub key: String,
}

/// Payload for `thread.title.regeneration.complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTitleRegenerationCompleteCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Original regeneration request ID.
    pub request_id: CommandId,
    /// Generated title, if renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Outcome details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ThreadTitleRegenerationOutcome>,
}

/// Payload for `queued-run.edit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedRunEditCommand {
    /// Optional inline composer context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Queued run ID.
    pub run_id: RunId,
    /// Updated prompt text.
    pub text: String,
    /// Replacement attachments list, or absent to preserve existing attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Value>>,
}

/// Payload for `runtime-request.respond`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRequestRespondCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Runtime request identifier.
    pub request_id: RuntimeRequestId,
    /// Approval decision, when answering an approval prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ProviderApprovalDecision>,
    /// Optional user feedback text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Structured user-input question answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<JsonRecord>,
    /// Attachments grouped by question ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments_by_question_id: Option<JsonRecord>,
}

/// Payload for `thread.merge_back`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMergeBackCommand {
    /// Actor requesting merge-back.
    pub created_by: OrchestrationActor,
    /// Client/surface requesting merge-back.
    pub creation_source: CreationSource,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Source fork thread ID.
    pub source_thread_id: ThreadId,
    /// Target parent thread ID.
    pub target_thread_id: ThreadId,
    /// Source history point to merge from.
    pub source_point: ThreadForkSourcePoint,
    /// Optional timestamp override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Payload for `checkpoint.rollback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRollbackCommand {
    /// Whether to restore workspace files alongside conversation state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_files: Option<bool>,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Checkpoint scope ID.
    pub scope_id: CheckpointScopeId,
    /// Target checkpoint ID.
    pub checkpoint_id: CheckpointId,
}

/// Payload for `thread.created.record`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCreatedRecordCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Parent thread ID.
    pub parent_thread_id: ThreadId,
    /// Parent run ID.
    pub parent_run_id: RunId,
    /// Parent execution node ID.
    pub parent_node_id: NodeId,
    /// Target newly created thread ID.
    pub target_thread_id: ThreadId,
    /// Target initial run ID, or `null`.
    pub target_run_id: Option<RunId>,
}

/// Unused alias keeping imported types visible across payload splits.
pub type ExtraCommandTypes = (
    MessageId,
    ProviderSessionId,
    RuntimeMode,
    ProviderInteractionMode,
    ModelSelection,
    ProviderFailure,
);
