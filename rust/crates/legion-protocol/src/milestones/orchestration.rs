//! Turns orchestration domain events into thread, guest, and pull-request watch milestones
//! (`apps/server/src/milestones/OrchestrationMilestones.ts`).

use std::collections::HashMap;

use legion_contracts::ids::ThreadId;
use legion_contracts::orchestration::{
    AppThread, ConversationMessage, DomainEvent, DomainEventPayload, Run, RuntimeRequest,
};
use serde_json::Value;

use super::event_enum::MilestoneEvent;
use super::hub::MilestoneHub;
use super::orchestration_helpers::{
    envelope, input_kind, map_guest_resolution, map_input_resolution, map_run_outcome,
    parse_pr_key, parse_pr_outcome,
};
use super::time::parse_iso_epoch_ms;
use super::types::{
    MilestoneActor, MilestoneActorType, MilestoneGuestResolution, MilestoneId,
    MilestoneInputResolution,
};

/// Optional workflow metadata supplied when Wave 1 `AppThread` has not yet added `workflow_run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowMilestoneMeta {
    /// Workflow run identifier associated with this thread, if any.
    pub workflow_run_id: Option<String>,
}

/// Converts orchestration domain events after `since_ms` into [`MilestoneEvent`]s.
///
/// Tracks per-thread pending guest share messages so resolving or closing/replacing a share link
/// emits the matching `thread.guest_request_resolved` milestone (`sent`, `dismissed`, or `closed`).
#[derive(Debug)]
pub struct OrchestrationMilestones {
    since_ms: i64,
    guest_pending: HashMap<ThreadId, HashMap<String, String>>,
}

impl OrchestrationMilestones {
    /// Creates an [`OrchestrationMilestones`] projector filtering out records older than `since_ms`.
    pub fn new(since_ms: i64) -> Self {
        Self {
            since_ms,
            guest_pending: HashMap::new(),
        }
    }

    /// Creates an [`OrchestrationMilestones`] projector seeded from an ISO-8601 UTC timestamp.
    pub fn from_iso(since_iso: &str) -> Option<Self> {
        parse_iso_epoch_ms(since_iso).map(Self::new)
    }

    fn is_new(&self, timestamp: &str) -> bool {
        parse_iso_epoch_ms(timestamp).is_some_and(|ts| ts >= self.since_ms)
    }

    /// Handles one domain event and publishes all derived milestones into `hub`.
    pub fn handle_and_publish(
        &mut self,
        hub: &MilestoneHub,
        event: &DomainEvent,
        thread: Option<&AppThread>,
        workflow_meta: Option<&WorkflowMilestoneMeta>,
    ) {
        for milestone in self.handle_domain_event(event, thread, workflow_meta) {
            hub.publish(milestone);
        }
    }

    /// Derives zero or more [`MilestoneEvent`] values from one [`DomainEvent`].
    pub fn handle_domain_event(
        &mut self,
        event: &DomainEvent,
        thread: Option<&AppThread>,
        workflow_meta: Option<&WorkflowMilestoneMeta>,
    ) -> Vec<MilestoneEvent> {
        let workflow_run_id = workflow_meta.and_then(|meta| meta.workflow_run_id.as_deref());
        match &event.payload {
            DomainEventPayload::RunUpdated { event_type, run } if event_type == "run.updated" => {
                self.handle_run_updated(run, thread, workflow_run_id)
            }
            DomainEventPayload::RuntimeRequestUpdated(request) => self
                .handle_runtime_request_updated(
                    request,
                    &event.occurred_at,
                    thread,
                    workflow_run_id,
                ),
            DomainEventPayload::ThreadState { event_type, thread }
                if event_type == "thread.guest-share-updated" =>
            {
                self.guest_milestones(thread, &event.occurred_at, workflow_run_id)
            }
            DomainEventPayload::MessageUpdated(message) => {
                self.handle_message_updated(message, thread, workflow_run_id)
            }
            _ => Vec::new(),
        }
    }

