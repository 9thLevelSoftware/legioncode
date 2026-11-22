//! Shared domain event translation helpers for provider turn drivers (`Replay (dev)` & ACP).

use std::collections::BTreeMap;

use legion_contracts::ids::{
    CommandId, EventId, MessageId, NodeId, ProviderDriverKind, ProviderInstanceId,
    ProviderSessionId, ProviderThreadId, ProviderTurnId, RunAttemptId, RunId, ThreadId, TurnItemId,
};
use legion_contracts::orchestration::{
    DomainEvent, DomainEventPayload, Run, RunAttempt, RunStatus, TurnItemKind, TurnItemStatus,
};
use legion_orchestrator::outbox::PendingOutboxEffect;
use legion_orchestrator::sink::{CommitCommandInput, EventSink};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ServerRuntimeError;
use crate::turn_events_builders::TurnItemMeta;

/// Mutable turn accumulator translating provider stream updates into orchestration events.
#[derive(Debug, Clone)]
pub struct TurnEmitContext {
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Owning run ID.
    pub run_id: RunId,
    /// Active run attempt ID.
    pub attempt_id: Option<RunAttemptId>,
    /// Root execution node ID.
    pub root_node_id: NodeId,
    /// Provider driver kind.
    pub driver: ProviderDriverKind,
    /// Provider instance ID.
    pub provider_instance_id: ProviderInstanceId,
    /// Attached provider session ID.
    pub provider_session_id: ProviderSessionId,
    /// Provider thread ID.
    pub provider_thread_id: ProviderThreadId,
    /// Provider turn ID.
    pub provider_turn_id: ProviderTurnId,
    /// Assistant conversation message ID.
    pub assistant_message_id: MessageId,
    /// Assistant timeline item ID.
    pub assistant_item_id: TurnItemId,
    /// Reasoning timeline item ID (created lazily on first thought delta).
    pub reasoning_item_id: Option<TurnItemId>,
    /// Monotonic ordinal assigned to generated timeline items.
    pub next_item_ordinal: u64,
    /// Accumulated assistant response text.
    pub assistant_text: String,
    /// Accumulated reasoning text.
    pub reasoning_text: String,
    /// Tracked tool call item IDs & titles keyed by provider tool call ID.
    pub tool_items: BTreeMap<String, (TurnItemId, String, u64)>,
    /// Original tool input payloads keyed by provider tool call ID.
    pub tool_inputs: BTreeMap<String, Value>,
    /// Turn start timestamp.
    pub started_at: String,
    /// Plan counter for unique plan IDs.
    pub plan_counter: u64,
    /// Cumulative token usage JSON (`TurnTokenUsage`) recorded during the turn.
    pub turn_token_usage: Option<Value>,
}

