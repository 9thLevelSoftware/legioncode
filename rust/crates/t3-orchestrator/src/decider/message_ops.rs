//! Pure message dispatch (`message.dispatch` / logical `message.send`) transitions.

use t3_contracts::ids::{CommandId, RunId};
use t3_contracts::orchestration::{
    AppThread, ConversationMessage, DispatchMode, DomainEvent, DomainEventPayload,
    MessageDispatchCommand, ModelSelection, Run, RunStatus,
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

    let active_runs: Vec<&Run> = state.runs.iter().filter(|r| is_run_active(r)).collect();
    let max_ordinal = state.runs.iter().map(|r| r.ordinal).max().unwrap_or(0);
    let mut events = Vec::new();
    let mut outbox = Vec::new();

    clear_settled_and_snoozed(&mut thread, cmd, now_iso, &mut events)?;
    let msg_run_id = apply_dispatch_mode(
        &thread,
        cmd,
        &active_runs,
        max_ordinal,
        now_iso,
        &mut events,
        &mut outbox,
    )?;

    let msg = build_user_message(cmd, msg_run_id, now_iso);
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

fn clear_settled_and_snoozed(
    thread: &mut AppThread,
    cmd: &MessageDispatchCommand,
    now_iso: &str,
    events: &mut Vec<DomainEvent>,
) -> Result<(), DeciderError> {
    if thread.settled_override.as_deref() == Some("settled") || thread.settled_at.is_some() {
        thread.settled_override = Some("unsettled".to_owned());
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

fn apply_dispatch_mode(
    thread: &AppThread,
    cmd: &MessageDispatchCommand,
    active_runs: &[&Run],
    max_ordinal: u64,
    now_iso: &str,
    events: &mut Vec<DomainEvent>,
    outbox: &mut Vec<PendingOutboxEffect>,
) -> Result<Option<RunId>, DeciderError> {
    match &cmd.dispatch_mode {
        DispatchMode::SteerActive { target_run_id } => {
            if !active_runs.iter().any(|r| r.id == *target_run_id) {
                return Err(DeciderError::InvalidRunState(format!(
                    "target run `{target_run_id}` is not active for steer"
                )));
            }
            outbox.push(PendingOutboxEffect {
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
            Ok(Some(target_run_id.clone()))
        }
        DispatchMode::RestartActive { target_run_id } => {
            let mut acc = DispatchAcc {
                now_iso,
                events,
                outbox,
            };
            dispatch_restart_active(
                thread,
                cmd,
                active_runs,
                target_run_id,
                max_ordinal,
                &mut acc,
            )
        }
        DispatchMode::QueueAfterActive
        | DispatchMode::DeferStart { .. }
        | DispatchMode::StartImmediately => {
            let queued = !active_runs.is_empty()
                || matches!(
                    cmd.dispatch_mode,
                    DispatchMode::QueueAfterActive | DispatchMode::DeferStart { .. }
                );
            let status = if queued {
                RunStatus::Queued
            } else {
                RunStatus::Running
            };
            let new_run_id = derived_run_id(&cmd.command_id)?;
            let run = build_run(&new_run_id, thread, max_ordinal + 1, status, now_iso, cmd);
            let idx = events.len();
            events.push(make_run_event(
                &cmd.command_id,
                idx,
                now_iso,
                "run.created",
                run,
            )?);
            if status == RunStatus::Running {
                outbox.push(PendingOutboxEffect {
                    id: derived_effect_id(&cmd.command_id, 0),
                    command_id: cmd.command_id.clone(),
                    thread_id: cmd.thread_id.clone(),
                    request: OutboxEffectRequest::ProviderTurnStart {
                        run_id: new_run_id.clone(),
                    },
                    available_at: None,
                });
            }
            Ok(Some(new_run_id))
        }
    }
}

struct DispatchAcc<'a> {
    now_iso: &'a str,
    events: &'a mut Vec<DomainEvent>,
    outbox: &'a mut Vec<PendingOutboxEffect>,
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
    let run = build_run(
        &new_run_id,
        thread,
        max_ordinal + 1,
        RunStatus::Running,
        acc.now_iso,
        cmd,
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

fn build_user_message(
    cmd: &MessageDispatchCommand,
    run_id: Option<RunId>,
    now_iso: &str,
) -> ConversationMessage {
    ConversationMessage {
        notification: cmd.notification.clone(),
        created_by: cmd.created_by,
        creation_source: cmd.creation_source,
        scheduled_task_id: cmd.scheduled_task_id.clone(),
        follows_thread_selection: None,
        sender_thread_id: cmd.sender_thread_id.clone(),
        linked_sender: cmd.linked_sender.clone(),
        idle_compaction: cmd.idle_compaction.clone(),
        workflow_script: None,
        id: cmd.message_id.clone(),
        thread_id: cmd.thread_id.clone(),
        run_id,
        node_id: None,
        role: "user".to_owned(),
        text: cmd.text.clone(),
        context: cmd.context.clone(),
        attachments: cmd.attachments.clone(),
        streaming: false,
        created_at: now_iso.to_owned(),
        updated_at: now_iso.to_owned(),
        delegated_completion: cmd.delegated_completion.clone(),
    }
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

fn build_run(
    run_id: &RunId,
    thread: &AppThread,
    ordinal: u64,
    status: RunStatus,
    now_iso: &str,
    cmd: &MessageDispatchCommand,
) -> Run {
    let selection: ModelSelection = cmd
        .model_selection
        .clone()
        .unwrap_or_else(|| thread.model_selection.clone());
    Run {
        id: run_id.clone(),
        thread_id: thread.id.clone(),
        ordinal,
        provider_instance_id: selection.instance_id.clone(),
        model_selection: selection,
        provider_thread_id: thread.active_provider_thread_id.clone(),
        user_message_id: cmd.message_id.clone(),
        root_node_id: None,
        active_attempt_id: None,
        status,
        queue_position: None,
        queue_held: None,
        steer_when_ready_run_id: None,
        requested_at: now_iso.to_owned(),
        started_at: if status == RunStatus::Running {
            Some(now_iso.to_owned())
        } else {
            None
        },
        completed_at: None,
        checkpoint_id: None,
        context_handoff_id: None,
        restart_continuation_of_run_id: cmd.restart_continuation_of_run_id.clone(),
        manual_continuation_of_run_id: cmd.manual_continuation_of_run_id.clone(),
        usage_limit_continuation_of_run_id: cmd.usage_limit_continuation_of_run_id.clone(),
        delegated_task_id: None,
        work_started_at: None,
        restart_cancelled_background_work: None,
        source_plan_ref: None,
        delegated_completion: None,
        workspace_preparation: None,
        fresh_context: cmd.fresh_context,
    }
}
