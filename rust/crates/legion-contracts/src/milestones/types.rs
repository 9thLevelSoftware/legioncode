//! Enumerated actor, outcome, resolution, and event-type discriminants for [`super::MilestoneEvent`].

use serde::{Deserialize, Serialize};

/// Actor category that caused a [`super::MilestoneEvent`] (`MilestoneActor.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneActorType {
    /// Human user.
    User,
    /// Coding agent.
    Agent,
    /// Internal server subsystem or scheduler.
    System,
    /// External guest share visitor.
    Guest,
}

/// Actor metadata attached to a [`super::MilestoneEvent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneActor {
    /// Actor category.
    #[serde(rename = "type")]
    pub actor_type: MilestoneActorType,
    /// Optional actor identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional human-readable actor label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Prompt category for `thread.needs_input` and `thread.input_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneInputKind {
    /// Structured question prompt.
    UserInput,
    /// Command, file, or permission approval prompt.
    Approval,
}

/// Terminal outcome for `thread.run_settled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneThreadRunOutcome {
    /// Run completed normally.
    Completed,
    /// Run failed with an error.
    Failed,
    /// Run was interrupted.
    Interrupted,
    /// Run was cancelled.
    Cancelled,
}

/// Resolution outcome for `thread.input_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneInputResolution {
    /// Prompt was answered by the user.
    Resolved,
    /// Prompt expired before an answer.
    Expired,
    /// Run ended or dismissed the prompt.
    Cancelled,
}

/// Resolution outcome for `thread.guest_request_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneGuestResolution {
    /// Guest request was promoted and sent into the thread.
    Sent,
    /// Guest request was dismissed.
    Dismissed,
    /// Share link closed or rotated while request was still open.
    Closed,
}

/// Notification outcome for `pr.watch_wake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestonePrWatchOutcome {
    /// Watched condition completed successfully.
    Completed,
    /// Watched check or read failed.
    Failed,
    /// Watch was cancelled.
    Cancelled,
    /// Watched PR state changed.
    Updated,
    /// Unclassified wake outcome.
    Unknown,
}

/// Resolution outcome for `workflow.input_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneWorkflowResolution {
    /// Question was answered.
    Answered,
    /// Question expired.
    Expired,
    /// Workflow ended before answering.
    Cancelled,
}

/// Terminal outcome for `workflow.run_ended`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneWorkflowOutcome {
    /// Workflow finished cleanly.
    Completed,
    /// Workflow stopped early.
    Stopped,
    /// Workflow chose not to run.
    Skipped,
    /// Workflow was cancelled.
    Cancelled,
}

/// Discriminant literals matching `MilestoneEventType` (`milestones.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MilestoneEventType {
    /// Thread run finished.
    #[serde(rename = "thread.run_settled")]
    ThreadRunSettled,
    /// Thread requires user approval or answer.
    #[serde(rename = "thread.needs_input")]
    ThreadNeedsInput,
    /// Pending thread input was resolved.
    #[serde(rename = "thread.input_resolved")]
    ThreadInputResolved,
    /// Guest visitor submitted a message request.
    #[serde(rename = "thread.guest_request")]
    ThreadGuestRequest,
    /// Pending guest message request was resolved.
    #[serde(rename = "thread.guest_request_resolved")]
    ThreadGuestRequestResolved,
    /// Watched pull request woke its thread.
    #[serde(rename = "pr.watch_wake")]
    PrWatchWake,
    /// Workflow run started.
    #[serde(rename = "workflow.run_started")]
    WorkflowRunStarted,
    /// Workflow step asked a question.
    #[serde(rename = "workflow.needs_input")]
    WorkflowNeedsInput,
    /// Open workflow question resolved.
    #[serde(rename = "workflow.input_resolved")]
    WorkflowInputResolved,
    /// Workflow run paused.
    #[serde(rename = "workflow.run_paused")]
    WorkflowRunPaused,
    /// Paused workflow run resumed.
    #[serde(rename = "workflow.run_resumed")]
    WorkflowRunResumed,
    /// Workflow run reached a terminal state.
    #[serde(rename = "workflow.run_ended")]
    WorkflowRunEnded,
}
