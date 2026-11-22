//! Pure run interrupt, thread stop, and checkpoint rollback (`BUG-02`) transitions.

use legion_contracts::orchestration::{
    CheckpointRollbackCommand, CheckpointRollbackRequest, DomainEvent, DomainEventPayload,
    RunInterruptCommand, RunStatus, ThreadStopCommand,
};

use crate::decider::message_ops::make_run_event;
use crate::decider::thread_ops::{is_run_active, make_thread_event, require_live_thread};
use crate::decider::types::{
    CommandDecision, DeciderError, RollbackIntent, derived_effect_id, derived_event_id,
};
use crate::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use crate::sink::projection_apply::DeciderThreadState;

pub(crate) fn decide_run_interrupt(
    state: &DeciderThreadState,
    cmd: &RunInterruptCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread = require_live_thread(state, &cmd.thread_id)?;
    let target = state
        .runs
        .iter()
        .find(|r| r.id == cmd.run_id)
        .ok_or_else(|| DeciderError::InvalidRunState(format!("run `{}` not found", cmd.run_id)))?;

    if !is_run_active(target) {
        return Err(DeciderError::InvalidRunState(format!(
            "run `{}` is already terminal",
            target.id
        )));
    }
    let was_queued = target.status == RunStatus::Queued;
    let mut updated = target.clone();
    updated.status = if was_queued {
        RunStatus::Cancelled
    } else {
        RunStatus::Interrupted
    };
    updated.completed_at = Some(now_iso.to_owned());
    let event = make_run_event(&cmd.command_id, 0, now_iso, "run.updated", updated.clone())?;
    let outbox = if was_queued {
        Vec::new()
    } else {
        vec![PendingOutboxEffect {
            id: derived_effect_id(&cmd.command_id, 0),
            command_id: cmd.command_id.clone(),
            thread_id: cmd.thread_id.clone(),
            request: OutboxEffectRequest::ProviderTurnInterrupt {
                run_id: Some(updated.id),
                provider_session_id: None,
                provider_thread_id: updated.provider_thread_id,
                provider_turn_id: None,
            },
            available_at: None,
        }]
    };
    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "run.interrupt".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events: vec![event],
        outbox_effects: outbox,
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_stop(
    state: &DeciderThreadState,
    cmd: &ThreadStopCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    let mut events = Vec::new();
    let mut outbox = Vec::new();
    for (idx, run) in state.runs.iter().filter(|r| is_run_active(r)).enumerate() {
        let was_queued = run.status == RunStatus::Queued;
        let mut stopped = run.clone();
        stopped.status = if was_queued {
            RunStatus::Cancelled
        } else {
            RunStatus::Interrupted
        };
        stopped.completed_at = Some(now_iso.to_owned());
        events.push(make_run_event(
            &cmd.command_id,
            idx,
            now_iso,
            "run.updated",
            stopped.clone(),
        )?);
        if !was_queued {
            outbox.push(PendingOutboxEffect {
                id: derived_effect_id(&cmd.command_id, outbox.len()),
                command_id: cmd.command_id.clone(),
                thread_id: cmd.thread_id.clone(),
                request: OutboxEffectRequest::ProviderTurnInterrupt {
                    run_id: Some(stopped.id),
                    provider_session_id: None,
                    provider_thread_id: stopped.provider_thread_id,
                    provider_turn_id: None,
                },
                available_at: None,
            });
        }
    }
    now_iso.clone_into(&mut thread.updated_at);
    events.push(make_thread_event(
        &cmd.command_id,
        events.len(),
        &cmd.thread_id,
        now_iso,
        "thread.metadata-updated",
        thread.clone(),
    )?);
    if cmd.stop_delegated_tasks.unwrap_or(true) {
        outbox.push(PendingOutboxEffect {
            id: derived_effect_id(&cmd.command_id, outbox.len()),
            command_id: cmd.command_id.clone(),
            thread_id: cmd.thread_id.clone(),
            request: OutboxEffectRequest::DelegatedTasksStop {
                reason: cmd.reason.clone(),
            },
            available_at: None,
        });
    }
    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "thread.stop".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events,
        outbox_effects: outbox,
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_checkpoint_rollback(
    state: &DeciderThreadState,
    cmd: &CheckpointRollbackCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    if state.runs.iter().any(is_run_active) {
        return Err(DeciderError::Conflict(
            "cannot roll back checkpoints while a run is active".to_owned(),
        ));
    }
    if thread.rollback_request_id.is_some() {
        return Err(DeciderError::Conflict(
            "a checkpoint rollback is already pending for this thread".to_owned(),
        ));
    }
    if !state.known_checkpoints.contains(&cmd.checkpoint_id) {
        return Err(DeciderError::CheckpointNotFound {
            thread_id: cmd.thread_id.clone(),
            checkpoint_id: cmd.checkpoint_id.clone(),
        });
    }

    let restore_files = cmd.restore_files.unwrap_or(true);
    let intent = RollbackIntent::pending(
        cmd.command_id.clone(),
        cmd.thread_id.clone(),
        cmd.scope_id.clone(),
        cmd.checkpoint_id.clone(),
        restore_files,
        now_iso,
    );
    thread.rollback_request_id = Some(cmd.command_id.clone());
    thread.rollback_failure = Some(None);
    now_iso.clone_into(&mut thread.updated_at);

    let ev_thread = make_thread_event(
        &cmd.command_id,
        0,
        &cmd.thread_id,
        now_iso,
        "thread.metadata-updated",
        thread.clone(),
    )?;
    let ev_rollback = DomainEvent {
        id: derived_event_id(&cmd.command_id, 1)?,
        thread_id: cmd.thread_id.clone(),
        run_id: None,
        node_id: None,
        driver: None,
        provider_instance_id: Some(thread.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload: DomainEventPayload::CheckpointRollbackRequested(CheckpointRollbackRequest {
            scope_id: cmd.scope_id.clone(),
            checkpoint_id: cmd.checkpoint_id.clone(),
            requested_at: now_iso.to_owned(),
        }),
    };
    let outbox_effect = PendingOutboxEffect {
        id: derived_effect_id(&cmd.command_id, 0),
        command_id: cmd.command_id.clone(),
        thread_id: cmd.thread_id.clone(),
        request: OutboxEffectRequest::rollback(
            cmd.command_id.clone(),
            thread.active_provider_thread_id.clone(),
            cmd.checkpoint_id.clone(),
            cmd.scope_id.clone(),
            restore_files,
        ),
        available_at: None,
    };

    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "checkpoint.rollback".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events: vec![ev_thread, ev_rollback],
        outbox_effects: vec![outbox_effect],
        rollback_intent: Some(intent),
        resulting_thread: Some(thread),
    })
}
