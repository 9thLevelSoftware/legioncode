//! SEP-3415 MCP event descriptors mapping milestones to poll and SSE stream handlers
//! (`apps/server/src/milestones/MilestoneEvents.ts`).

use legion_contracts::ids::{ProjectId, ThreadId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::event_enum::MilestoneEvent;
use super::hub::{MILESTONE_KEPT, MilestoneHub, MilestoneReadOptions};
use super::stream::{MilestoneFollowItem, MilestoneFollowStream};
use super::types::{MilestoneEventType, MilestoneFilter, MilestoneId};

/// Default event batch size returned by MCP event polls (`MilestoneEvents.ts`).
pub const MILESTONE_DEFAULT_EVENTS: usize = 100;
/// Maximum event batch size matching [`MILESTONE_KEPT`].
pub const MILESTONE_MAX_EVENTS: usize = MILESTONE_KEPT;

/// SEP-3415 event category distinguishing thread/PR descriptors from workflow descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneEventCategory {
    /// Thread, guest share, and pull-request watch milestones.
    Threads,
    /// Workflow lifecycle and question milestones.
    Workflows,
}

/// Filter parameters exposed on thread/PR milestone events (`threads` group).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneThreadFilterArgs {
    /// Narrow to milestones belonging to these projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ids: Option<Vec<ProjectId>>,
    /// Narrow to milestones belonging to these threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ids: Option<Vec<ThreadId>>,
    /// Narrow to agent step threads belonging to these workflow runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_ids: Option<Vec<String>>,
    /// Include subagent child threads and invoked runs (`false` by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_subagent_children: Option<bool>,
}

/// Filter parameters exposed on workflow milestone events (`workflows` group).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneWorkflowFilterArgs {
    /// Narrow to workflow runs matching these workflow names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflows: Option<Vec<String>>,
    /// Narrow to these workflow run IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_ids: Option<Vec<String>>,
    /// Include invoked sub-workflows (`false` by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_subagent_children: Option<bool>,
}

/// Poll request arguments passed by MCP clients (`cursor`, `maxAgeMs`, `maxEvents`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneEventPollRequest {
    /// Cursor from a previous poll or stream position, or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Skip buffered events older than `max_age_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<i64>,
    /// Maximum returned events (clamped to `MILESTONE_MAX_EVENTS`, default `100`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_events: Option<usize>,
}

/// Wire occurrence shape emitted inside SEP-3415 `poll` responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneOccurrence {
    /// Deduplication event ID matching `data.id`.
    pub event_id: MilestoneId,
    /// Occurrence ISO-8601 UTC timestamp matching `data.occurredAt`.
    pub timestamp: String,
    /// Full milestone payload.
    pub data: MilestoneEvent,
}

/// Result returned by SEP-3415 event polling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneEventPollPage {
    /// Matching event occurrences ordered oldest to newest.
    pub events: Vec<MilestoneOccurrence>,
    /// Next poll cursor.
    pub cursor: String,
    /// `true` when buffered events were lost or excluded by age.
    pub truncated: bool,
    /// `true` when additional buffered matches remain after `events`.
    pub has_more: bool,
}

/// SEP-3415 SSE event stream item emitted for `events/stream`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum McpEventStreamItem {
    /// Delivered milestone occurrence.
    #[serde(rename = "Event", rename_all = "camelCase")]
    Event {
        /// Cursor after this event.
        cursor: String,
        /// Deduplication event ID.
        event_id: MilestoneId,
        /// Occurrence timestamp.
        timestamp: String,
        /// Full milestone payload.
        data: Box<MilestoneEvent>,
    },
    /// Stream sequence moved past unmatched milestones.
    #[serde(rename = "Position", rename_all = "camelCase")]
    Position {
        /// Current cursor position.
        cursor: String,
    },
    /// Stream fell behind subscriber backlog and skipped milestones.
    #[serde(rename = "Gap", rename_all = "camelCase")]
    Gap {
        /// Boundary cursor before resumption.
        cursor: String,
    },
}

impl McpEventStreamItem {
    /// Converts a [`MilestoneFollowItem`] into an [`McpEventStreamItem`].
    pub fn from_follow_item(item: MilestoneFollowItem) -> Self {
        match item {
            MilestoneFollowItem::Milestone(positioned) => Self::Event {
                cursor: positioned.cursor,
                event_id: positioned.event.id().clone(),
                timestamp: positioned.event.occurred_at().to_owned(),
                data: Box::new(positioned.event),
            },
            MilestoneFollowItem::Position { cursor } => Self::Position { cursor },
            MilestoneFollowItem::Gap { cursor } => Self::Gap { cursor },
        }
    }
}

