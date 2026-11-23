//! Coarse milestone event primitive types, envelopes, and filters
//! (`packages/contracts/src/milestones.ts`).

use std::fmt;

use legion_contracts::ids::ProjectId;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

/// Validation error when constructing a [`MilestoneId`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("milestone id cannot be empty or whitespace-only")]
pub struct MilestoneIdError;

/// Stable identifier for a single milestone transition; publishing twice is deduplicated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MilestoneId(String);

impl MilestoneId {
    /// Trims and validates a non-empty [`MilestoneId`].
    pub fn new(raw: impl AsRef<str>) -> Result<Self, MilestoneIdError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(MilestoneIdError);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the underlying identifier string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MilestoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for MilestoneId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for MilestoneId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MilestoneId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(de::Error::custom)
    }
}

/// Kind of actor that triggered a milestone (`packages/contracts/src/milestones.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneActorType {
    /// Interactive user action.
    User,
    /// Autonomous coding agent run or request.
    Agent,
    /// Server / background monitor / scheduler.
    System,
    /// External share guest.
    Guest,
}

/// Actor metadata attached to a [`MilestoneEnvelope`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneActor {
    /// Actor category.
    #[serde(rename = "type")]
    pub actor_type: MilestoneActorType,
    /// Optional actor identifier (provider instance ID, share ID, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional human-readable actor label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl MilestoneActor {
    /// Builds an actor with optional identifier and label.
    pub fn new(actor_type: MilestoneActorType, id: Option<String>, label: Option<String>) -> Self {
        Self {
            actor_type,
            id,
            label,
        }
    }
}

/// Prompt category for `thread.needs_input` and `thread.input_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneInputKind {
    /// Interactive question prompt (`user_input` / `mcp-elicitation`).
    UserInput,
    /// Command, file, or permission approval prompt.
    Approval,
}

/// Terminal outcome for `thread.run_settled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneRunOutcome {
    /// Run finished successfully.
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
#[serde(rename_all = "lowercase")]
pub enum MilestoneInputResolution {
    /// Prompt was answered/approved.
    Resolved,
    /// Prompt timed out/expired.
    Expired,
    /// Prompt was cancelled.
    Cancelled,
}

/// Resolution outcome for `thread.guest_request_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneGuestResolution {
    /// Guest request was sent into the thread.
    Sent,
    /// Guest request was dismissed.
    Dismissed,
    /// Guest share link closed or rotated before resolution.
    Closed,
}

/// Notification outcome for `pr.watch_wake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestonePrOutcome {
    /// Watched check/PR completed.
    Completed,
    /// Watched check/PR failed.
    Failed,
    /// Watch was cancelled.
    Cancelled,
    /// Watched PR updated.
    Updated,
    /// Fallback outcome when wire value is unrecognized.
    Unknown,
}

/// Resolution outcome for `workflow.input_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneWorkflowInputResolution {
    /// Workflow question was answered.
    Answered,
    /// Workflow question expired.
    Expired,
    /// Workflow run ended with question still open.
    Cancelled,
}

/// Outcome for `workflow.run_ended`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneWorkflowOutcome {
    /// Workflow finished cleanly.
    Completed,
    /// Workflow was stopped.
    Stopped,
    /// Workflow was skipped.
    Skipped,
    /// Workflow was cancelled.
    Cancelled,
}

/// Common fields shared by every [`super::event_enum::MilestoneEvent`] variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneEnvelope {
    /// Stable transition identifier.
    pub id: MilestoneId,
    /// ISO-8601 UTC timestamp when the milestone occurred.
    pub occurred_at: String,
    /// Project owning the thread, or `null` for workflow-level milestones.
    pub project_id: Option<ProjectId>,
    /// Thread associated with the milestone, or `null`.
    pub thread_id: Option<legion_contracts::ids::ThreadId>,
    /// Parent thread ID when a thread belongs to a hierarchy.
    pub parent_thread_id: Option<legion_contracts::ids::ThreadId>,
    /// Actor that caused the milestone.
    pub actor: MilestoneActor,
    /// Whether the milestone belongs to a subagent child or invoked sub-workflow.
    pub is_subagent_child: bool,
    /// Workflow run ID for workflow milestones and workflow agent steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
}

