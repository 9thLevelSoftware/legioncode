//! Settlement polling helpers and `OrchestrationMilestones` pump for workflow agent runs.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use legion_contracts::orchestration::{AppThread, DomainEventPayload};
use legion_orchestrator::Orchestrator;
use legion_orchestrator::projections::query_wire_thread_detail;
use legion_protocol::milestones::{
    MilestoneActor, MilestoneActorType, MilestoneEnvelope, MilestoneEvent, MilestoneHub,
    MilestoneId, MilestoneWorkflowInputResolution, MilestoneWorkflowOutcome,
    OrchestrationMilestones, WorkflowMilestoneMeta, WorkflowRunFields,
};
use legion_workflows::traits::{
    AgentEnding, AgentResult, WorkflowAgentError, WorkflowMilestoneEvent, WorkflowMilestoneSink,
};
use serde_json::Value;

/// Publishes `legion-workflows` transitions into `MilestoneHub`.
#[derive(Clone)]
pub struct MilestoneHubWorkflowSink {
    hub: Arc<MilestoneHub>,
}

impl MilestoneHubWorkflowSink {
    /// Wraps an existing [`MilestoneHub`] for workflow milestone emission.
    pub fn new(hub: Arc<MilestoneHub>) -> Self {
        Self { hub }
    }
}

impl WorkflowMilestoneSink for MilestoneHubWorkflowSink {
    fn publish(&self, event: WorkflowMilestoneEvent) {
        if let Some(mapped) = convert_workflow_milestone(&event) {
            self.hub.publish(mapped);
        }
    }
}

fn convert_workflow_milestone(event: &WorkflowMilestoneEvent) -> Option<MilestoneEvent> {
    let id = MilestoneId::new(&event.id).ok()?;
    let actor = MilestoneActor::new(
        MilestoneActorType::System,
        Some(event.workflow_run_id.clone()),
        Some(event.workflow.clone()),
    );
    let envelope = MilestoneEnvelope {
        id,
        occurred_at: event.occurred_at.clone(),
        project_id: None,
        thread_id: None,
        parent_thread_id: None,
        actor,
        is_subagent_child: event.is_subagent_child,
        workflow_run_id: Some(event.workflow_run_id.clone()),
    };
    let workflow_fields = WorkflowRunFields {
        workflow_run_id: event.workflow_run_id.clone(),
        workflow: event.workflow.clone(),
        run_title: event.run_title.clone(),
        parent_workflow_run_id: event.parent_workflow_run_id.clone(),
    };
    Some(match event.event_type.as_str() {
        "workflow.run_started" => MilestoneEvent::WorkflowRunStarted {
            envelope,
            workflow_fields,
        },
        "workflow.needs_input" => MilestoneEvent::WorkflowNeedsInput {
            envelope,
            workflow_fields,
            step: event.step.clone().unwrap_or_default(),
            question: event.question.clone().unwrap_or_default(),
        },
        "workflow.input_resolved" => MilestoneEvent::WorkflowInputResolved {
            envelope,
            workflow_fields,
            step: event.step.clone().unwrap_or_default(),
            resolution: match event.resolution.as_deref() {
                Some("answered") => MilestoneWorkflowInputResolution::Answered,
                Some("expired") => MilestoneWorkflowInputResolution::Expired,
                _ => MilestoneWorkflowInputResolution::Cancelled,
            },
        },
        "workflow.run_paused" => MilestoneEvent::WorkflowRunPaused {
            envelope,
            workflow_fields,
            reason: event.reason.clone().unwrap_or_default(),
        },
        "workflow.run_resumed" => MilestoneEvent::WorkflowRunResumed {
            envelope,
            workflow_fields,
        },
        "workflow.run_ended" => MilestoneEvent::WorkflowRunEnded {
            envelope,
            workflow_fields,
            outcome: match event.outcome.as_deref() {
                Some("completed") => MilestoneWorkflowOutcome::Completed,
                Some("stopped") => MilestoneWorkflowOutcome::Stopped,
                Some("skipped") => MilestoneWorkflowOutcome::Skipped,
                _ => MilestoneWorkflowOutcome::Cancelled,
            },
            detail: event.detail.clone(),
        },
        _ => return None,
    })
}

