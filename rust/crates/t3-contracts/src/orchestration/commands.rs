//! Client-dispatchable `OrchestrationV2Command` union (`orchestrationV2.ts:3125-3651`).

use serde::{Deserialize, Serialize};

use crate::ids::{
    CommandId, GuestMessageId, MessageId, NodeId, ProviderSessionId, RunId, RuntimeRequestId,
    ThreadId,
};
use crate::policy::RuntimeMode;

use super::command_payloads::{
    MessageDispatchCommand, ProviderInteractionMode, ThreadCreateCommand,
    ThreadMetadataUpdateCommand,
};
use super::command_payloads_extra::{
    CheckpointRollbackCommand, CompletionWakePolicy, PreparedRunPhase, QueuedRunEditCommand,
    RuntimeRequestRespondCommand, ThreadCreatedRecordCommand, ThreadIssueLinkCommand,
    ThreadIssueUnlinkCommand, ThreadMergeBackCommand, ThreadPullRequestLinkCommand,
    ThreadTitleRegenerationCompleteCommand, ThreadUnsettleCommand, ThreadVisitCommand,
    UserActionReason,
};
use super::command_payloads_pr::{
    DelegatedTaskCompletionAckCommand, DelegatedTaskRequestCommand, ProviderSwitchCommand,
    QueuedRunReorderCommand, RunInterruptCommand, ThreadForkCommand,
    ThreadPullRequestLinkSyncCommand, ThreadPullRequestSyncCommand, ThreadPullRequestWatchCommand,
    ThreadStopCommand,
};
use super::model_selection::{ModelSelection, ProviderFailure};

