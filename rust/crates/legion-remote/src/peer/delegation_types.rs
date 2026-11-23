//! Data model types for [`super::RemoteDelegationTracker`].

use legion_contracts::ids::{EnvironmentId, RunId, ThreadId};
use serde::{Deserialize, Serialize};

/// Remote child identity and stop-fencing state (`OrchestrationV2RemoteTaskChild`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteChildRef {
    /// Linked peer environment ID.
    pub environment_id: EnvironmentId,
    /// Remote child thread ID.
    pub thread_id: ThreadId,
    /// Display label of the peer environment.
    pub label: String,
    /// Optional thread title on the remote peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional remote run ID carrying the delegated turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Whether the remote side is confirmed stopped or ran out without owed stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_there: Option<bool>,
    /// Monotonic stop generation owed to the remote peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_count: Option<u64>,
}

/// Reason a local parent or caller aborted or stopped a remote delegated task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerCancelReason {
    /// Local parent thread was deleted while the follower was waiting (`BUG-07`).
    ParentDeleted,
    /// Local parent thread was stopped (`delegated_task.remote.stop`).
    ParentStopped,
    /// Task was explicitly cancelled via `task_cancel`.
    TaskCancelled {
        /// Optional cancellation reason text.
        reason: Option<String>,
    },
}

/// Terminal task status recorded locally on the parent thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteTaskTerminalStatus {
    /// Remote run finished successfully.
    Completed,
    /// Remote run or follow failed permanently.
    Failed,
    /// Task was cancelled.
    Cancelled,
    /// Parent was stopped/interrupted.
    Interrupted,
}

/// How the remote thread stopped (`delegated_task.remote.complete` `stoppedThere`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionStoppedThere {
    /// Remote run ended on its own.
    RanOut,
    /// A stop request from this environment reached the remote peer.
    Stopped,
}

/// Classification of errors encountered while following a remote delegated task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FollowErrorKind {
    /// Peer (or local caller lookup during wait!) returned `thread_not_found`.
    ThreadNotFound,
    /// Peer returned `run_not_found`.
    RunNotFound,
    /// Peer revoked or refused link capability (`capability_denied`).
    CapabilityDenied,
    /// Peer rejected request (`invalid_request`).
    InvalidRequest {
        /// Whether the peer link still exists locally (`RemoteDelegation.ts:703-708`).
        link_exists: bool,
    },
    /// Transient network/timeout/unreachable failure (retryable).
    Transient,
    /// Unexpected internal defect (`FOLLOW_MAX_DEFECTS` cap).
    Defect,
}

/// Stop command emitted by [`super::RemoteDelegationTracker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStopCommand {
    /// Dispatch `legion_thread_interrupt(stop: true)` to the remote peer.
    StopThere {
        /// Linked peer environment ID.
        environment_id: EnvironmentId,
        /// Remote thread ID to stop.
        thread_id: ThreadId,
        /// Idempotency key fenced by `stop_count` and `run_id`.
        idempotency_key: String,
        /// Fenced stop generation (`stop_count`), if incremented via parent stop.
        stop_count: Option<u64>,
        /// Optional cancellation reason forwarded to the peer.
        reason: Option<String>,
    },
    /// Nothing remains to stop on the remote peer.
    AlreadyStopped,
}

/// Next action decided by [`super::RemoteDelegationTracker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowAction {
    /// Issue a bounded `WAIT_SLICE_MS` wait slice (`until_delegated_work_settled: true`).
    WaitSlice {
        /// Slice timeout in milliseconds (`<= WAIT_SLICE_MS`).
        timeout_ms: u64,
    },
    /// Send a remote stop command (`legion_thread_interrupt(stop: true)`).
    SendStop(RemoteStopCommand),
    /// Complete the task locally on the parent thread.
    CompleteLocal {
        /// Terminal status.
        status: RemoteTaskTerminalStatus,
        /// Final summary or error text.
        result: String,
        /// Stop state acknowledged by this completion.
        stopped_there: Option<CompletionStoppedThere>,
        /// `stop_count` epoch acknowledged.
        stop_count: Option<u64>,
        /// Executed model note if reported.
        executed_model: Option<String>,
    },
    /// Retry after backoff (`consecutive_defects < FOLLOW_MAX_DEFECTS`).
    RetryWithBackoff {
        /// Current consecutive defect count.
        defect_count: u32,
    },
    /// Fail after hitting `FOLLOW_MAX_DEFECTS`, keeping remote stoppable via `task_cancel`.
    GiveUpWithWarning {
        /// Diagnostic message naming the remote thread and peer label.
        result: String,
    },
}