pub(crate) fn latest_run_id(orchestrator: &Orchestrator, thread_id: &str) -> Option<String> {
    let conn = orchestrator.sink().db().lock().ok()?;
    let detail = query_wire_thread_detail(&conn, thread_id).ok()?;
    detail
        .projection
        .runs
        .last()
        .and_then(|r| r.get("id").and_then(Value::as_str))
        .map(str::to_owned)
}

pub(crate) fn read_turn_settlement(
    orchestrator: &Orchestrator,
    thread_id: &str,
    turn_id: &str,
) -> Result<Option<AgentResult>, WorkflowAgentError> {
    let conn = orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| WorkflowAgentError::new("database lock poisoned"))?;
    let detail = query_wire_thread_detail(&conn, thread_id)
        .map_err(|e| WorkflowAgentError::new(e.to_string()))?;
    let run = detail
        .projection
        .runs
        .iter()
        .rfind(|r| turn_id.is_empty() || r.get("id").and_then(Value::as_str) == Some(turn_id));
    let Some(run) = run else {
        return Ok(None);
    };
    let status = run
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let ending = match status.as_str() {
        "completed" => AgentEnding::Completed,
        "failed" | "error" => AgentEnding::Failed,
        "interrupted" | "cancelled" | "stopped" => AgentEnding::Stopped,
        _ => return Ok(None),
    };
    let reply = detail
        .projection
        .messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
        .and_then(|m| m.get("text").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let output = run.get("output").cloned();
    let detail_msg = run
        .get("errorMessage")
        .and_then(Value::as_str)
        .map_or(status, str::to_owned);
    Ok(Some(AgentResult {
        ending,
        detail: detail_msg,
        reply,
        output,
    }))
}

/// Spawns a background task feeding [`Orchestrator`] domain events through
/// [`OrchestrationMilestones`] into `hub` (`#40774`).
pub fn start_orchestration_milestone_pump(
    orchestrator: Arc<Orchestrator>,
    hub: Arc<MilestoneHub>,
) -> tokio::task::JoinHandle<()> {
    let since_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let mut sub = orchestrator.subscribe();
    let mut milestones = OrchestrationMilestones::new(since_ms);
    tokio::spawn(async move {
        while let Some(persisted) = sub.recv().await {
            if !matches!(
                persisted.event.payload,
                DomainEventPayload::RunUpdated { .. }
                    | DomainEventPayload::RuntimeRequestUpdated(_)
                    | DomainEventPayload::ThreadState { .. }
                    | DomainEventPayload::MessageUpdated(_)
            ) {
                continue;
            }
            let (thread_opt, wf_meta) =
                load_thread_for_milestone(&orchestrator, persisted.event.thread_id.as_str());
            milestones.handle_and_publish(
                &hub,
                &persisted.event,
                thread_opt.as_ref(),
                wf_meta.as_ref(),
            );
        }
    })
}

fn load_thread_for_milestone(
    orchestrator: &Orchestrator,
    thread_id: &str,
) -> (Option<AppThread>, Option<WorkflowMilestoneMeta>) {
    let Ok(conn) = orchestrator.sink().db().lock() else {
        return (None, None);
    };
    let row: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id = ?1 LIMIT 1",
            [thread_id],
            |r| r.get(0),
        )
        .ok();
    let Some(raw) = row else {
        return (None, None);
    };
    let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let wf_run_id = val
        .pointer("/workflowRun/runId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let thread = serde_json::from_value::<AppThread>(val).ok();
    let meta = wf_run_id.map(|id| WorkflowMilestoneMeta {
        workflow_run_id: Some(id),
    });
    (thread, meta)
}