impl TurnEmitContext {
    /// Construct a new [`TurnEmitContext`] for an executing run.
    pub fn new(
        run: &Run,
        driver: ProviderDriverKind,
        provider_session_id: ProviderSessionId,
        provider_thread_id: ProviderThreadId,
        now_iso: String,
        next_item_ordinal: u64,
    ) -> Result<Self, ServerRuntimeError> {
        let root_node_id = match run.root_node_id.clone() {
            Some(id) => id,
            None => NodeId::new(format!("node-{}", run.id.as_str()))
                .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?,
        };
        let provider_turn_id = ProviderTurnId::new(format!("pturn-{}", run.id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        let assistant_message_id = MessageId::new(format!("msg-assistant-{}", run.id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        let assistant_item_id = TurnItemId::new(format!("item-assistant-{}", run.id.as_str()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        Ok(Self {
            thread_id: run.thread_id.clone(),
            run_id: run.id.clone(),
            attempt_id: run.active_attempt_id.clone(),
            root_node_id,
            driver,
            provider_instance_id: run.provider_instance_id.clone(),
            provider_session_id,
            provider_thread_id,
            provider_turn_id,
            assistant_message_id,
            assistant_item_id,
            reasoning_item_id: None,
            next_item_ordinal: next_item_ordinal.max(1),
            assistant_text: String::new(),
            reasoning_text: String::new(),
            tool_items: BTreeMap::new(),
            tool_inputs: BTreeMap::new(),
            started_at: now_iso,
            plan_counter: 0,
            turn_token_usage: None,
        })
    }

    pub(crate) fn alloc_ordinal(&mut self) -> u64 {
        let ord = self.next_item_ordinal;
        self.next_item_ordinal = self.next_item_ordinal.saturating_add(1);
        ord
    }

    /// Wrap a payload into a [`DomainEvent`] attributed to this turn.
    pub fn wrap_event(
        &self,
        now_iso: &str,
        payload: DomainEventPayload,
    ) -> Result<DomainEvent, ServerRuntimeError> {
        let id = EventId::new(format!("evt-{}", Uuid::new_v4()))
            .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
        Ok(DomainEvent {
            id,
            thread_id: self.thread_id.clone(),
            run_id: Some(self.run_id.clone()),
            node_id: Some(self.root_node_id.clone()),
            driver: Some(self.driver.clone()),
            provider_instance_id: Some(self.provider_instance_id.clone()),
            raw_event_id: None,
            occurred_at: now_iso.to_owned(),
            payload,
        })
    }

    /// Build initial `running` events when a provider turn begins executing.
    pub fn build_start_events(
        &self,
        run: &Run,
        attempt: Option<&RunAttempt>,
        now_iso: &str,
    ) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
        let session =
            self.build_provider_session("running", &run.model_selection.model, now_iso, now_iso);
        let p_thread = self.build_provider_thread("active", run.ordinal, now_iso);
        let p_turn = self.build_provider_turn("running", run.ordinal, Some(now_iso), None);
        let mut updated_run = run.clone();
        updated_run.status = RunStatus::Running;
        updated_run.provider_thread_id = Some(self.provider_thread_id.clone());
        updated_run.started_at = Some(updated_run.started_at.unwrap_or_else(|| now_iso.to_owned()));
        updated_run.work_started_at = Some(now_iso.to_owned());

        let mut events = vec![
            self.wrap_event(
                now_iso,
                DomainEventPayload::ProviderSessionUpdated {
                    event_type: "provider-session.attached".to_owned(),
                    session,
                },
            )?,
            self.wrap_event(
                now_iso,
                DomainEventPayload::ProviderThreadUpdated(Box::new(p_thread)),
            )?,
            self.wrap_event(now_iso, DomainEventPayload::ProviderTurnUpdated(p_turn))?,
            self.wrap_event(
                now_iso,
                DomainEventPayload::RunUpdated {
                    event_type: "run.updated".to_owned(),
                    run: Box::new(updated_run),
                },
            )?,
        ];
        if let Some(att) = attempt {
            let mut next_att = att.clone();
            "running".clone_into(&mut next_att.status);
            next_att.started_at = Some(next_att.started_at.unwrap_or_else(|| now_iso.to_owned()));
            next_att.provider_thread_id = self.provider_thread_id.clone();
            next_att.provider_turn_id = Some(self.provider_turn_id.clone());
            events.push(self.wrap_event(
                now_iso,
                DomainEventPayload::RunAttemptUpdated {
                    event_type: "run-attempt.updated".to_owned(),
                    attempt: next_att,
                },
            )?);
        }
        let node = self.build_execution_node("running", Some(now_iso), None);
        events.push(self.wrap_event(now_iso, DomainEventPayload::NodeUpdated(node))?);
        Ok(events)
    }

    /// Build terminal events when the turn reaches completion or interruption/failure.
    pub fn build_finish_events(
        &mut self,
        run: &Run,
        attempt: Option<&RunAttempt>,
        final_status: RunStatus,
        now_iso: &str,
    ) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
        self.build_finish_events_with_error(run, attempt, final_status, None, now_iso)
    }

    /// Build terminal events, attaching an error `TurnItem` & `session.last_error` when failed.
    pub fn build_finish_events_with_error(
        &mut self,
        run: &Run,
        attempt: Option<&RunAttempt>,
        final_status: RunStatus,
        error_message: Option<&str>,
        now_iso: &str,
    ) -> Result<Vec<DomainEvent>, ServerRuntimeError> {
        let mut events = Vec::new();
        if !self.assistant_text.is_empty() {
            events.extend(self.build_assistant_message_events(false, now_iso)?);
        }
        if let Some(evt) = self.build_reasoning_finish_event(now_iso)? {
            events.push(evt);
        }
        let clean_error = if final_status == RunStatus::Failed {
            let trimmed = error_message.map(str::trim).filter(|s| !s.is_empty());
            trimmed.map(ToOwned::to_owned)
        } else {
            None
        };
        if let Some(ref msg) = clean_error {
            events.push(self.build_error_turn_item(msg, "unknown", false, now_iso)?);
        }
        self.append_terminal_lifecycle_events(
            &mut events,
            run,
            attempt,
            final_status,
            clean_error,
            now_iso,
        )?;
        Ok(events)
    }

    fn build_reasoning_finish_event(
        &mut self,
        now_iso: &str,
    ) -> Result<Option<DomainEvent>, ServerRuntimeError> {
        let Some(item_id) = self.reasoning_item_id.clone() else {
            return Ok(None);
        };
        let ord = self.alloc_ordinal();
        let item = self.make_turn_item(
            TurnItemMeta {
                id: item_id,
                ordinal: ord,
                status: TurnItemStatus::Completed,
                completed_at: Some(now_iso),
                title: None,
                now_iso,
            },
            TurnItemKind::Reasoning {
                text: self.reasoning_text.clone(),
                streaming: false,
            },
        );
        self.wrap_event(now_iso, DomainEventPayload::TurnItemUpdated(Box::new(item)))
            .map(Some)
    }

    fn append_terminal_lifecycle_events(
        &mut self,
        events: &mut Vec<DomainEvent>,
        run: &Run,
        attempt: Option<&RunAttempt>,
        final_status: RunStatus,
        clean_error: Option<String>,
        now_iso: &str,
    ) -> Result<(), ServerRuntimeError> {
        let status_str = match final_status {
            RunStatus::Interrupted => "interrupted",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Failed => "failed",
            _ => "completed",
        };
        let p_turn = self.build_provider_turn(
            status_str,
            run.ordinal,
            Some(&self.started_at),
            Some(now_iso),
        );
        let p_thread_status = if final_status == RunStatus::Failed {
            "error"
        } else {
            "idle"
        };
        let p_thread = self.build_provider_thread(p_thread_status, run.ordinal, now_iso);
        let session_status = if final_status == RunStatus::Failed {
            "error"
        } else {
            "ready"
        };
        let mut session = self.build_provider_session(
            session_status,
            &run.model_selection.model,
            &self.started_at,
            now_iso,
        );
        session.last_error = clean_error;
        let mut updated_run = run.clone();
        updated_run.status = final_status;
        updated_run.provider_thread_id = Some(self.provider_thread_id.clone());
        updated_run.started_at = Some(self.started_at.clone());
        updated_run.completed_at = Some(now_iso.to_owned());

        events.push(self.wrap_event(now_iso, DomainEventPayload::ProviderTurnUpdated(p_turn))?);
        events.push(self.wrap_event(
            now_iso,
            DomainEventPayload::ProviderThreadUpdated(Box::new(p_thread)),
        )?);
        events.push(self.wrap_event(
            now_iso,
            DomainEventPayload::ProviderSessionUpdated {
                event_type: "provider-session.updated".to_owned(),
                session,
            },
        )?);
        if let Some(att) = attempt {
            let mut next_att = att.clone();
            status_str.clone_into(&mut next_att.status);
            next_att.started_at = Some(self.started_at.clone());
            next_att.completed_at = Some(now_iso.to_owned());
            next_att.provider_thread_id = self.provider_thread_id.clone();
            next_att.provider_turn_id = Some(self.provider_turn_id.clone());
            events.push(self.wrap_event(
                now_iso,
                DomainEventPayload::RunAttemptUpdated {
                    event_type: "run-attempt.updated".to_owned(),
                    attempt: next_att,
                },
            )?);
        }
        let node = self.build_execution_node(status_str, Some(now_iso), Some(now_iso));
        events.push(self.wrap_event(now_iso, DomainEventPayload::NodeUpdated(node))?);
        events.push(self.wrap_event(
            now_iso,
            DomainEventPayload::RunUpdated {
                event_type: "run.updated".to_owned(),
                run: Box::new(updated_run),
            },
        )?);
        Ok(())
    }
}

/// Commit a batch of orchestration domain events and pending outbox effects atomically.
pub fn commit_runtime_events(
    sink: &EventSink,
    thread_id: &ThreadId,
    command_type: &str,
    now_iso: &str,
    events: Vec<DomainEvent>,
    outbox_effects: Vec<PendingOutboxEffect>,
) -> Result<(), ServerRuntimeError> {
    let command_id = CommandId::new(format!("cmd-rt-{}", Uuid::new_v4()))
        .map_err(|e| ServerRuntimeError::Persistence(e.to_string()))?;
    let input = CommitCommandInput {
        command_id,
        command_type: command_type.to_owned(),
        thread_id: thread_id.clone(),
        actor_kind: "server".to_owned(),
        causation_event_id: None,
        correlation_id: None,
        now_iso: now_iso.to_owned(),
        events,
        outbox_effects,
    };
    let _ = sink.commit_command(&input)?;
    Ok(())
}