/// Client-dispatchable orchestration v2 commands (58 variants, tagged by `"type"`).
///
/// Server-only `OrchestrationV2InternalCommand` variants (`3654-3886`) are intentionally
/// excluded so deserializing client RPC payloads rejects them automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OrchestrationCommand {
    /// Request stopping an active thread and optionally its delegated tasks.
    #[serde(rename = "thread.stop")]
    ThreadStop(ThreadStopCommand),
    /// Create a new thread.
    #[serde(rename = "thread.create")]
    ThreadCreate(ThreadCreateCommand),
    /// Archive a thread.
    #[serde(rename = "thread.archive", rename_all = "camelCase")]
    ThreadArchive {
        command_id: CommandId,
        thread_id: ThreadId,
    },
    /// Close an active guest share link on a thread.
    #[serde(rename = "thread.guest-share.close", rename_all = "camelCase")]
    ThreadGuestShareClose {
        command_id: CommandId,
        thread_id: ThreadId,
    },
    /// Dismiss a pending guest-submitted message.
    #[serde(rename = "thread.guest-message.dismiss", rename_all = "camelCase")]
    ThreadGuestMessageDismiss {
        command_id: CommandId,
        thread_id: ThreadId,
        guest_message_id: GuestMessageId,
    },
    /// Unarchive a thread.
    #[serde(rename = "thread.unarchive", rename_all = "camelCase")]
    ThreadUnarchive {
        command_id: CommandId,
        thread_id: ThreadId,
    },
    /// Soft-delete a thread (optionally only if unused).
    #[serde(rename = "thread.delete", rename_all = "camelCase")]
    ThreadDelete {
        command_id: CommandId,
        thread_id: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only_if_unused: Option<bool>,
    },
    /// Explicitly settle a thread.
    #[serde(rename = "thread.settle", rename_all = "camelCase")]
    ThreadSettle {
        command_id: CommandId,
        thread_id: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_latest_run_id: Option<RunId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settled_at: Option<String>,
    },
    /// Auto-settle a thread if unchanged since `snapshot_at`.
    #[serde(rename = "thread.auto-settle", rename_all = "camelCase")]
    ThreadAutoSettle {
        command_id: CommandId,
        thread_id: ThreadId,
        snapshot_at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settled_at: Option<String>,
    },
    /// Unsettle a thread back into Active.
    #[serde(rename = "thread.unsettle")]
    ThreadUnsettle(ThreadUnsettleCommand),
    /// Snooze a thread until `snoozed_until`.
    #[serde(rename = "thread.snooze", rename_all = "camelCase")]
    ThreadSnooze {
        command_id: CommandId,
        thread_id: ThreadId,
        snoozed_until: String,
    },
    /// Unsnooze a thread.
    #[serde(rename = "thread.unsnooze", rename_all = "camelCase")]
    ThreadUnsnooze {
        command_id: CommandId,
        thread_id: ThreadId,
        reason: UserActionReason,
    },
    /// Enable or disable per-thread auto-settlement.
    #[serde(rename = "thread.auto-settle.set", rename_all = "camelCase")]
    ThreadAutoSettleSet {
        command_id: CommandId,
        thread_id: ThreadId,
        enabled: bool,
    },
    /// Pin a thread in the sidebar.
    #[serde(rename = "thread.pin", rename_all = "camelCase")]
    ThreadPin {
        command_id: CommandId,
        thread_id: ThreadId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        order_key: Option<String>,
    },
    /// Unpin a thread.
    #[serde(rename = "thread.unpin", rename_all = "camelCase")]
    ThreadUnpin {
        command_id: CommandId,
        thread_id: ThreadId,
    },
    /// Add or remove a thread tag.
    #[serde(rename = "thread.tag.set", rename_all = "camelCase")]
    ThreadTagSet {
        command_id: CommandId,
        thread_id: ThreadId,
        tag: String,
        tagged: bool,
    },
    /// Reorder a pinned thread.
    #[serde(rename = "thread.pin.reorder", rename_all = "camelCase")]
    ThreadPinReorder {
        command_id: CommandId,
        thread_id: ThreadId,
        order_key: String,
    },
    /// Reorder an active thread.
    #[serde(rename = "thread.active.reorder", rename_all = "camelCase")]
    ThreadActiveReorder {
        command_id: CommandId,
        thread_id: ThreadId,
        order_key: String,
    },
    /// Record a user visit or open event on a thread.
    #[serde(rename = "thread.visit")]
    ThreadVisit(ThreadVisitCommand),
    /// Mark a thread as unread.
    #[serde(rename = "thread.mark-unread", rename_all = "camelCase")]
    ThreadMarkUnread {
        command_id: CommandId,
        thread_id: ThreadId,
    },
    /// Update thread metadata fields.
    #[serde(rename = "thread.metadata.update")]
    ThreadMetadataUpdate(ThreadMetadataUpdateCommand),
    /// Link a pull request to a thread.
    #[serde(rename = "thread.pull-request.link")]
    ThreadPullRequestLink(ThreadPullRequestLinkCommand),
    /// Unlink a pull request from a thread.
    #[serde(rename = "thread.pull-request.unlink", rename_all = "camelCase")]
    ThreadPullRequestUnlink {
        command_id: CommandId,
        thread_id: ThreadId,
        host: String,
        repository: String,
        number: u32,
    },
    /// Sync snapshot/stack data for a linked pull request.
    #[serde(rename = "thread.pull-request-link.sync")]
    ThreadPullRequestLinkSync(ThreadPullRequestLinkSyncCommand),
    /// Link an issue to a thread.
    #[serde(rename = "thread.issue.link")]
    ThreadIssueLink(ThreadIssueLinkCommand),
    /// Unlink an issue from a thread.
    #[serde(rename = "thread.issue.unlink")]
    ThreadIssueUnlink(ThreadIssueUnlinkCommand),
    /// Start or stop watching a linked pull request.
    #[serde(rename = "thread.pull-request.watch")]
    ThreadPullRequestWatch(ThreadPullRequestWatchCommand),
    /// Synchronize legacy pull-request metadata.
    #[serde(rename = "thread.pull-request.sync")]
    ThreadPullRequestSync(ThreadPullRequestSyncCommand),
    /// Record completion of an asynchronous title regeneration request.
    #[serde(rename = "thread.title.regeneration.complete")]
    ThreadTitleRegenerationComplete(ThreadTitleRegenerationCompleteCommand),
    /// Change a thread's runtime permission mode.
    #[serde(rename = "thread.runtime-mode.set", rename_all = "camelCase")]
    ThreadRuntimeModeSet {
        command_id: CommandId,
        thread_id: ThreadId,
        runtime_mode: RuntimeMode,
    },
    /// Change a thread's provider interaction mode.
    #[serde(rename = "thread.interaction-mode.set", rename_all = "camelCase")]
    ThreadInteractionModeSet {
        command_id: CommandId,
        thread_id: ThreadId,
        interaction_mode: ProviderInteractionMode,
    },
    /// Update a thread's model selection.
    #[serde(rename = "thread.model-selection.set", rename_all = "camelCase")]
    ThreadModelSelectionSet {
        command_id: CommandId,
        thread_id: ThreadId,
        model_selection: ModelSelection,
    },
    /// Detach a provider session from a thread.
    #[serde(rename = "provider-session.detach", rename_all = "camelCase")]
    ProviderSessionDetach {
        command_id: CommandId,
        thread_id: ThreadId,
        provider_session_id: ProviderSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Dispatch a user, agent, or notification message to a thread.
    #[serde(rename = "message.dispatch")]
    MessageDispatch(MessageDispatchCommand),
    /// Release a prepared run after workspace setup finishes.
    #[serde(rename = "prepared-run.release", rename_all = "camelCase")]
    PreparedRunRelease {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
    },
    /// Accept delivery of a pending notification wake message.
    #[serde(rename = "notification.delivery.accept", rename_all = "camelCase")]
    NotificationDeliveryAccept {
        command_id: CommandId,
        thread_id: ThreadId,
        message_id: MessageId,
    },
    /// Report progress on a preparing run.
    #[serde(rename = "prepared-run.progress", rename_all = "camelCase")]
    PreparedRunProgress {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
        phase: PreparedRunPhase,
    },
    /// Mark a preparing run as failed.
    #[serde(rename = "prepared-run.fail", rename_all = "camelCase")]
    PreparedRunFail {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
        failure: ProviderFailure,
    },
    /// Retry workspace preparation for a failed prepared run.
    #[serde(rename = "prepared-run.retry", rename_all = "camelCase")]
    PreparedRunRetry {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
    },
    /// Stop an active subagent node.
    #[serde(rename = "subagent.stop", rename_all = "camelCase")]
    SubagentStop {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
        subagent_id: NodeId,
    },
    /// Interrupt an active run.
    #[serde(rename = "run.interrupt")]
    RunInterrupt(RunInterruptCommand),
    /// Promote a queued message to steer the active run.
    #[serde(rename = "queued-message.promote-to-steer", rename_all = "camelCase")]
    QueuedMessagePromoteToSteer {
        command_id: CommandId,
        thread_id: ThreadId,
        queued_run_id: RunId,
        target_run_id: RunId,
    },
    /// Resume held queued runs on a thread.
    #[serde(rename = "queue.resume", rename_all = "camelCase")]
    QueueResume {
        command_id: CommandId,
        thread_id: ThreadId,
    },
    /// Force-start a queued run immediately.
    #[serde(rename = "queued-run.start", rename_all = "camelCase")]
    QueuedRunStart {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
    },
    /// Reorder a queued run ahead of `before_run_id`.
    #[serde(rename = "queued-run.reorder")]
    QueuedRunReorder(QueuedRunReorderCommand),
    /// Cancel a queued run.
    #[serde(rename = "queued-run.cancel", rename_all = "camelCase")]
    QueuedRunCancel {
        command_id: CommandId,
        thread_id: ThreadId,
        run_id: RunId,
    },
    /// Edit a queued run's prompt text and/or attachments.
    #[serde(rename = "queued-run.edit")]
    QueuedRunEdit(QueuedRunEditCommand),
    /// Respond to a pending approval or structured question request.
    #[serde(rename = "runtime-request.respond")]
    RuntimeRequestRespond(RuntimeRequestRespondCommand),
    /// Dismiss a pending user-input prompt.
    #[serde(rename = "thread.user-input.dismiss", rename_all = "camelCase")]
    ThreadUserInputDismiss {
        command_id: CommandId,
        thread_id: ThreadId,
        request_id: RuntimeRequestId,
    },
    /// Roll back conversation and optionally workspace files to a checkpoint.
    #[serde(rename = "checkpoint.rollback")]
    CheckpointRollback(CheckpointRollbackCommand),
    /// Fork a thread at a specific run or checkpoint.
    #[serde(rename = "thread.fork")]
    ThreadFork(ThreadForkCommand),
    /// Merge context from a forked thread back into its target thread.
    #[serde(rename = "thread.merge_back")]
    ThreadMergeBack(ThreadMergeBackCommand),
    /// Spawn a delegated child task thread.
    #[serde(rename = "delegated_task.request")]
    DelegatedTaskRequest(DelegatedTaskRequestCommand),
    /// Update completion wake policy on a delegated task.
    #[serde(rename = "delegated_task.wake-policy", rename_all = "camelCase")]
    DelegatedTaskWakePolicy {
        command_id: CommandId,
        parent_thread_id: ThreadId,
        task_id: NodeId,
        completion_wake: CompletionWakePolicy,
    },
    /// Acknowledge delivery of a delegated task completion.
    #[serde(rename = "delegated_task.completion-delivery.acknowledge")]
    DelegatedTaskCompletionDeliveryAcknowledge(DelegatedTaskCompletionAckCommand),
    /// Dispose a delegated task completion delivery cohort.
    #[serde(
        rename = "delegated_task.completion-delivery.dispose",
        rename_all = "camelCase"
    )]
    DelegatedTaskCompletionDeliveryDispose {
        command_id: CommandId,
        parent_thread_id: ThreadId,
        task_id: NodeId,
    },
    /// Record a created thread link from a parent task node.
    #[serde(rename = "thread.created.record")]
    ThreadCreatedRecord(ThreadCreatedRecordCommand),
    /// Switch provider instance and model selection on a thread.
    #[serde(rename = "provider.switch")]
    ProviderSwitch(ProviderSwitchCommand),
}
