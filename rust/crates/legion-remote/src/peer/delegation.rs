//! Remote delegated task follower tracker with `stop_count` fencing and `BUG-07` fix.
//!
//! Mirrors `apps/server/src/peer/RemoteDelegation.ts`, fixing `BUG-07`: when a local parent
//! thread is deleted (`CallerCancelReason::ParentDeleted`), a `thread_not_found` follow failure
//! MUST still emit [`RemoteStopCommand::StopThere`] so the remote child thread is never orphaned.

#[path = "delegation_types.rs"]
mod types;

use legion_contracts::ids::{NodeId, ThreadId};

pub use types::{
    CallerCancelReason, CompletionStoppedThere, FollowAction, FollowErrorKind, RemoteChildRef,
    RemoteStopCommand, RemoteTaskTerminalStatus,
};

/// Per-slice wait duration when polling a remote thread (`cli/thread.ts:42`).
pub const WAIT_SLICE_MS: u64 = 15_000;
/// Maximum total wait budget across wait slices (`RemoteDelegation.ts:54`).
pub const MAX_WAIT_MS: u64 = 600_000;
/// Consecutive unexpected defects before failing a remote task (`RemoteDelegation.ts:58`).
pub const FOLLOW_MAX_DEFECTS: u32 = 3;

/// Stateful per-task follower coordinator with `stop_count` fencing and `BUG-07` fix.
///
/// # Examples
///
/// ```
/// use legion_contracts::ids::{EnvironmentId, NodeId, ThreadId};
/// use legion_remote::peer::delegation::{
///     CallerCancelReason, FollowAction, FollowErrorKind, RemoteChildRef,
///     RemoteDelegationTracker, RemoteStopCommand, WAIT_SLICE_MS,
/// };
///
/// let child = RemoteChildRef {
///     environment_id: EnvironmentId::new("env-box")?,
///     thread_id: ThreadId::new("remote-thread-1")?,
///     label: "Box".to_owned(),
///     title: None,
///     run_id: None,
///     stopped_there: None,
///     stop_count: None,
/// };
/// let mut tracker = RemoteDelegationTracker::new(
///     NodeId::new("task-1")?,
///     ThreadId::new("parent-1")?,
///     child,
/// );
/// assert_eq!(tracker.next_wait_slice_ms(), Some(WAIT_SLICE_MS));
///
/// // BUG-07 fix: ParentDeleted + thread_not_found MUST emit StopThere instead of marking stopped.
/// let _ = tracker.on_caller_cancel(CallerCancelReason::ParentDeleted);
/// let action = tracker.on_follow_error(FollowErrorKind::ThreadNotFound, "thread not found");
/// assert!(matches!(action, FollowAction::SendStop(RemoteStopCommand::StopThere { .. })));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDelegationTracker {
    task_id: NodeId,
    parent_thread_id: ThreadId,
    remote_child: RemoteChildRef,
    local_result: Option<String>,
    local_status: Option<RemoteTaskTerminalStatus>,
    caller_cancel: Option<CallerCancelReason>,
    elapsed_wait_ms: u64,
    consecutive_defects: u32,
    follow_again: bool,
}

impl RemoteDelegationTracker {
    /// Creates a follower tracker for `(parent_thread_id, task_id)` pointing at `remote_child`.
    pub fn new(task_id: NodeId, parent_thread_id: ThreadId, remote_child: RemoteChildRef) -> Self {
        Self {
            task_id,
            parent_thread_id,
            remote_child,
            local_result: None,
            local_status: None,
            caller_cancel: None,
            elapsed_wait_ms: 0,
            consecutive_defects: 0,
            follow_again: false,
        }
    }

    /// Task identifier on the local parent thread.
    pub fn task_id(&self) -> &NodeId {
        &self.task_id
    }

    /// Parent thread identifier on the local environment.
    pub fn parent_thread_id(&self) -> &ThreadId {
        &self.parent_thread_id
    }

    /// Current remote child metadata and stop fencing state.
    pub fn remote_child(&self) -> &RemoteChildRef {
        &self.remote_child
    }

    /// Total milliseconds waited so far within the current `MAX_WAIT_MS` cycle.
    pub fn elapsed_wait_ms(&self) -> u64 {
        self.elapsed_wait_ms
    }

    /// Returns the next bounded wait slice (`<= WAIT_SLICE_MS`), or `None` once `MAX_WAIT_MS` is spent.
    pub fn next_wait_slice_ms(&self) -> Option<u64> {
        let remaining = MAX_WAIT_MS.saturating_sub(self.elapsed_wait_ms);
        if remaining == 0 {
            None
        } else {
            Some(WAIT_SLICE_MS.min(remaining))
        }
    }

    /// Advances the wait clock by `slice_ms`, resetting `consecutive_defects` (`RemoteDelegation.ts:729`).
    pub fn record_wait_slice_timeout(&mut self, slice_ms: u64) -> FollowAction {
        self.consecutive_defects = 0;
        self.elapsed_wait_ms = self
            .elapsed_wait_ms
            .saturating_add(slice_ms)
            .min(MAX_WAIT_MS);
        let next = self.next_wait_slice_ms().unwrap_or_else(|| {
            self.elapsed_wait_ms = 0;
            WAIT_SLICE_MS
        });
        FollowAction::WaitSlice { timeout_ms: next }
    }

