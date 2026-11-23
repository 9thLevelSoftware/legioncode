//! Coarse server milestone notifications and client subscription filters (`milestones.ts`).

mod types;

use serde::{Deserialize, Serialize};

use crate::ids::{
    GuestMessageId, GuestShareId, MilestoneId, ProjectId, RunId, RuntimeRequestId, ThreadId,
};
use crate::pull_requests::ThreadPullRequestKey;

pub use types::{
    MilestoneActor, MilestoneActorType, MilestoneEventType, MilestoneGuestResolution,
    MilestoneInputKind, MilestoneInputResolution, MilestonePrWatchOutcome,
    MilestoneThreadRunOutcome, MilestoneWorkflowOutcome, MilestoneWorkflowResolution,
};

/// Workflow identity fields attached to `workflow.*` [`MilestoneEventKind`] variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneWorkflowRunFields {
    /// Workflow run identifier.
    pub workflow_run_id: String,
    /// Registered workflow name.
    pub workflow: String,
    /// Display title of the workflow run.
    pub run_title: String,
    /// Parent workflow run ID if invoked by another workflow, else `None`.
    pub parent_workflow_run_id: Option<String>,
}

/// Variant payload of a [`MilestoneEvent`], tagged by `"type"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MilestoneEventKind {
    /// `thread.run_settled`.
    #[serde(rename = "thread.run_settled", rename_all = "camelCase")]
    ThreadRunSettled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run_id: Option<String>,
        run_id: RunId,
        outcome: MilestoneThreadRunOutcome,
    },
    /// `thread.needs_input`.
    #[serde(rename = "thread.needs_input", rename_all = "camelCase")]
    ThreadNeedsInput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run_id: Option<String>,
        request_id: RuntimeRequestId,
        kind: MilestoneInputKind,
    },
    /// `thread.input_resolved`.
    #[serde(rename = "thread.input_resolved", rename_all = "camelCase")]
    ThreadInputResolved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run_id: Option<String>,
        request_id: RuntimeRequestId,
        kind: MilestoneInputKind,
        resolution: MilestoneInputResolution,
    },
    /// `thread.guest_request`.
    #[serde(rename = "thread.guest_request", rename_all = "camelCase")]
    ThreadGuestRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run_id: Option<String>,
        share_id: GuestShareId,
        guest_message_id: GuestMessageId,
    },
    /// `thread.guest_request_resolved`.
    #[serde(rename = "thread.guest_request_resolved", rename_all = "camelCase")]
    ThreadGuestRequestResolved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run_id: Option<String>,
        share_id: GuestShareId,
        guest_message_id: GuestMessageId,
        resolution: MilestoneGuestResolution,
    },
    /// `pr.watch_wake`.
    #[serde(rename = "pr.watch_wake", rename_all = "camelCase")]
    PrWatchWake {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run_id: Option<String>,
        pull_requests: Vec<ThreadPullRequestKey>,
        outcome: MilestonePrWatchOutcome,
        summary: String,
        watch_ended: bool,
    },
    /// `workflow.run_started`.
    #[serde(rename = "workflow.run_started", rename_all = "camelCase")]
    WorkflowRunStarted {
        #[serde(flatten)]
        workflow_run: MilestoneWorkflowRunFields,
    },
    /// `workflow.needs_input`.
    #[serde(rename = "workflow.needs_input", rename_all = "camelCase")]
    WorkflowNeedsInput {
        #[serde(flatten)]
        workflow_run: MilestoneWorkflowRunFields,
        step: String,
        question: String,
    },
    /// `workflow.input_resolved`.
    #[serde(rename = "workflow.input_resolved", rename_all = "camelCase")]
    WorkflowInputResolved {
        #[serde(flatten)]
        workflow_run: MilestoneWorkflowRunFields,
        step: String,
        resolution: MilestoneWorkflowResolution,
    },
    /// `workflow.run_paused`.
    #[serde(rename = "workflow.run_paused", rename_all = "camelCase")]
    WorkflowRunPaused {
        #[serde(flatten)]
        workflow_run: MilestoneWorkflowRunFields,
        reason: String,
    },
    /// `workflow.run_resumed`.
    #[serde(rename = "workflow.run_resumed", rename_all = "camelCase")]
    WorkflowRunResumed {
        #[serde(flatten)]
        workflow_run: MilestoneWorkflowRunFields,
    },
    /// `workflow.run_ended`.
    #[serde(rename = "workflow.run_ended", rename_all = "camelCase")]
    WorkflowRunEnded {
        #[serde(flatten)]
        workflow_run: MilestoneWorkflowRunFields,
        outcome: MilestoneWorkflowOutcome,
        detail: Option<String>,
    },
}

/// Coarse milestone notification event emitted by the server (`MilestoneEvent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneEvent {
    /// Stable transition identifier.
    pub id: MilestoneId,
    /// ISO-8601 occurrence timestamp.
    pub occurred_at: String,
    /// Owning project ID, or `None`.
    pub project_id: Option<ProjectId>,
    /// Owning thread ID, or `None`.
    pub thread_id: Option<ThreadId>,
    /// Parent thread ID when subagent child, or `None`.
    pub parent_thread_id: Option<ThreadId>,
    /// Causing actor metadata.
    pub actor: MilestoneActor,
    /// True for subagent threads or child workflow runs.
    pub is_subagent_child: bool,
    /// Tagged milestone discriminant and event-specific payload.
    #[serde(flatten)]
    pub kind: MilestoneEventKind,
}

