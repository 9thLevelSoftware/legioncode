//! Pull-request, fork, and delegated-task command payload structs.

use serde::{Deserialize, Serialize};

use crate::ids::{CommandId, NodeId, ProjectId, RunId, ThreadId};
use crate::policy::RuntimeMode;
use crate::pull_requests::{
    ThreadLinkedPullRequest, ThreadPullRequestLinkSource, ThreadPullRequestSnapshot,
    ThreadPullRequestStack,
};

use super::command_payloads::ProviderInteractionMode;
use super::common::{CreationSource, OrchestrationActor};
use super::model_selection::{ModelSelection, ThreadForkSourcePoint, WorkspaceStrategy};

/// Watch link metadata nested in `thread.pull-request.watch` (`orchestrationV2.ts:3362-3368`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestWatchLinkInfo {
    /// PR web URL.
    pub url: String,
    /// Link source.
    pub source: ThreadPullRequestLinkSource,
    /// Optional repository web URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_web_url: Option<String>,
}

/// Wire payload for `thread.pull-request.watch` (`orchestrationV2.ts:3353-3369`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestWatchCommand {
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
    /// Enable or disable watching.
    pub watching: bool,
    /// Watch duration (`"checks"` or `"merged"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Whether checks were pending at watch start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_pending: Option<bool>,
    /// Optional link creation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<PullRequestWatchLinkInfo>,
}

/// Precondition snapshot for `thread.pull-request.sync` (`orchestrationV2.ts:3376-3382`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestSyncExpected {
    /// Expected workspace root path.
    pub workspace_root: String,
    /// Expected branch name, or `null`.
    pub branch: Option<String>,
    /// Expected worktree path, or `null`.
    pub worktree_path: Option<String>,
    /// Expected legacy linked PR, or `null`.
    pub linked_pull_request: Option<ThreadLinkedPullRequest>,
    /// Expected branch PR, or `null`.
    pub branch_pull_request: Option<ThreadLinkedPullRequest>,
}

/// Wire payload for `thread.pull-request.sync` (`orchestrationV2.ts:3371-3385`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestSyncCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Project identifier.
    pub project_id: ProjectId,
    /// Snapshot sequence precondition.
    pub snapshot_sequence: u64,
    /// Expected workspace/PR state.
    pub expected: ThreadPullRequestSyncExpected,
    /// Updated branch PR, or `null`.
    pub branch_pull_request: Option<ThreadLinkedPullRequest>,
    /// Optional updated legacy linked PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_pull_request: Option<ThreadLinkedPullRequest>,
}

/// Wire payload for `thread.pull-request-link.sync` (`orchestrationV2.ts:3328-3334`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPullRequestLinkSyncCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Forge host.
    pub host: String,
    /// Canonical `owner/repo`.
    pub repository: String,
    /// Positive PR number.
    pub number: u32,
    /// Refreshed PR snapshot.
    pub snapshot: ThreadPullRequestSnapshot,
    /// Refreshed native stack, or `null`.
    pub stack: Option<ThreadPullRequestStack>,
}

/// Wire payload for `thread.fork` (`orchestrationV2.ts:3578-3587`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkCommand {
    /// Actor requesting the fork.
    pub created_by: OrchestrationActor,
    /// Client/surface requesting the fork.
    pub creation_source: CreationSource,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Source thread ID.
    pub source_thread_id: ThreadId,
    /// Target new thread ID.
    pub target_thread_id: ThreadId,
    /// Point in history where the fork branches.
    pub source_point: ThreadForkSourcePoint,
    /// Optional initial title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Whether the fork is a hidden side chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_chat: Option<bool>,
    /// Optional timestamp override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Wire payload for `delegated_task.request` (`orchestrationV2.ts:3598-3613`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedTaskRequestCommand {
    /// Actor requesting the task.
    pub created_by: OrchestrationActor,
    /// Originating surface.
    pub creation_source: CreationSource,
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Parent thread ID.
    pub parent_thread_id: ThreadId,
    /// Parent run ID.
    pub parent_run_id: RunId,
    /// Parent execution node ID.
    pub parent_node_id: NodeId,
    /// Task prompt text.
    pub task: String,
    /// Optional task title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Task model selection.
    pub model_selection: ModelSelection,
    /// Task runtime mode.
    pub runtime_mode: RuntimeMode,
    /// Task interaction mode.
    pub interaction_mode: ProviderInteractionMode,
    /// Optional workspace launch strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_strategy: Option<WorkspaceStrategy>,
    /// Wake policy (`always` or `settled_only`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_wake: Option<String>,
    /// Optional timestamp override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Wire payload for `thread.stop` (`orchestrationV2.ts:3131-3136`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStopCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Optional stop reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether to cascade stop into delegated child tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_delegated_tasks: Option<bool>,
}

/// Wire payload for `run.interrupt` (`orchestrationV2.ts:3499-3504`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInterruptCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Active run ID.
    pub run_id: RunId,
    /// Optional interrupt reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether queued follow-ups should pause after interrupting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_queue: Option<bool>,
}

/// Wire payload for `queued-run.reorder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedRunReorderCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Run ID being moved.
    pub run_id: RunId,
    /// Destination anchor run ID, or `null` to append to the tail.
    pub before_run_id: Option<RunId>,
}

/// Wire payload for `delegated_task.completion-delivery.acknowledge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedTaskCompletionAckCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Parent thread ID.
    pub parent_thread_id: ThreadId,
    /// Delegated task node ID.
    pub task_id: NodeId,
    /// Run ID that observed completion, or `null`.
    pub observed_by_run_id: Option<RunId>,
}

/// Wire payload for `provider.switch` (`orchestrationV2.ts:3646-3650`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSwitchCommand {
    /// Idempotency command ID.
    pub command_id: CommandId,
    /// Target thread ID.
    pub thread_id: ThreadId,
    /// Destination provider instance and model selection.
    pub model_selection: ModelSelection,
}