    fn handle_run_updated(
        &self,
        run: &Run,
        thread: Option<&AppThread>,
        workflow_run_id: Option<&str>,
    ) -> Vec<MilestoneEvent> {
        let (Some(started_at), Some(completed_at)) = (&run.started_at, &run.completed_at) else {
            return Vec::new();
        };
        if started_at.is_empty() || !self.is_new(completed_at) {
            return Vec::new();
        }
        let (Some(outcome), Some(thread)) = (map_run_outcome(run.status), thread) else {
            return Vec::new();
        };
        let Ok(id) = MilestoneId::new(format!("thread.run_settled:{}", run.id.as_str())) else {
            return Vec::new();
        };
        vec![MilestoneEvent::ThreadRunSettled {
            envelope: envelope(
                id,
                thread,
                completed_at,
                MilestoneActor::new(
                    MilestoneActorType::Agent,
                    Some(run.provider_instance_id.as_str().to_owned()),
                    None,
                ),
                workflow_run_id,
            ),
            run_id: run.id.clone(),
            outcome,
        }]
    }

    fn handle_runtime_request_updated(
        &self,
        request: &RuntimeRequest,
        occurred_at: &str,
        thread: Option<&AppThread>,
        workflow_run_id: Option<&str>,
    ) -> Vec<MilestoneEvent> {
        let (Some(kind), Some(thread)) = (input_kind(&request.kind), thread) else {
            return Vec::new();
        };
        if request.status == "pending" {
            if !self.is_new(&request.created_at) {
                return Vec::new();
            }
            let Ok(id) = MilestoneId::new(format!("thread.needs_input:{}", request.id.as_str()))
            else {
                return Vec::new();
            };
            return vec![MilestoneEvent::ThreadNeedsInput {
                envelope: envelope(
                    id,
                    thread,
                    &request.created_at,
                    MilestoneActor::new(MilestoneActorType::Agent, None, None),
                    workflow_run_id,
                ),
                request_id: request.id.clone(),
                kind,
            }];
        }
        let Some(resolution) = map_input_resolution(&request.status) else {
            return Vec::new();
        };
        let at = request.resolved_at.as_deref().unwrap_or(occurred_at);
        if !self.is_new(at) {
            return Vec::new();
        }
        let Ok(id) = MilestoneId::new(format!("thread.input_resolved:{}", request.id.as_str()))
        else {
            return Vec::new();
        };
        let actor_type = if resolution == MilestoneInputResolution::Resolved {
            MilestoneActorType::User
        } else {
            MilestoneActorType::System
        };
        vec![MilestoneEvent::ThreadInputResolved {
            envelope: envelope(
                id,
                thread,
                at,
                MilestoneActor::new(actor_type, None, None),
                workflow_run_id,
            ),
            request_id: request.id.clone(),
            kind,
            resolution,
        }]
    }

    fn handle_message_updated(
        &self,
        message: &ConversationMessage,
        thread: Option<&AppThread>,
        workflow_run_id: Option<&str>,
    ) -> Vec<MilestoneEvent> {
        let Some(notification) = &message.notification else {
            return Vec::new();
        };
        if notification.source.get("kind").and_then(Value::as_str) != Some("monitor") {
            return Vec::new();
        }
        let Some(primary_pr) = notification.source.get("pullRequest") else {
            return Vec::new();
        };
        if !self.is_new(&message.created_at) {
            return Vec::new();
        }
        let Some(thread) = thread else {
            return Vec::new();
        };
        let mut stamps = vec![primary_pr];
        if let Some(others) = notification
            .source
            .get("otherPullRequests")
            .and_then(Value::as_array)
        {
            stamps.extend(others.iter());
        }
        let watch_ended = stamps
            .iter()
            .any(|stamp| stamp.get("watchEnded").and_then(Value::as_bool) == Some(true));
        let pull_requests = stamps.into_iter().filter_map(parse_pr_key).collect();
        let Ok(id) = MilestoneId::new(format!("pr.watch_wake:{}", message.id.as_str())) else {
            return Vec::new();
        };
        vec![MilestoneEvent::PrWatchWake {
            envelope: envelope(
                id,
                thread,
                &message.created_at,
                MilestoneActor::new(
                    MilestoneActorType::System,
                    None,
                    Some("pull request watch".to_owned()),
                ),
                workflow_run_id,
            ),
            pull_requests,
            outcome: parse_pr_outcome(&notification.outcome),
            summary: notification.summary.clone(),
            watch_ended,
        }]
    }

