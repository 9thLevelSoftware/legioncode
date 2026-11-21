//! Known orchestration domain event types and [`DomainEventPayload`] codec.

use serde_json::Value;

use crate::pull_requests::ThreadPullRequestLink;

use super::checkpoint_models::{
    Checkpoint, CheckpointRollbackRequest, CheckpointScope, ContextHandoff, ContextTransfer,
    PlanArtifact,
};
use super::items::TurnItem;
use super::run_models::{ExecutionNode, Run, RunAttempt, RunBackgroundWorkCancelled, Subagent};
use super::runtime_models::{
    ConversationMessage, ProviderSession, ProviderSessionDetached, ProviderThread, ProviderTurn,
    RuntimeRequest,
};
use super::state_models::{AppThread, ThreadVisit};

/// All 48 known `OrchestrationV2DomainEvent` `type` strings (`orchestrationV2.ts:2025-2175`).
pub const KNOWN_DOMAIN_EVENT_TYPES: &[&str] = &[
    "thread.created",
    "thread.archived",
    "thread.unarchived",
    "thread.deleted",
    "thread.settled",
    "thread.unsettled",
    "thread.snoozed",
    "thread.unsnoozed",
    "thread.pinned",
    "thread.auto-settle-set",
    "thread.unpinned",
    "thread.pin-reordered",
    "thread.active-reordered",
    "thread.tags-updated",
    "thread.visited",
    "thread.marked-unread",
    "thread.metadata-updated",
    "thread.pull-request-synced",
    "thread.issue-synced",
    "thread.runtime-mode-updated",
    "thread.interaction-mode-updated",
    "thread.model-selection-updated",
    "thread.provider-switched",
    "thread.guest-share-updated",
    "thread.pull-request-link-updated",
    "thread.visit-recorded",
    "run.created",
    "run.updated",
    "run.background-work-cancelled",
    "run-attempt.created",
    "run-attempt.updated",
    "node.updated",
    "subagent.updated",
    "provider-session.attached",
    "provider-session.updated",
    "provider-session.detached",
    "provider-thread.updated",
    "provider-turn.updated",
    "runtime-request.updated",
    "message.updated",
    "turn-item.updated",
    "plan.updated",
    "checkpoint-scope.created",
    "checkpoint.captured",
    "checkpoint.rollback-requested",
    "context-handoff.updated",
    "context-transfer.created",
    "context-transfer.updated",
];

/// Returns `true` if `event_type` is one of the 48 known domain event types.
pub fn is_known_domain_event_type(event_type: &str) -> bool {
    KNOWN_DOMAIN_EVENT_TYPES.contains(&event_type)
}

/// Typed payload discriminant across all 48 domain event `type` tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEventPayload {
    /// `thread.created` or one of the 23 `thread.*` state snapshot events.
    ThreadState {
        /// Exact `"type"` literal (`thread.created`, `thread.archived`, ...).
        event_type: String,
        /// Updated thread aggregate.
        thread: Box<AppThread>,
    },
    /// `thread.pull-request-link-updated`.
    ThreadPullRequestLinkUpdated(Box<ThreadPullRequestLink>),
    /// `thread.visit-recorded`.
    ThreadVisitRecorded(ThreadVisit),
    /// `run.created` or `run.updated`.
    RunUpdated {
        /// Exact `"type"` literal (`run.created` or `run.updated`).
        event_type: String,
        /// Updated run state.
        run: Box<Run>,
    },
    /// `run.background-work-cancelled`.
    RunBackgroundWorkCancelled(RunBackgroundWorkCancelled),
    /// `run-attempt.created` or `run-attempt.updated`.
    RunAttemptUpdated {
        /// Exact `"type"` literal (`run-attempt.created` or `run-attempt.updated`).
        event_type: String,
        /// Updated run attempt.
        attempt: RunAttempt,
    },
    /// `node.updated`.
    NodeUpdated(ExecutionNode),
    /// `subagent.updated`.
    SubagentUpdated(Box<Subagent>),
    /// `provider-session.attached` or `provider-session.updated`.
    ProviderSessionUpdated {
        /// Exact `"type"` literal (`provider-session.attached` or `provider-session.updated`).
        event_type: String,
        /// Provider session snapshot.
        session: ProviderSession,
    },
    /// `provider-session.detached`.
    ProviderSessionDetached(ProviderSessionDetached),
    /// `provider-thread.updated`.
    ProviderThreadUpdated(Box<ProviderThread>),
    /// `provider-turn.updated`.
    ProviderTurnUpdated(ProviderTurn),
    /// `runtime-request.updated`.
    RuntimeRequestUpdated(RuntimeRequest),
    /// `message.updated`.
    MessageUpdated(Box<ConversationMessage>),
    /// `turn-item.updated`.
    TurnItemUpdated(Box<TurnItem>),
    /// `plan.updated`.
    PlanUpdated(PlanArtifact),
    /// `checkpoint-scope.created`.
    CheckpointScopeCreated(CheckpointScope),
    /// `checkpoint.captured`.
    CheckpointCaptured(Checkpoint),
    /// `checkpoint.rollback-requested`.
    CheckpointRollbackRequested(CheckpointRollbackRequest),
    /// `context-handoff.updated`.
    ContextHandoffUpdated(Box<ContextHandoff>),
    /// `context-transfer.created` or `context-transfer.updated`.
    ContextTransferUpdated {
        /// Exact `"type"` literal (`context-transfer.created` or `context-transfer.updated`).
        event_type: String,
        /// Context transfer record.
        transfer: Box<ContextTransfer>,
    },
}

