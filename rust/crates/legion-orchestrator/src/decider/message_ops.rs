//! Pure message dispatch (`message.dispatch` / logical `message.send`) transitions.

use legion_contracts::ids::{CommandId, RunId};
use legion_contracts::orchestration::{
    AppThread, DispatchMode, DomainEvent, DomainEventPayload, MessageDispatchCommand, Run,
    RunOutputSchema, RunStatus,
};

use crate::decider::output_ops::validate_output_request_for_message;
use crate::decider::queue_ops::{
    build_dispatched_run, build_user_message, effective_dispatch_mode, next_queue_position,
};
use crate::decider::thread_ops::{is_run_active, make_thread_event, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError, derived_effect_id, derived_event_id};
use crate::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use crate::sink::projection_apply::DeciderThreadState;

pub(crate) fn decide_message_dispatch(
    state: &DeciderThreadState,
    cmd: &MessageDispatchCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    if thread.archived_at.is_some() {
        return Err(DeciderError::ThreadArchived(cmd.thread_id.clone()));
    }
    let inherited_output_schema = resolve_dispatch_output_schema(state, cmd);
    validate_output_request_for_message(&cmd.text, &cmd.attachments, cmd.output_schema.as_ref())?;

    let active_runs: Vec<&Run> = state.runs.iter().filter(|r| is_run_active(r)).collect();
    let max_ordinal = state.runs.iter().map(|r| r.ordinal).max().unwrap_or(0);
    let mut events = Vec::new();
    let mut outbox = Vec::new();

    clear_settled_and_snoozed(&mut thread, cmd, now_iso, &mut events)?;
    let mode = effective_dispatch_mode(state, cmd);
    let mut acc = DispatchAcc {
        now_iso,
        events: &mut events,
        outbox: &mut outbox,
        output_schema: inherited_output_schema,
        queue_pos: next_queue_position(&state.runs),
    };
    let (msg_run_id, will_queue) =
        apply_dispatch_mode(&thread, cmd, &mode, &active_runs, max_ordinal, &mut acc)?;

    let msg = build_user_message(cmd, msg_run_id, will_queue, now_iso);
    let ev_idx = events.len();
    events.push(DomainEvent {
        id: derived_event_id(&cmd.command_id, ev_idx)?,
        thread_id: cmd.thread_id.clone(),
        run_id: msg.run_id.clone(),
        node_id: None,
        driver: None,
        provider_instance_id: Some(thread.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload: DomainEventPayload::MessageUpdated(Box::new(msg)),
    });

    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "message.dispatch".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events,
        outbox_effects: outbox,
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

fn resolve_dispatch_output_schema(
    state: &DeciderThreadState,
    cmd: &MessageDispatchCommand,
) -> Option<RunOutputSchema> {
    if let Some(schema) = &cmd.output_schema {
        return Some(schema.clone());
    }
    let continued = cmd
        .restart_continuation_of_run_id
        .as_ref()
        .or(cmd.usage_limit_continuation_of_run_id.as_ref())
        .or(cmd.manual_continuation_of_run_id.as_ref())?;
    state
        .runs
        .iter()
        .find(|r| r.id == *continued)
        .and_then(|r| r.output_schema.clone())
}

fn clear_settled_and_snoozed(
    thread: &mut AppThread,
    cmd: &MessageDispatchCommand,
    now_iso: &str,
    events: &mut Vec<DomainEvent>,
) -> Result<(), DeciderError> {
    if thread.settled_override.as_deref() == Some("settled") || thread.settled_at.is_some() {
        thread.settled_override = Some("active".to_owned());
        thread.settled_at = None;
        thread.unsettled_at = Some(Some(now_iso.to_owned()));
        thread.active_order_key = Some(Some(now_iso.to_owned()));
        now_iso.clone_into(&mut thread.updated_at);
        let idx = events.len();
        events.push(make_thread_event(
            &cmd.command_id,
            idx,
            &cmd.thread_id,
            now_iso,
            "thread.unsettled",
            thread.clone(),
        )?);
    }

    if thread
        .snoozed_until
        .as_ref()
        .and_then(|o| o.as_ref())
        .is_some()
    {
        thread.snoozed_until = Some(None);
        thread.snoozed_at = Some(None);
        thread.last_snooze_wake_at = Some(Some(now_iso.to_owned()));
        now_iso.clone_into(&mut thread.updated_at);
        let idx = events.len();
        events.push(make_thread_event(
            &cmd.command_id,
            idx,
            &cmd.thread_id,
            now_iso,
            "thread.unsnoozed",
            thread.clone(),
        )?);
    }
    Ok(())
}

struct DispatchAcc<'a> {
    now_iso: &'a str,
    events: &'a mut Vec<DomainEvent>,
    outbox: &'a mut Vec<PendingOutboxEffect>,
    output_schema: Option<RunOutputSchema>,
    queue_pos: u64,
}

fn apply_dispatch_mode(
    thread: &AppThread,
    cmd: &MessageDispatchCommand,
    mode: &DispatchMode,
    active_runs: &[&Run],
    max_ordinal: u64,
    acc: &mut DispatchAcc<'_>,
) -> Result<(Option<RunId>, bool), DeciderError> {
    match mode {
        DispatchMode::SteerActive { target_run_id } => {
            if !active_runs.iter().any(|r| r.id == *target_run_id) {
                return Err(DeciderError::InvalidRunState(format!(
                    "target run `{target_run_id}` is not active for steer"
                )));
            }
            acc.outbox.push(PendingOutboxEffect {
                id: derived_effect_id(&cmd.command_id, 0),
                command_id: cmd.command_id.clone(),
                thread_id: cmd.thread_id.clone(),
                request: OutboxEffectRequest::ProviderTurnSteer {
                    run_id: Some(target_run_id.clone()),
                    message_id: cmd.message_id.clone(),
                    model_selection: cmd.model_selection.clone(),
                },
                available_at: None,
            });
            Ok((Some(target_run_id.clone()), false))
        }
        DispatchMode::RestartActive { target_run_id } => {
            let run_id =
                dispatch_restart_active(thread, cmd, active_runs, target_run_id, max_ordinal, acc)?;
            Ok((run_id, false))
        }
        DispatchMode::QueueAfterActive
        | DispatchMode::DeferStart { .. }
        | DispatchMode::StartImmediately => {
            let queued = !active_runs.is_empty()
                || matches!(
                    mode,
                    DispatchMode::QueueAfterActive | DispatchMode::DeferStart { .. }
                );
            let status = if queued {
                RunStatus::Queued
            } else {
                RunStatus::Running
            };
            let new_run_id = derived_run_id(&cmd.command_id)?;
            let mut run = build_dispatched_run(
                &new_run_id,
                thread,
                max_ordinal + 1,
                status,
                acc.now_iso,
                cmd,
                acc.output_schema.clone(),
            );
            if queued {
                run.queue_position = Some(Some(acc.queue_pos));
            }
            let idx = acc.events.len();
            acc.events.push(make_run_event(
                &cmd.command_id,
                idx,
                acc.now_iso,
                "run.created",
                run,
            )?);
            if status == RunStatus::Running {
                acc.outbox.push(PendingOutboxEffect {
                    id: derived_effect_id(&cmd.command_id, 0),
                    command_id: cmd.command_id.clone(),
                    thread_id: cmd.thread_id.clone(),
                    request: OutboxEffectRequest::ProviderTurnStart {
                        run_id: new_run_id.clone(),
                    },
                    available_at: None,
                });
            }
            Ok((Some(new_run_id), queued))
        }
    }
}

fn dispatch_restart_active(
    thread: &AppThread,
    cmd: &MessageDispatchCommand,
    active_runs: &[&Run],
    target_run_id: &RunId,
    max_ordinal: u64,
    acc: &mut DispatchAcc<'_>,
) -> Result<Option<RunId>, DeciderError> {
    let Some(target) = active_runs.iter().find(|r| r.id == *target_run_id) else {
        return Err(DeciderError::InvalidRunState(format!(
            "target run `{target_run_id}` is not active for restart"
        )));
    };
    let mut old = (*target).clone();
    old.status = RunStatus::Interrupted;
    old.completed_at = Some(acc.now_iso.to_owned());
    let idx = acc.events.len();
    acc.events.push(make_run_event(
        &cmd.command_id,
        idx,
        acc.now_iso,
        "run.updated",
        old,
    )?);

    let new_run_id = derived_run_id(&cmd.command_id)?;
    let run = build_dispatched_run(
        &new_run_id,
        thread,
        max_ordinal + 1,
        RunStatus::Running,
        acc.now_iso,
        cmd,
        acc.output_schema.clone(),
    );
    let idx2 = acc.events.len();
    acc.events.push(make_run_event(
        &cmd.command_id,
        idx2,
        acc.now_iso,
        "run.created",
        run,
    )?);
    acc.outbox.push(PendingOutboxEffect {
        id: derived_effect_id(&cmd.command_id, 0),
        command_id: cmd.command_id.clone(),
        thread_id: cmd.thread_id.clone(),
        request: OutboxEffectRequest::ProviderTurnRestart {
            run_id: new_run_id.clone(),
            message_id: cmd.message_id.clone(),
            model_selection: cmd.model_selection.clone(),
        },
        available_at: None,
    });
    Ok(Some(new_run_id))
}

fn derived_run_id(command_id: &CommandId) -> Result<RunId, DeciderError> {
    RunId::new(format!("{}-run", command_id.as_str()))
        .map_err(|err| DeciderError::InvalidId(err.to_string()))
}

pub(crate) fn make_run_event(
    command_id: &CommandId,
    index: usize,
    occurred_at: &str,
    event_type: &str,
    run: Run,
) -> Result<DomainEvent, DeciderError> {
    Ok(DomainEvent {
        id: derived_event_id(command_id, index)?,
        thread_id: run.thread_id.clone(),
        run_id: Some(run.id.clone()),
        node_id: None,
        driver: None,
        provider_instance_id: Some(run.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: occurred_at.to_owned(),
        payload: DomainEventPayload::RunUpdated {
            event_type: event_type.to_owned(),
            run: Box::new(run),
        },
    })
}
