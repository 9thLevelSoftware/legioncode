//! [`MilestoneEvent`] tagged enum and [`milestone_matches`] predicate.

use legion_contracts::ids::{RunId, RuntimeRequestId};
use legion_contracts::pull_requests::ThreadPullRequestKey;
use serde::{Deserialize, Serialize};

use super::types::{
    MilestoneEnvelope, MilestoneEventType, MilestoneFilter, MilestoneGuestResolution, MilestoneId,
    MilestoneInputKind, MilestoneInputResolution, MilestonePrOutcome, MilestoneRunOutcome,
    MilestoneWorkflowInputResolution, MilestoneWorkflowOutcome, WorkflowRunFields,
};

/// Coarse milestone event emitted across thread, guest, pull request watch, and workflow streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MilestoneEvent {
    /// Thread run finished (`completed`, `failed`, `interrupted`, or `cancelled`).
    #[serde(rename = "thread.run_settled", rename_all = "camelCase")]
    ThreadRunSettled {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Settled run ID.
        run_id: RunId,
        /// Terminal outcome.
        outcome: MilestoneRunOutcome,
    },
    /// Thread agent is waiting on user input or approval.
    #[serde(rename = "thread.needs_input", rename_all = "camelCase")]
    ThreadNeedsInput {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Pending runtime request ID.
        request_id: RuntimeRequestId,
        /// Request category (`user_input` or `approval`).
        kind: MilestoneInputKind,
    },
    /// Thread prompt was answered, expired, or cancelled.
    #[serde(rename = "thread.input_resolved", rename_all = "camelCase")]
    ThreadInputResolved {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Resolved runtime request ID.
        request_id: RuntimeRequestId,
        /// Request category (`user_input` or `approval`).
        kind: MilestoneInputKind,
        /// Resolution status.
        resolution: MilestoneInputResolution,
    },
    /// External guest submitted a request on a shared thread.
    #[serde(rename = "thread.guest_request", rename_all = "camelCase")]
    ThreadGuestRequest {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Guest share link ID.
        share_id: String,
        /// Guest message ID.
        guest_message_id: String,
    },
    /// Guest request was sent, dismissed, or closed when the link changed.
    #[serde(rename = "thread.guest_request_resolved", rename_all = "camelCase")]
    ThreadGuestRequestResolved {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Guest share link ID.
        share_id: String,
        /// Guest message ID.
        guest_message_id: String,
        /// Resolution status.
        resolution: MilestoneGuestResolution,
    },
    /// Watched pull request woke its thread.
    #[serde(rename = "pr.watch_wake", rename_all = "camelCase")]
    PrWatchWake {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Watched pull request keys reported by the monitor.
        pull_requests: Vec<ThreadPullRequestKey>,
        /// Monitor notification outcome.
        outcome: MilestonePrOutcome,
        /// Short monitor wake summary.
        summary: String,
        /// Whether any watched pull request ended watching with this wake.
        watch_ended: bool,
    },
    /// Workflow run started.
    #[serde(rename = "workflow.run_started", rename_all = "camelCase")]
    WorkflowRunStarted {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Workflow run identity fields.
        #[serde(flatten)]
        workflow_fields: WorkflowRunFields,
    },
    /// Workflow run is asking a person a question.
    #[serde(rename = "workflow.needs_input", rename_all = "camelCase")]
    WorkflowNeedsInput {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Workflow run identity fields.
        #[serde(flatten)]
        workflow_fields: WorkflowRunFields,
        /// Step name requesting input.
        step: String,
        /// Prompt text.
        question: String,
    },
    /// Workflow question was answered, expired, or cancelled.
    #[serde(rename = "workflow.input_resolved", rename_all = "camelCase")]
    WorkflowInputResolved {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Workflow run identity fields.
        #[serde(flatten)]
        workflow_fields: WorkflowRunFields,
        /// Step name whose input resolved.
        step: String,
        /// Resolution status.
        resolution: MilestoneWorkflowInputResolution,
    },
    /// Workflow run paused.
    #[serde(rename = "workflow.run_paused", rename_all = "camelCase")]
    WorkflowRunPaused {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Workflow run identity fields.
        #[serde(flatten)]
        workflow_fields: WorkflowRunFields,
        /// Pause reason.
        reason: String,
    },
    /// Workflow run resumed.
    #[serde(rename = "workflow.run_resumed", rename_all = "camelCase")]
    WorkflowRunResumed {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Workflow run identity fields.
        #[serde(flatten)]
        workflow_fields: WorkflowRunFields,
    },
    /// Workflow run finished.
    #[serde(rename = "workflow.run_ended", rename_all = "camelCase")]
    WorkflowRunEnded {
        /// Shared envelope fields.
        #[serde(flatten)]
        envelope: MilestoneEnvelope,
        /// Workflow run identity fields.
        #[serde(flatten)]
        workflow_fields: WorkflowRunFields,
        /// Workflow end outcome.
        outcome: MilestoneWorkflowOutcome,
        /// Optional error or status detail.
        detail: Option<String>,
    },
}