impl DomainEventPayload {
    /// Returns the wire `"type"` string for this domain event payload.
    pub fn event_type(&self) -> &str {
        match self {
            Self::ThreadState { event_type, .. }
            | Self::RunUpdated { event_type, .. }
            | Self::RunAttemptUpdated { event_type, .. }
            | Self::ProviderSessionUpdated { event_type, .. }
            | Self::ContextTransferUpdated { event_type, .. } => event_type,
            Self::ThreadPullRequestLinkUpdated(_) => "thread.pull-request-link-updated",
            Self::ThreadVisitRecorded(_) => "thread.visit-recorded",
            Self::RunBackgroundWorkCancelled(_) => "run.background-work-cancelled",
            Self::NodeUpdated(_) => "node.updated",
            Self::SubagentUpdated(_) => "subagent.updated",
            Self::ProviderSessionDetached(_) => "provider-session.detached",
            Self::ProviderThreadUpdated(_) => "provider-thread.updated",
            Self::ProviderTurnUpdated(_) => "provider-turn.updated",
            Self::RuntimeRequestUpdated(_) => "runtime-request.updated",
            Self::MessageUpdated(_) => "message.updated",
            Self::TurnItemUpdated(_) => "turn-item.updated",
            Self::PlanUpdated(_) => "plan.updated",
            Self::CheckpointScopeCreated(_) => "checkpoint-scope.created",
            Self::CheckpointCaptured(_) => "checkpoint.captured",
            Self::CheckpointRollbackRequested(_) => "checkpoint.rollback-requested",
            Self::ContextHandoffUpdated(_) => "context-handoff.updated",
        }
    }

    pub(super) fn serialize_payload_value(&self) -> Result<Value, serde_json::Error> {
        match self {
            Self::ThreadState { thread, .. } => serde_json::to_value(thread),
            Self::ThreadPullRequestLinkUpdated(link) => serde_json::to_value(link),
            Self::ThreadVisitRecorded(visit) => serde_json::to_value(visit),
            Self::RunUpdated { run, .. } => serde_json::to_value(run),
            Self::RunBackgroundWorkCancelled(cancelled) => serde_json::to_value(cancelled),
            Self::RunAttemptUpdated { attempt, .. } => serde_json::to_value(attempt),
            Self::NodeUpdated(node) => serde_json::to_value(node),
            Self::SubagentUpdated(subagent) => serde_json::to_value(subagent),
            Self::ProviderSessionUpdated { session, .. } => serde_json::to_value(session),
            Self::ProviderSessionDetached(detached) => serde_json::to_value(detached),
            Self::ProviderThreadUpdated(thread) => serde_json::to_value(thread),
            Self::ProviderTurnUpdated(turn) => serde_json::to_value(turn),
            Self::RuntimeRequestUpdated(req) => serde_json::to_value(req),
            Self::MessageUpdated(msg) => serde_json::to_value(msg),
            Self::TurnItemUpdated(item) => serde_json::to_value(item),
            Self::PlanUpdated(plan) => serde_json::to_value(plan),
            Self::CheckpointScopeCreated(scope) => serde_json::to_value(scope),
            Self::CheckpointCaptured(cp) => serde_json::to_value(cp),
            Self::CheckpointRollbackRequested(req) => serde_json::to_value(req),
            Self::ContextHandoffUpdated(handoff) => serde_json::to_value(handoff),
            Self::ContextTransferUpdated { transfer, .. } => serde_json::to_value(transfer),
        }
    }
}

