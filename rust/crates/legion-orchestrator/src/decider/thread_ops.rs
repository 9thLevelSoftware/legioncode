//! Shared decider helpers plus thread archive, unarchive, and delete transitions.

use legion_contracts::ids::{CommandId, ThreadId};
use legion_contracts::orchestration::{AppThread, DomainEvent, DomainEventPayload, Run, RunStatus};

use crate::decider::types::{CommandDecision, DeciderError, derived_effect_id, derived_event_id};
use crate::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use crate::sink::projection_apply::DeciderThreadState;

pub(crate) fn require_live_thread(
    state: &DeciderThreadState,
    thread_id: &ThreadId,
) -> Result<AppThread, DeciderError> {
    let thread = state
        .thread
        .clone()
        .ok_or_else(|| DeciderError::ThreadNotFound(thread_id.clone()))?;
    if thread.deleted_at.is_some() {
        return Err(DeciderError::ThreadDeleted(thread_id.clone()));
    }
    Ok(thread)
}

pub(crate) fn is_run_active(run: &Run) -> bool {
    matches!(
        run.status,
        RunStatus::Preparing
            | RunStatus::Queued
            | RunStatus::Starting
            | RunStatus::Running
            | RunStatus::Waiting
    )
}

pub(crate) fn make_thread_event(
    command_id: &CommandId,
    index: usize,
    thread_id: &ThreadId,
    occurred_at: &str,
    event_type: &str,
    thread: AppThread,
) -> Result<DomainEvent, DeciderError> {
    Ok(DomainEvent {
        id: derived_event_id(command_id, index)?,
        thread_id: thread_id.clone(),
        run_id: None,
        node_id: None,
        driver: None,
        provider_instance_id: Some(thread.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: occurred_at.to_owned(),
        payload: DomainEventPayload::ThreadState {
            event_type: event_type.to_owned(),
            thread: Box::new(thread),
        },
    })
}

pub(crate) fn decide_thread_archive(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    for child in &state.child_threads {
        if child.deleted_at.is_none() && child.archived_at.is_none() {
            return Err(DeciderError::ActiveWorkConflict {
                thread_id: thread_id.clone(),
                child_thread_id: child.id.clone(),
            });
        }
    }
    if state.runs.iter().any(is_run_active) {
        return Err(DeciderError::ActiveWorkConflict {
            thread_id: thread_id.clone(),
            child_thread_id: thread_id.clone(),
        });
    }
    thread.archived_at = Some(now_iso.to_owned());
    thread.pinned_at = Some(None);
    thread.pin_order_key = Some(None);
    thread.snoozed_until = Some(None);
    thread.snoozed_at = Some(None);
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.archived",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.archive".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_unarchive(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.archived_at = None;
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.unarchived",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.unarchive".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_delete(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    only_if_unused: bool,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    if only_if_unused && !state.runs.is_empty() {
        return Err(DeciderError::Conflict(format!(
            "thread `{thread_id}` has runs and cannot be deleted with `only_if_unused`"
        )));
    }
    thread.deleted_at = Some(now_iso.to_owned());
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.deleted",
        thread.clone(),
    )?;
    let outbox_effects = if state.child_threads.is_empty() {
        Vec::new()
    } else {
        vec![PendingOutboxEffect {
            id: derived_effect_id(command_id, 0),
            command_id: command_id.clone(),
            thread_id: thread_id.clone(),
            request: OutboxEffectRequest::DelegatedTasksStop {
                reason: Some("thread.deleted".to_owned()),
            },
            available_at: None,
        }]
    };
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.delete".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects,
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}