impl MilestoneEvent {
    /// Returns the discriminant enum value matching `type` on the wire.
    #[must_use]
    pub const fn event_type(&self) -> MilestoneEventType {
        match &self.kind {
            MilestoneEventKind::ThreadRunSettled { .. } => MilestoneEventType::ThreadRunSettled,
            MilestoneEventKind::ThreadNeedsInput { .. } => MilestoneEventType::ThreadNeedsInput,
            MilestoneEventKind::ThreadInputResolved { .. } => {
                MilestoneEventType::ThreadInputResolved
            }
            MilestoneEventKind::ThreadGuestRequest { .. } => MilestoneEventType::ThreadGuestRequest,
            MilestoneEventKind::ThreadGuestRequestResolved { .. } => {
                MilestoneEventType::ThreadGuestRequestResolved
            }
            MilestoneEventKind::PrWatchWake { .. } => MilestoneEventType::PrWatchWake,
            MilestoneEventKind::WorkflowRunStarted { .. } => MilestoneEventType::WorkflowRunStarted,
            MilestoneEventKind::WorkflowNeedsInput { .. } => MilestoneEventType::WorkflowNeedsInput,
            MilestoneEventKind::WorkflowInputResolved { .. } => {
                MilestoneEventType::WorkflowInputResolved
            }
            MilestoneEventKind::WorkflowRunPaused { .. } => MilestoneEventType::WorkflowRunPaused,
            MilestoneEventKind::WorkflowRunResumed { .. } => MilestoneEventType::WorkflowRunResumed,
            MilestoneEventKind::WorkflowRunEnded { .. } => MilestoneEventType::WorkflowRunEnded,
        }
    }

    /// Returns the registered workflow name if this is a `workflow.*` milestone.
    #[must_use]
    pub fn workflow(&self) -> Option<&str> {
        match &self.kind {
            MilestoneEventKind::WorkflowRunStarted { workflow_run }
            | MilestoneEventKind::WorkflowNeedsInput { workflow_run, .. }
            | MilestoneEventKind::WorkflowInputResolved { workflow_run, .. }
            | MilestoneEventKind::WorkflowRunPaused { workflow_run, .. }
            | MilestoneEventKind::WorkflowRunResumed { workflow_run }
            | MilestoneEventKind::WorkflowRunEnded { workflow_run, .. } => {
                Some(workflow_run.workflow.as_str())
            }
            MilestoneEventKind::ThreadRunSettled { .. }
            | MilestoneEventKind::ThreadNeedsInput { .. }
            | MilestoneEventKind::ThreadInputResolved { .. }
            | MilestoneEventKind::ThreadGuestRequest { .. }
            | MilestoneEventKind::ThreadGuestRequestResolved { .. }
            | MilestoneEventKind::PrWatchWake { .. } => None,
        }
    }

    /// Returns the workflow run identifier attached to this milestone, if any.
    #[must_use]
    pub fn workflow_run_id(&self) -> Option<&str> {
        match &self.kind {
            MilestoneEventKind::ThreadRunSettled {
                workflow_run_id, ..
            }
            | MilestoneEventKind::ThreadNeedsInput {
                workflow_run_id, ..
            }
            | MilestoneEventKind::ThreadInputResolved {
                workflow_run_id, ..
            }
            | MilestoneEventKind::ThreadGuestRequest {
                workflow_run_id, ..
            }
            | MilestoneEventKind::ThreadGuestRequestResolved {
                workflow_run_id, ..
            }
            | MilestoneEventKind::PrWatchWake {
                workflow_run_id, ..
            } => workflow_run_id.as_deref(),
            MilestoneEventKind::WorkflowRunStarted { workflow_run }
            | MilestoneEventKind::WorkflowNeedsInput { workflow_run, .. }
            | MilestoneEventKind::WorkflowInputResolved { workflow_run, .. }
            | MilestoneEventKind::WorkflowRunPaused { workflow_run, .. }
            | MilestoneEventKind::WorkflowRunResumed { workflow_run }
            | MilestoneEventKind::WorkflowRunEnded { workflow_run, .. } => {
                Some(workflow_run.workflow_run_id.as_str())
            }
        }
    }
}

/// Client filter restricting which [`MilestoneEvent`] values match (`MilestoneFilter`).
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
    pub thread_ids: Option<Vec<ThreadId>>,
    /// Allowed workflow names (`Some` matches only workflow milestones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflows: Option<Vec<String>>,
    /// Allowed workflow run IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_ids: Option<Vec<String>>,
    /// Whether to include subagent threads and child workflow runs (defaults to `false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_subagent_children: Option<bool>,
}

/// Returns `true` when `event` satisfies every filter criterion in `filter` (`milestoneMatches`).
#[must_use]
pub fn milestone_matches(event: &MilestoneEvent, filter: &MilestoneFilter) -> bool {
    (filter.include_subagent_children == Some(true) || !event.is_subagent_child)
        && filter
            .types
            .as_ref()
            .is_none_or(|types| types.contains(&event.event_type()))
        && filter.project_ids.as_ref().is_none_or(|ids| {
            event
                .project_id
                .as_ref()
                .is_some_and(|pid| ids.contains(pid))
        })
        && filter.thread_ids.as_ref().is_none_or(|ids| {
            event
                .thread_id
                .as_ref()
                .is_some_and(|tid| ids.contains(tid))
        })
        && filter.workflows.as_ref().is_none_or(|names| {
            event
                .workflow()
                .is_some_and(|name| names.iter().any(|w| w == name))
        })
        && filter.workflow_run_ids.as_ref().is_none_or(|run_ids| {
            event
                .workflow_run_id()
                .is_some_and(|id| run_ids.iter().any(|candidate| candidate == id))
        })
}