/// Static descriptor for one SEP-3415 MCP milestone event type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MilestoneEventDescriptor {
    /// Wire event type name (e.g., `"thread.run_settled"`).
    pub name: MilestoneEventType,
    /// Human-readable description for MCP `events/list`.
    pub description: &'static str,
    /// Parameter schema category (`threads` vs `workflows`).
    pub category: MilestoneEventCategory,
    /// Whether this event must remain hidden from in-thread agent calls (`true`).
    pub hidden_from_threads: bool,
}

impl MilestoneEventDescriptor {
    /// JSON Schema for the event type's SEP-3415 filter parameters.
    pub fn parameters_schema(&self) -> Value {
        match self.category {
            MilestoneEventCategory::Threads => json!({
                "type": "object",
                "properties": {
                    "projectIds": { "type": "array", "items": { "type": "string" } },
                    "threadIds": { "type": "array", "items": { "type": "string" } },
                    "workflowRunIds": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workflow runs, for the threads that are their agent steps."
                    },
                    "includeSubagentChildren": {
                        "type": "boolean",
                        "description": "Also subagent threads and runs another run invoked."
                    }
                }
            }),
            MilestoneEventCategory::Workflows => json!({
                "type": "object",
                "properties": {
                    "workflows": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workflow names."
                    },
                    "workflowRunIds": { "type": "array", "items": { "type": "string" } },
                    "includeSubagentChildren": {
                        "type": "boolean",
                        "description": "Also subagent threads and runs another run invoked."
                    }
                }
            }),
        }
    }
}

/// Matches `McpInvocationContext.hiddenFromThreads`: true only for outside clients with no thread scope.
pub const fn hidden_from_threads(has_thread_scope: bool) -> bool {
    !has_thread_scope
}

/// SEP-3415 milestone catalog and poll/stream dispatcher backed by [`MilestoneHub`].
#[derive(Debug, Clone)]
pub struct MilestoneEvents {
    hub: MilestoneHub,
}

impl MilestoneEvents {
    /// Wraps `hub` for outside-agent MCP polling and streaming.
    pub const fn new(hub: MilestoneHub) -> Self {
        Self { hub }
    }

    /// Returns descriptors for the 6 thread / guest / PR milestone events.
    pub fn threads(&self) -> Vec<MilestoneEventDescriptor> {
        MilestoneEventType::ALL
            .into_iter()
            .filter(|ty| !ty.is_workflow())
            .map(descriptor_for)
            .collect()
    }

    /// Returns descriptors for the 6 workflow milestone events.
    pub fn workflows(&self) -> Vec<MilestoneEventDescriptor> {
        MilestoneEventType::ALL
            .into_iter()
            .filter(|ty| ty.is_workflow())
            .map(descriptor_for)
            .collect()
    }

    /// Returns all 12 milestone descriptors in registration order.
    pub fn all(&self) -> Vec<MilestoneEventDescriptor> {
        MilestoneEventType::ALL
            .into_iter()
            .map(descriptor_for)
            .collect()
    }

    /// Polls buffered matching milestones for a thread-group event type.
    pub fn poll_thread(
        &self,
        event_type: MilestoneEventType,
        args: MilestoneThreadFilterArgs,
        request: MilestoneEventPollRequest,
    ) -> MilestoneEventPollPage {
        let filter = MilestoneFilter {
            types: Some(vec![event_type]),
            project_ids: args.project_ids,
            thread_ids: args.thread_ids,
            workflows: None,
            workflow_run_ids: args.workflow_run_ids,
            include_subagent_children: args.include_subagent_children,
        };
        self.poll_with_filter(filter, request)
    }

    /// Polls buffered matching milestones for a workflow-group event type.
    pub fn poll_workflow(
        &self,
        event_type: MilestoneEventType,
        args: MilestoneWorkflowFilterArgs,
        request: MilestoneEventPollRequest,
    ) -> MilestoneEventPollPage {
        let filter = MilestoneFilter {
            types: Some(vec![event_type]),
            project_ids: None,
            thread_ids: None,
            workflows: args.workflows,
            workflow_run_ids: args.workflow_run_ids,
            include_subagent_children: args.include_subagent_children,
        };
        self.poll_with_filter(filter, request)
    }