    /// Derives `thread.guest_request` and `thread.guest_request_resolved` from a thread snapshot.
    pub fn guest_milestones(
        &mut self,
        thread: &AppThread,
        occurred_at: &str,
        workflow_run_id: Option<&str>,
    ) -> Vec<MilestoneEvent> {
        let share = thread.guest_share.as_ref().and_then(Option::as_ref);
        let before = self
            .guest_pending
            .get(&thread.id)
            .cloned()
            .unwrap_or_default();
        let mut pending = HashMap::new();
        let mut out = Vec::new();
        let mut seen_message_ids = Vec::new();

        if let Some(share_obj) = share
            && let Some(share_id) = share_obj.get("id").and_then(Value::as_str)
        {
            let guest_name = share_obj
                .get("guestName")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let guest_actor = MilestoneActor::new(
                MilestoneActorType::Guest,
                Some(share_id.to_owned()),
                guest_name,
            );
            if let Some(messages) = share_obj.get("messages").and_then(Value::as_array) {
                for message in messages {
                    let Some(msg_id) = message.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    seen_message_ids.push(msg_id.to_owned());
                    let status = message.get("status").and_then(Value::as_str).unwrap_or("");
                    if status != "pending" {
                        if before.contains_key(msg_id)
                            && let Some(resolution) = map_guest_resolution(status)
                            && let Ok(id) =
                                MilestoneId::new(format!("thread.guest_request_resolved:{msg_id}"))
                        {
                            out.push(MilestoneEvent::ThreadGuestRequestResolved {
                                envelope: envelope(
                                    id,
                                    thread,
                                    occurred_at,
                                    MilestoneActor::new(MilestoneActorType::User, None, None),
                                    workflow_run_id,
                                ),
                                share_id: share_id.to_owned(),
                                guest_message_id: msg_id.to_owned(),
                                resolution,
                            });
                        }
                        continue;
                    }
                    pending.insert(msg_id.to_owned(), share_id.to_owned());
                    let submitted_at = message
                        .get("submittedAt")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !before.contains_key(msg_id)
                        && self.is_new(submitted_at)
                        && let Ok(id) = MilestoneId::new(format!("thread.guest_request:{msg_id}"))
                    {
                        out.push(MilestoneEvent::ThreadGuestRequest {
                            envelope: envelope(
                                id,
                                thread,
                                occurred_at,
                                guest_actor.clone(),
                                workflow_run_id,
                            ),
                            share_id: share_id.to_owned(),
                            guest_message_id: msg_id.to_owned(),
                        });
                    }
                }
            }
        }

        for (guest_message_id, share_id) in &before {
            if !seen_message_ids.iter().any(|seen| seen == guest_message_id)
                && let Ok(id) =
                    MilestoneId::new(format!("thread.guest_request_resolved:{guest_message_id}"))
            {
                out.push(MilestoneEvent::ThreadGuestRequestResolved {
                    envelope: envelope(
                        id,
                        thread,
                        occurred_at,
                        MilestoneActor::new(MilestoneActorType::User, None, None),
                        workflow_run_id,
                    ),
                    share_id: share_id.clone(),
                    guest_message_id: guest_message_id.clone(),
                    resolution: MilestoneGuestResolution::Closed,
                });
            }
        }

        if pending.is_empty() {
            self.guest_pending.remove(&thread.id);
        } else {
            self.guest_pending.insert(thread.id.clone(), pending);
        }
        out
    }
}