impl MilestoneEvent {
    /// Returns the shared [`MilestoneEnvelope`].
    pub const fn envelope(&self) -> &MilestoneEnvelope {
        match self {
            Self::ThreadRunSettled { envelope, .. }
            | Self::ThreadNeedsInput { envelope, .. }
            | Self::ThreadInputResolved { envelope, .. }
            | Self::ThreadGuestRequest { envelope, .. }
            | Self::ThreadGuestRequestResolved { envelope, .. }
            | Self::PrWatchWake { envelope, .. }
            | Self::WorkflowRunStarted { envelope, .. }
            | Self::WorkflowNeedsInput { envelope, .. }
            | Self::WorkflowInputResolved { envelope, .. }
            | Self::WorkflowRunPaused { envelope, .. }
            | Self::WorkflowRunResumed { envelope, .. }
            | Self::WorkflowRunEnded { envelope, .. } => envelope,
        }
    }

    /// Returns the milestone's deduplication ID.
    pub fn id(&self) -> &MilestoneId {
        &self.envelope().id
    }

    /// Returns the ISO-8601 UTC timestamp string.
    pub fn occurred_at(&self) -> &str {
        &self.envelope().occurred_at
    }

    /// Returns the discriminant [`MilestoneEventType`].
    pub const fn event_type(&self) -> MilestoneEventType {
        match self {
            Self::ThreadRunSettled { .. } => MilestoneEventType::ThreadRunSettled,
            Self::ThreadNeedsInput { .. } => MilestoneEventType::ThreadNeedsInput,
            Self::ThreadInputResolved { .. } => MilestoneEventType::ThreadInputResolved,
            Self::ThreadGuestRequest { .. } => MilestoneEventType::ThreadGuestRequest,
            Self::ThreadGuestRequestResolved { .. } => {
                MilestoneEventType::ThreadGuestRequestResolved
            }
            Self::PrWatchWake { .. } => MilestoneEventType::PrWatchWake,
            Self::WorkflowRunStarted { .. } => MilestoneEventType::WorkflowRunStarted,
            Self::WorkflowNeedsInput { .. } => MilestoneEventType::WorkflowNeedsInput,
            Self::WorkflowInputResolved { .. } => MilestoneEventType::WorkflowInputResolved,
            Self::WorkflowRunPaused { .. } => MilestoneEventType::WorkflowRunPaused,
            Self::WorkflowRunResumed { .. } => MilestoneEventType::WorkflowRunResumed,
            Self::WorkflowRunEnded { .. } => MilestoneEventType::WorkflowRunEnded,
        }
    }

    /// Returns the workflow name when this is a `workflow.*` milestone.
    pub fn workflow_name(&self) -> Option<&str> {
        match self {
            Self::WorkflowRunStarted {
                workflow_fields, ..
            }
            | Self::WorkflowNeedsInput {
                workflow_fields, ..
            }
            | Self::WorkflowInputResolved {
                workflow_fields, ..
            }
            | Self::WorkflowRunPaused {
                workflow_fields, ..
            }
            | Self::WorkflowRunResumed {
                workflow_fields, ..
            }
            | Self::WorkflowRunEnded {
                workflow_fields, ..
            } => Some(&workflow_fields.workflow),
            Self::ThreadRunSettled { .. }
            | Self::ThreadNeedsInput { .. }
            | Self::ThreadInputResolved { .. }
            | Self::ThreadGuestRequest { .. }
            | Self::ThreadGuestRequestResolved { .. }
            | Self::PrWatchWake { .. } => None,
        }
    }

    /// Returns the effective `workflowRunId` from either workflow fields or envelope.
    pub fn effective_workflow_run_id(&self) -> Option<&str> {
        match self {
            Self::WorkflowRunStarted {
                workflow_fields, ..
            }
            | Self::WorkflowNeedsInput {
                workflow_fields, ..
            }
            | Self::WorkflowInputResolved {
                workflow_fields, ..
            }
            | Self::WorkflowRunPaused {
                workflow_fields, ..
            }
            | Self::WorkflowRunResumed {
                workflow_fields, ..
            }
            | Self::WorkflowRunEnded {
                workflow_fields, ..
            } => Some(&workflow_fields.workflow_run_id),
            _ => self.envelope().workflow_run_id.as_deref(),
        }
    }
}

/// Returns `true` when `event` matches every populated predicate in `filter`.
pub fn milestone_matches(event: &MilestoneEvent, filter: Option<&MilestoneFilter>) -> bool {
    let Some(filter) = filter else {
        return !event.envelope().is_subagent_child;
    };
    let envelope = event.envelope();
    if filter.include_subagent_children != Some(true) && envelope.is_subagent_child {
        return false;
    }
    if let Some(types) = &filter.types
        && !types.contains(&event.event_type())
    {
        return false;
    }
    if let Some(project_ids) = &filter.project_ids
        && !envelope
            .project_id
            .as_ref()
            .is_some_and(|project_id| project_ids.contains(project_id))
    {
        return false;
    }
    if let Some(thread_ids) = &filter.thread_ids
        && !envelope
            .thread_id
            .as_ref()
            .is_some_and(|thread_id| thread_ids.contains(thread_id))
    {
        return false;
    }
    if let Some(workflows) = &filter.workflows
        && !event
            .workflow_name()
            .is_some_and(|name| workflows.iter().any(|candidate| candidate == name))
    {
        return false;
    }
    if let Some(workflow_run_ids) = &filter.workflow_run_ids
        && !event
            .effective_workflow_run_id()
            .is_some_and(|run_id| workflow_run_ids.iter().any(|candidate| candidate == run_id))
    {
        return false;
    }
    true
}