    /// Opens a replay + live stream for a thread-group event type.
    pub fn stream_thread(
        &self,
        event_type: MilestoneEventType,
        args: MilestoneThreadFilterArgs,
        request: &MilestoneEventPollRequest,
    ) -> MilestoneFollowStream {
        let filter = MilestoneFilter {
            types: Some(vec![event_type]),
            project_ids: args.project_ids,
            thread_ids: args.thread_ids,
            workflows: None,
            workflow_run_ids: args.workflow_run_ids,
            include_subagent_children: args.include_subagent_children,
        };
        self.hub.follow(MilestoneReadOptions {
            cursor: request.cursor.clone(),
            max_age_ms: request.max_age_ms,
            filter: Some(filter),
        })
    }

    /// Opens a replay + live stream for a workflow-group event type.
    pub fn stream_workflow(
        &self,
        event_type: MilestoneEventType,
        args: MilestoneWorkflowFilterArgs,
        request: &MilestoneEventPollRequest,
    ) -> MilestoneFollowStream {
        let filter = MilestoneFilter {
            types: Some(vec![event_type]),
            project_ids: None,
            thread_ids: None,
            workflows: args.workflows,
            workflow_run_ids: args.workflow_run_ids,
            include_subagent_children: args.include_subagent_children,
        };
        self.hub.follow(MilestoneReadOptions {
            cursor: request.cursor.clone(),
            max_age_ms: request.max_age_ms,
            filter: Some(filter),
        })
    }

    fn poll_with_filter(
        &self,
        filter: MilestoneFilter,
        request: MilestoneEventPollRequest,
    ) -> MilestoneEventPollPage {
        let limit = request
            .max_events
            .unwrap_or(MILESTONE_DEFAULT_EVENTS)
            .min(MILESTONE_MAX_EVENTS);
        let page = self.hub.read(
            &MilestoneReadOptions {
                cursor: request.cursor,
                max_age_ms: request.max_age_ms,
                filter: Some(filter),
            },
            limit,
        );
        MilestoneEventPollPage {
            events: page
                .events
                .into_iter()
                .map(|positioned| MilestoneOccurrence {
                    event_id: positioned.event.id().clone(),
                    timestamp: positioned.event.occurred_at().to_owned(),
                    data: positioned.event,
                })
                .collect(),
            cursor: page.cursor,
            truncated: page.truncated,
            has_more: page.has_more,
        }
    }
}

const fn description_of(event_type: MilestoneEventType) -> &'static str {
    match event_type {
        MilestoneEventType::ThreadRunSettled => {
            "A thread's turn ended: completed, failed, interrupted or cancelled."
        }
        MilestoneEventType::ThreadNeedsInput => {
            "An agent waits on a person to answer a question or approve an action."
        }
        MilestoneEventType::ThreadInputResolved => {
            "A question or approval an agent waited on was answered, or lapsed."
        }
        MilestoneEventType::ThreadGuestRequest => "A guest on a shared thread asked for something.",
        MilestoneEventType::ThreadGuestRequestResolved => {
            "A guest's request was sent, dismissed, or its link closed."
        }
        MilestoneEventType::PrWatchWake => "A watched pull request changed and woke its thread.",
        MilestoneEventType::WorkflowRunStarted => "A workflow run started.",
        MilestoneEventType::WorkflowNeedsInput => "A workflow run asks a person a question.",
        MilestoneEventType::WorkflowInputResolved => {
            "A workflow run's question was answered, timed out, or ended with its run."
        }
        MilestoneEventType::WorkflowRunPaused => {
            "A workflow run paused: a step failed, or someone paused it."
        }
        MilestoneEventType::WorkflowRunResumed => "A paused workflow run went on again.",
        MilestoneEventType::WorkflowRunEnded => {
            "A workflow run ended: completed, stopped, skipped or cancelled."
        }
    }
}

const fn descriptor_for(event_type: MilestoneEventType) -> MilestoneEventDescriptor {
    MilestoneEventDescriptor {
        name: event_type,
        description: description_of(event_type),
        category: if event_type.is_workflow() {
            MilestoneEventCategory::Workflows
        } else {
            MilestoneEventCategory::Threads
        },
        hidden_from_threads: true,
    }
}