fn is_thread_state_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "thread.created"
            | "thread.archived"
            | "thread.unarchived"
            | "thread.deleted"
            | "thread.settled"
            | "thread.unsettled"
            | "thread.snoozed"
            | "thread.unsnoozed"
            | "thread.pinned"
            | "thread.auto-settle-set"
            | "thread.unpinned"
            | "thread.pin-reordered"
            | "thread.active-reordered"
            | "thread.tags-updated"
            | "thread.visited"
            | "thread.marked-unread"
            | "thread.metadata-updated"
            | "thread.pull-request-synced"
            | "thread.issue-synced"
            | "thread.runtime-mode-updated"
            | "thread.interaction-mode-updated"
            | "thread.model-selection-updated"
            | "thread.provider-switched"
            | "thread.guest-share-updated"
    )
}

pub(super) fn decode_payload_for_type(
    event_type: &str,
    payload: Value,
) -> Result<DomainEventPayload, String> {
    let err_map = |err: serde_json::Error| err.to_string();
    if is_thread_state_event_type(event_type) {
        let thread = serde_json::from_value(payload).map_err(err_map)?;
        return Ok(DomainEventPayload::ThreadState {
            event_type: event_type.to_owned(),
            thread: Box::new(thread),
        });
    }
    match event_type {
        "thread.pull-request-link-updated" => Ok(DomainEventPayload::ThreadPullRequestLinkUpdated(
            Box::new(serde_json::from_value(payload).map_err(err_map)?),
        )),
        "thread.visit-recorded" => Ok(DomainEventPayload::ThreadVisitRecorded(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "run.created" | "run.updated" => Ok(DomainEventPayload::RunUpdated {
            event_type: event_type.to_owned(),
            run: Box::new(serde_json::from_value(payload).map_err(err_map)?),
        }),
        "run.background-work-cancelled" => Ok(DomainEventPayload::RunBackgroundWorkCancelled(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "run-attempt.created" | "run-attempt.updated" => {
            Ok(DomainEventPayload::RunAttemptUpdated {
                event_type: event_type.to_owned(),
                attempt: serde_json::from_value(payload).map_err(err_map)?,
            })
        }
        "node.updated" => Ok(DomainEventPayload::NodeUpdated(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "subagent.updated" => Ok(DomainEventPayload::SubagentUpdated(Box::new(
            serde_json::from_value(payload).map_err(err_map)?,
        ))),
        "provider-session.attached" | "provider-session.updated" => {
            Ok(DomainEventPayload::ProviderSessionUpdated {
                event_type: event_type.to_owned(),
                session: serde_json::from_value(payload).map_err(err_map)?,
            })
        }
        "provider-session.detached" => Ok(DomainEventPayload::ProviderSessionDetached(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "provider-thread.updated" => Ok(DomainEventPayload::ProviderThreadUpdated(Box::new(
            serde_json::from_value(payload).map_err(err_map)?,
        ))),
        "provider-turn.updated" => Ok(DomainEventPayload::ProviderTurnUpdated(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "runtime-request.updated" => Ok(DomainEventPayload::RuntimeRequestUpdated(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "message.updated" => Ok(DomainEventPayload::MessageUpdated(Box::new(
            serde_json::from_value(payload).map_err(err_map)?,
        ))),
        "turn-item.updated" => Ok(DomainEventPayload::TurnItemUpdated(Box::new(
            serde_json::from_value(payload).map_err(err_map)?,
        ))),
        "plan.updated" => Ok(DomainEventPayload::PlanUpdated(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "checkpoint-scope.created" => Ok(DomainEventPayload::CheckpointScopeCreated(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "checkpoint.captured" => Ok(DomainEventPayload::CheckpointCaptured(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "checkpoint.rollback-requested" => Ok(DomainEventPayload::CheckpointRollbackRequested(
            serde_json::from_value(payload).map_err(err_map)?,
        )),
        "context-handoff.updated" => Ok(DomainEventPayload::ContextHandoffUpdated(Box::new(
            serde_json::from_value(payload).map_err(err_map)?,
        ))),
        "context-transfer.created" | "context-transfer.updated" => {
            Ok(DomainEventPayload::ContextTransferUpdated {
                event_type: event_type.to_owned(),
                transfer: Box::new(serde_json::from_value(payload).map_err(err_map)?),
            })
        }
        other => Err(format!("unknown domain event type `{other}`")),
    }
}