    /// Records local completion and checks whether the task still owes a remote stop.
    pub fn record_local_completion(
        &mut self,
        status: RemoteTaskTerminalStatus,
        result: String,
        stopped_there: Option<bool>,
    ) {
        self.local_status = Some(status);
        self.local_result = Some(result);
        self.remote_child.stopped_there = stopped_there;
    }

    /// Builds the fenced idempotency key (`stop[-{stop_count}][:{run_id}]`, `RemoteDelegation.ts:794-796`).
    pub fn stop_idempotency_key(&self) -> String {
        let base = match self.remote_child.stop_count {
            Some(count) => format!("stop-{count}"),
            None => "stop".to_owned(),
        };
        let suffix = match &self.remote_child.run_id {
            Some(run_id) => format!("{base}:{run_id}"),
            None => base,
        };
        format!("{suffix}:{}", self.remote_child.thread_id)
    }

    /// Records a parent/caller cancellation (`ParentDeleted`, `ParentStopped`, or `TaskCancelled`).
    ///
    /// Always emits [`RemoteStopCommand::StopThere`], bumping `stop_count` when stopping after a
    /// result or deleting the parent thread.
    pub fn on_caller_cancel(&mut self, reason: CallerCancelReason) -> RemoteStopCommand {
        let is_deleted = matches!(reason, CallerCancelReason::ParentDeleted);
        let cancel_text = match &reason {
            CallerCancelReason::TaskCancelled { reason } => reason.clone(),
            CallerCancelReason::ParentDeleted => Some("Parent thread deleted".to_owned()),
            CallerCancelReason::ParentStopped => None,
        };
        self.caller_cancel = Some(reason);
        if self.local_result.is_some() || is_deleted {
            let next_count = self.remote_child.stop_count.unwrap_or(0).saturating_add(1);
            self.remote_child.stop_count = Some(next_count);
            self.remote_child.stopped_there = Some(false);
        }
        RemoteStopCommand::StopThere {
            environment_id: self.remote_child.environment_id.clone(),
            thread_id: self.remote_child.thread_id.clone(),
            idempotency_key: self.stop_idempotency_key(),
            stop_count: self.remote_child.stop_count,
            reason: cancel_text,
        }
    }

    /// Checks whether `ack_stop_count` matches the latest owed `stop_count` (`Orchestrator.ts:10507-10510`).
    ///
    /// Stale acknowledgements from an older `stop_count` return `false` and leave `stopped_there == Some(false)`.
    pub fn accept_stop_acknowledgement(&mut self, ack_stop_count: Option<u64>) -> bool {
        let expected = self.remote_child.stop_count.unwrap_or(0);
        if ack_stop_count.unwrap_or(0) != expected {
            return false;
        }
        self.remote_child.stopped_there = Some(true);
        true
    }

    /// Decides what the follower should do after an error while waiting/reading the remote thread.
    ///
    /// Fixes `BUG-07`: if `self.caller_cancel == Some(CallerCancelReason::ParentDeleted)`,
    /// even `FollowErrorKind::ThreadNotFound` emits [`RemoteStopCommand::StopThere`] instead of
    /// assuming the remote thread disappeared.
    pub fn on_follow_error(&mut self, error: FollowErrorKind, message: &str) -> FollowAction {
        if matches!(self.caller_cancel, Some(CallerCancelReason::ParentDeleted)) {
            return FollowAction::SendStop(RemoteStopCommand::StopThere {
                environment_id: self.remote_child.environment_id.clone(),
                thread_id: self.remote_child.thread_id.clone(),
                idempotency_key: self.stop_idempotency_key(),
                stop_count: self.remote_child.stop_count,
                reason: Some("Parent thread deleted".to_owned()),
            });
        }

        if error == FollowErrorKind::Defect {
            self.consecutive_defects = self.consecutive_defects.saturating_add(1);
            if self.consecutive_defects < FOLLOW_MAX_DEFECTS {
                return FollowAction::RetryWithBackoff {
                    defect_count: self.consecutive_defects,
                };
            }
            return FollowAction::GiveUpWithWarning {
                result: format!(
                    "Following the thread {} in {} failed repeatedly, so it may still run there (task_cancel stops it).",
                    self.remote_child.thread_id, self.remote_child.label
                ),
            };
        }

        self.consecutive_defects = 0;
        let is_final = match error {
            FollowErrorKind::ThreadNotFound
            | FollowErrorKind::RunNotFound
            | FollowErrorKind::CapabilityDenied => true,
            FollowErrorKind::InvalidRequest { link_exists } => !link_exists,
            FollowErrorKind::Transient | FollowErrorKind::Defect => false,
        };
        if is_final {
            return FollowAction::CompleteLocal {
                status: RemoteTaskTerminalStatus::Failed,
                result: message.to_owned(),
                stopped_there: Some(CompletionStoppedThere::Stopped),
                stop_count: self.remote_child.stop_count,
                executed_model: None,
            };
        }

        FollowAction::RetryWithBackoff { defect_count: 0 }
    }

    /// Marks that another follow pass is needed (matching `following` map `again` flag).
    pub fn request_follow_again(&mut self) {
        self.follow_again = true;
    }

    /// Consumes a queued `follow_again` flag.
    pub fn take_follow_again(&mut self) -> bool {
        let was_queued = self.follow_again;
        self.follow_again = false;
        was_queued
    }
}