/// Extra fields shared by all `workflow.*` milestone events (`#38882`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunFields {
    /// Workflow run identifier.
    pub workflow_run_id: String,
    /// Workflow template/definition name.
    pub workflow: String,
    /// Human-readable run title.
    pub run_title: String,
    /// Parent workflow run ID when invoked by another run.
    pub parent_workflow_run_id: Option<String>,
}

/// Canonical discriminant names for [`super::event_enum::MilestoneEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MilestoneEventType {
    /// `thread.run_settled`.
    #[serde(rename = "thread.run_settled")]
    ThreadRunSettled,
    /// `thread.needs_input`.
    #[serde(rename = "thread.needs_input")]
    ThreadNeedsInput,
    /// `thread.input_resolved`.
    #[serde(rename = "thread.input_resolved")]
    ThreadInputResolved,
    /// `thread.guest_request`.
    #[serde(rename = "thread.guest_request")]
    ThreadGuestRequest,
    /// `thread.guest_request_resolved`.
    #[serde(rename = "thread.guest_request_resolved")]
    ThreadGuestRequestResolved,
    /// `pr.watch_wake`.
    #[serde(rename = "pr.watch_wake")]
    PrWatchWake,
    /// `workflow.run_started`.
    #[serde(rename = "workflow.run_started")]
    WorkflowRunStarted,
    /// `workflow.needs_input`.
    #[serde(rename = "workflow.needs_input")]
    WorkflowNeedsInput,
    /// `workflow.input_resolved`.
    #[serde(rename = "workflow.input_resolved")]
    WorkflowInputResolved,
    /// `workflow.run_paused`.
    #[serde(rename = "workflow.run_paused")]
    WorkflowRunPaused,
    /// `workflow.run_resumed`.
    #[serde(rename = "workflow.run_resumed")]
    WorkflowRunResumed,
    /// `workflow.run_ended`.
    #[serde(rename = "workflow.run_ended")]
    WorkflowRunEnded,
}

impl MilestoneEventType {
    /// All 12 milestone type strings in canonical definition order.
    pub const ALL: [Self; 12] = [
        Self::ThreadRunSettled,
        Self::ThreadNeedsInput,
        Self::ThreadInputResolved,
        Self::ThreadGuestRequest,
        Self::ThreadGuestRequestResolved,
        Self::PrWatchWake,
        Self::WorkflowRunStarted,
        Self::WorkflowNeedsInput,
        Self::WorkflowInputResolved,
        Self::WorkflowRunPaused,
        Self::WorkflowRunResumed,
        Self::WorkflowRunEnded,
    ];

    /// Wire string representation (e.g., `"thread.run_settled"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThreadRunSettled => "thread.run_settled",
            Self::ThreadNeedsInput => "thread.needs_input",
            Self::ThreadInputResolved => "thread.input_resolved",
            Self::ThreadGuestRequest => "thread.guest_request",
            Self::ThreadGuestRequestResolved => "thread.guest_request_resolved",
            Self::PrWatchWake => "pr.watch_wake",
            Self::WorkflowRunStarted => "workflow.run_started",
            Self::WorkflowNeedsInput => "workflow.needs_input",
            Self::WorkflowInputResolved => "workflow.input_resolved",
            Self::WorkflowRunPaused => "workflow.run_paused",
            Self::WorkflowRunResumed => "workflow.run_resumed",
            Self::WorkflowRunEnded => "workflow.run_ended",
        }
    }

    /// Returns `true` when this type belongs to `workflow.*`.
    pub const fn is_workflow(self) -> bool {
        matches!(
            self,
            Self::WorkflowRunStarted
                | Self::WorkflowNeedsInput
                | Self::WorkflowInputResolved
                | Self::WorkflowRunPaused
                | Self::WorkflowRunResumed
                | Self::WorkflowRunEnded
        )
    }
}

/// Filter selecting matching milestones (`packages/contracts/src/milestones.ts`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneFilter {
    /// Allowed event types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<MilestoneEventType>>,
    /// Allowed project IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<ProjectId>>,
    /// Allowed thread IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ids: Option<Vec<legion_contracts::ids::ThreadId>>,
    /// Allowed workflow names; when present, matches only workflow milestones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflows: Option<Vec<String>>,
    /// Allowed workflow run IDs (matches workflow runs and threads running their agent steps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_ids: Option<Vec<String>>,
    /// When `true`, includes subagent children and invoked sub-workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_subagent_children: Option<bool>,
}
