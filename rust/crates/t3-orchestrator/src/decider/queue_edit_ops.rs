//! Pure transitions for promoting queued messages and editing queued run prompts (`#40316`, `#40373`).

use t3_contracts::ids::{CommandId, RunId, ThreadId};
use t3_contracts::orchestration::{
    DomainEvent, DomainEventPayload, QueuedRunEditCommand, RunStatus,
};

use crate::decider::message_ops::make_run_event;
use crate::decider::output_ops::validate_output_request_for_message;
use crate::decider::queue_ops::{
    is_slash_command, ordered_ready_queued_runs, resolve_queued_run_execution_model,
};
use crate::decider::thread_ops::{is_run_active, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError, derived_effect_id, derived_event_id};
use crate::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use crate::sink::projection_apply::DeciderThreadState;

/// Promotes a queued run to steer `target_run_id`, or moves it to queue position 1 if it is a
/// slash command (`#40316`) while applying `followsThreadSelection` inheritance (`#40373`).
pub(crate) fn decide_queued_message_promote_to_steer(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    queued_run_id: &RunId,
    target_run_id: &RunId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread = require_live_thread(state, thread_id)?;
    let Some(queued_run) = state
        .runs
        .iter()
        .find(|r| r.id == *queued_run_id && r.status == RunStatus::Queued)
    else {
        return Err(DeciderError::InvalidRunState(format!(
            "queued run `{queued_run_id}` not found or not queued"
        )));
    };
    if !state
        .runs
        .iter()
        .any(|r| r.id == *target_run_id && is_run_active(r))
    {
        return Err(DeciderError::InvalidRunState(format!(
            "target run `{target_run_id}` is not active"
        )));
    }
    let queued_msg = state.messages_by_id.get(&queued_run.user_message_id);
    let slash_cmd = queued_msg.is_some_and(|m| is_slash_command(&m.text, &m.attachments));
    let model_selection = resolve_queued_run_execution_model(&thread, queued_run, queued_msg);

    if slash_cmd {
        let mut events = Vec::new();
        let mut promoted = queued_run.clone();
        promoted.queue_position = Some(Some(1));
        promoted.steer_when_ready_run_id = None;
        promoted.provider_instance_id = model_selection.instance_id.clone();
        promoted.model_selection = model_selection;
        events.push(make_run_event(
            command_id,
            0,
            now_iso,
            "run.updated",
            promoted,
        )?);
        let mut next_pos = 2_u64;
        for other in ordered_ready_queued_runs(&state.runs) {
            if other.id == *queued_run_id {
                continue;
            }
            let mut shifted = other.clone();
            shifted.queue_position = Some(Some(next_pos));
            next_pos += 1;
            let idx = events.len();
            events.push(make_run_event(
                command_id,
                idx,
                now_iso,
                "run.updated",
                shifted,
            )?);
        }
        return Ok(CommandDecision {
            command_id: command_id.clone(),
            command_type: "queued-message.promote-to-steer".to_owned(),
            thread_id: thread_id.clone(),
            events,
            outbox_effects: Vec::new(),
            rollback_intent: None,
            resulting_thread: Some(thread),
        });
    }

    if queued_run.fresh_context == Some(true) {
        return Err(DeciderError::InvalidRunState(
            "A message that starts a fresh session can't steer the current turn.".to_owned(),
        ));
    }
    if queued_run.output_schema.is_some() {
        return Err(DeciderError::InvalidRunState(
            "A message that asks for a result can't steer the current turn.".to_owned(),
        ));
    }

    let mut updated = queued_run.clone();
    updated.steer_when_ready_run_id = Some(target_run_id.clone());
    updated.provider_instance_id = model_selection.instance_id.clone();
    updated.model_selection = model_selection.clone();
    let event = make_run_event(command_id, 0, now_iso, "run.updated", updated)?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "queued-message.promote-to-steer".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: vec![PendingOutboxEffect {
            id: derived_effect_id(command_id, 0),
            command_id: command_id.clone(),
            thread_id: thread_id.clone(),
            request: OutboxEffectRequest::ProviderTurnSteer {
                run_id: Some(target_run_id.clone()),
                message_id: queued_run.user_message_id.clone(),
                model_selection: Some(model_selection),
            },
            available_at: None,
        }],
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

/// Edits a queued run's prompt/attachments and clears `steerWhenReadyRunId` if the edited
/// message became a slash command (`#40316`), while checking `outputSchema` compatibility.
pub(crate) fn decide_queued_run_edit(
    state: &DeciderThreadState,
    cmd: &QueuedRunEditCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread = require_live_thread(state, &cmd.thread_id)?;
    let Some(queued_run) = state
        .runs
        .iter()
        .find(|r| r.id == cmd.run_id && r.status == RunStatus::Queued)
    else {
        return Err(DeciderError::InvalidRunState(format!(
            "queued run `{}` not found or not queued",
            cmd.run_id
        )));
    };
    let Some(existing_msg) = state.messages_by_id.get(&queued_run.user_message_id) else {
        return Err(DeciderError::InvalidRunState(format!(
            "queued message `{}` not found",
            queued_run.user_message_id
        )));
    };
    let next_attachments = cmd
        .attachments
        .clone()
        .unwrap_or_else(|| existing_msg.attachments.clone());
    validate_output_request_for_message(
        &cmd.text,
        &next_attachments,
        queued_run.output_schema.as_ref(),
    )?;

    let mut updated_msg = existing_msg.clone();
    cmd.text.clone_into(&mut updated_msg.text);
    if let Some(ctx) = &cmd.context {
        updated_msg.context = Some(ctx.clone());
    }
    updated_msg.attachments = next_attachments;
    now_iso.clone_into(&mut updated_msg.updated_at);

    let mut events = vec![DomainEvent {
        id: derived_event_id(&cmd.command_id, 0)?,
        thread_id: cmd.thread_id.clone(),
        run_id: Some(queued_run.id.clone()),
        node_id: None,
        driver: None,
        provider_instance_id: Some(queued_run.provider_instance_id.clone()),
        raw_event_id: None,
        occurred_at: now_iso.to_owned(),
        payload: DomainEventPayload::MessageUpdated(Box::new(updated_msg.clone())),
    }];

    if is_slash_command(&updated_msg.text, &updated_msg.attachments)
        && queued_run.steer_when_ready_run_id.is_some()
    {
        let mut updated_run = queued_run.clone();
        updated_run.steer_when_ready_run_id = None;
        events.push(make_run_event(
            &cmd.command_id,
            1,
            now_iso,
            "run.updated",
            updated_run,
        )?);
    }

    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "queued-run.edit".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events,
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}
