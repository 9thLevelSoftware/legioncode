//! Metadata helpers (`command_type`, `command_id`, internal command guard) for [`OrchestrationCommand`].

use crate::ids::CommandId;

use super::commands::OrchestrationCommand;

/// Server-only internal command types that must NOT be accepted on client dispatch endpoints.
pub const INTERNAL_COMMAND_TYPES: &[&str] = &[
    "thread.guest-share.open",
    "thread.guest-share.redeem",
    "thread.guest-message.submit",
    "thread.project.adopt-folder",
    "thread.pull-request-watch.checks-reopen",
    "thread.pull-request-watch.sync",
    "delegated_task.remote.request",
    "delegated_task.remote.complete",
    "delegated_task.remote.stop",
    "checkpoint.rollback.fail",
    "runtime-request.delivery.fail",
    "thread.interrupt.unresponsive",
    "thread.background-work.settle",
    "secret_request.record",
    "link_request.record",
];

/// Returns `true` when `command_type` belongs to server-only `OrchestrationV2InternalCommand`.
pub fn is_internal_command_type(command_type: &str) -> bool {
    INTERNAL_COMMAND_TYPES.contains(&command_type)
}

impl OrchestrationCommand {
    /// Returns the wire `"type"` tag for this command.
    pub fn command_type(&self) -> &'static str {
        match self {
            Self::ThreadStop(_) => "thread.stop",
            Self::ThreadCreate(_) => "thread.create",
            Self::ThreadArchive { .. } => "thread.archive",
            Self::ThreadGuestShareClose { .. } => "thread.guest-share.close",
            Self::ThreadGuestMessageDismiss { .. } => "thread.guest-message.dismiss",
            Self::ThreadUnarchive { .. } => "thread.unarchive",
            Self::ThreadDelete { .. } => "thread.delete",
            Self::ThreadSettle { .. } => "thread.settle",
            Self::ThreadAutoSettle { .. } => "thread.auto-settle",
            Self::ThreadUnsettle(_) => "thread.unsettle",
            Self::ThreadSnooze { .. } => "thread.snooze",
            Self::ThreadUnsnooze { .. } => "thread.unsnooze",
            Self::ThreadAutoSettleSet { .. } => "thread.auto-settle.set",
            Self::ThreadPin { .. } => "thread.pin",
            Self::ThreadUnpin { .. } => "thread.unpin",
            Self::ThreadTagSet { .. } => "thread.tag.set",
            Self::ThreadPinReorder { .. } => "thread.pin.reorder",
            Self::ThreadActiveReorder { .. } => "thread.active.reorder",
            Self::ThreadVisit(_) => "thread.visit",
            Self::ThreadMarkUnread { .. } => "thread.mark-unread",
            Self::ThreadMetadataUpdate(_) => "thread.metadata.update",
            Self::ThreadPullRequestLink(_) => "thread.pull-request.link",
            Self::ThreadPullRequestUnlink { .. } => "thread.pull-request.unlink",
            Self::ThreadPullRequestLinkSync(_) => "thread.pull-request-link.sync",
            Self::ThreadIssueLink(_) => "thread.issue.link",
            Self::ThreadIssueUnlink(_) => "thread.issue.unlink",
            Self::ThreadPullRequestWatch(_) => "thread.pull-request.watch",
            Self::ThreadPullRequestSync(_) => "thread.pull-request.sync",
            Self::ThreadTitleRegenerationComplete(_) => "thread.title.regeneration.complete",
            Self::ThreadRuntimeModeSet { .. } => "thread.runtime-mode.set",
            Self::ThreadInteractionModeSet { .. } => "thread.interaction-mode.set",
            Self::ThreadModelSelectionSet { .. } => "thread.model-selection.set",
            Self::ProviderSessionDetach { .. } => "provider-session.detach",
            Self::MessageDispatch(_) => "message.dispatch",
            Self::PreparedRunRelease { .. } => "prepared-run.release",
            Self::NotificationDeliveryAccept { .. } => "notification.delivery.accept",
            Self::PreparedRunProgress { .. } => "prepared-run.progress",
            Self::PreparedRunFail { .. } => "prepared-run.fail",
            Self::PreparedRunRetry { .. } => "prepared-run.retry",
            Self::SubagentStop { .. } => "subagent.stop",
            Self::RunInterrupt(_) => "run.interrupt",
            Self::QueuedMessagePromoteToSteer { .. } => "queued-message.promote-to-steer",
            Self::QueueResume { .. } => "queue.resume",
            Self::QueuedRunStart { .. } => "queued-run.start",
            Self::QueuedRunReorder(_) => "queued-run.reorder",
            Self::QueuedRunCancel { .. } => "queued-run.cancel",
            Self::QueuedRunEdit(_) => "queued-run.edit",
            Self::RuntimeRequestRespond(_) => "runtime-request.respond",
            Self::ThreadUserInputDismiss { .. } => "thread.user-input.dismiss",
            Self::CheckpointRollback(_) => "checkpoint.rollback",
            Self::ThreadFork(_) => "thread.fork",
            Self::ThreadMergeBack(_) => "thread.merge_back",
            Self::DelegatedTaskRequest(_) => "delegated_task.request",
            Self::DelegatedTaskWakePolicy { .. } => "delegated_task.wake-policy",
            Self::DelegatedTaskCompletionDeliveryAcknowledge(_) => {
                "delegated_task.completion-delivery.acknowledge"
            }
            Self::DelegatedTaskCompletionDeliveryDispose { .. } => {
                "delegated_task.completion-delivery.dispose"
            }
            Self::ThreadCreatedRecord(_) => "thread.created.record",
            Self::ProviderSwitch(_) => "provider.switch",
        }
    }

    /// Returns the command's idempotency [`CommandId`].
    pub fn command_id(&self) -> &CommandId {
        match self {
            Self::ThreadArchive { command_id, .. }
            | Self::ThreadGuestShareClose { command_id, .. }
            | Self::ThreadGuestMessageDismiss { command_id, .. }
            | Self::ThreadUnarchive { command_id, .. }
            | Self::ThreadDelete { command_id, .. }
            | Self::ThreadSettle { command_id, .. }
            | Self::ThreadAutoSettle { command_id, .. }
            | Self::ThreadSnooze { command_id, .. }
            | Self::ThreadUnsnooze { command_id, .. }
            | Self::ThreadAutoSettleSet { command_id, .. }
            | Self::ThreadPin { command_id, .. }
            | Self::ThreadUnpin { command_id, .. }
            | Self::ThreadTagSet { command_id, .. }
            | Self::ThreadPinReorder { command_id, .. }
            | Self::ThreadActiveReorder { command_id, .. }
            | Self::ThreadMarkUnread { command_id, .. }
            | Self::ThreadPullRequestUnlink { command_id, .. }
            | Self::ThreadRuntimeModeSet { command_id, .. }
            | Self::ThreadInteractionModeSet { command_id, .. }
            | Self::ThreadModelSelectionSet { command_id, .. }
            | Self::ProviderSessionDetach { command_id, .. }
            | Self::PreparedRunRelease { command_id, .. }
            | Self::NotificationDeliveryAccept { command_id, .. }
            | Self::PreparedRunProgress { command_id, .. }
            | Self::PreparedRunFail { command_id, .. }
            | Self::PreparedRunRetry { command_id, .. }
            | Self::SubagentStop { command_id, .. }
            | Self::QueuedMessagePromoteToSteer { command_id, .. }
            | Self::QueueResume { command_id, .. }
            | Self::QueuedRunStart { command_id, .. }
            | Self::QueuedRunCancel { command_id, .. }
            | Self::ThreadUserInputDismiss { command_id, .. }
            | Self::DelegatedTaskWakePolicy { command_id, .. }
            | Self::DelegatedTaskCompletionDeliveryDispose { command_id, .. } => command_id,
            Self::ThreadStop(c) => &c.command_id,
            Self::ThreadCreate(c) => &c.command_id,
            Self::ThreadUnsettle(c) => &c.command_id,
            Self::ThreadVisit(c) => &c.command_id,
            Self::ThreadMetadataUpdate(c) => &c.command_id,
            Self::ThreadPullRequestLink(c) => &c.command_id,
            Self::ThreadPullRequestLinkSync(c) => &c.command_id,
            Self::ThreadIssueLink(c) => &c.command_id,
            Self::ThreadIssueUnlink(c) => &c.command_id,
            Self::ThreadPullRequestWatch(c) => &c.command_id,
            Self::ThreadPullRequestSync(c) => &c.command_id,
            Self::ThreadTitleRegenerationComplete(c) => &c.command_id,
            Self::MessageDispatch(c) => &c.command_id,
            Self::RunInterrupt(c) => &c.command_id,
            Self::QueuedRunReorder(c) => &c.command_id,
            Self::QueuedRunEdit(c) => &c.command_id,
            Self::RuntimeRequestRespond(c) => &c.command_id,
            Self::CheckpointRollback(c) => &c.command_id,
            Self::ThreadFork(c) => &c.command_id,
            Self::ThreadMergeBack(c) => &c.command_id,
            Self::DelegatedTaskRequest(c) => &c.command_id,
            Self::DelegatedTaskCompletionDeliveryAcknowledge(c) => &c.command_id,
            Self::ThreadCreatedRecord(c) => &c.command_id,
            Self::ProviderSwitch(c) => &c.command_id,
        }
    }
}
