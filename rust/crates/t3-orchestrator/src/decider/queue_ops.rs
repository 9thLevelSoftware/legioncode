//! Queue ordering, slash-command enqueue rules, and runtime model inheritance
//! (`0a159c9fba` / #39417, `c385377544` / #40316, `08804b2170` / #40373).

use serde_json::Value;
use t3_contracts::ids::{CommandId, RunId, ThreadId};
use t3_contracts::orchestration::{
    AppThread, ConversationMessage, DispatchMode, MessageDispatchCommand, ModelSelection,
    OrchestrationActor, Run, RunOutputSchema, RunStatus,
};

use crate::decider::message_ops::make_run_event;
use crate::decider::thread_ops::{is_run_active, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError, derived_effect_id};
use crate::outbox::{OutboxEffectRequest, PendingOutboxEffect};
use crate::sink::projection_apply::DeciderThreadState;

/// Returns `true` when `text` starts with a `/command` token and has no attachments (`#40316`).
///
/// Matches `/^\/[\w:-]+(?:\s|$)/u` so paths like `/home/me/index.ts` or `/ spaced` are excluded.
#[must_use]
pub fn is_slash_command(text: &str, attachments: &[Value]) -> bool {
    if !attachments.is_empty() {
        return false;
    }
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !is_slash_cmd_char(first) {
        return false;
    }
    for ch in chars {
        if ch.is_whitespace() {
            return true;
        }
        if !is_slash_cmd_char(ch) {
            return false;
        }
    }
    true
}

fn is_slash_cmd_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == ':' || ch == '-'
}

/// Returns `true` when the message is a `/compact` slash command (`#39417`, `#39455`).
#[must_use]
pub fn is_compact_command(text: &str, attachments: &[Value]) -> bool {
    if !attachments.is_empty() {
        return false;
    }
    let trimmed = text.trim();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    first.eq_ignore_ascii_case("/compact")
}

/// Returns `true` when an unheld queued run belongs to an agent message (`#39417`).
#[must_use]
pub fn has_unheld_queued_agent_message(state: &DeciderThreadState) -> bool {
    state
        .runs
        .iter()
        .filter(|run| run.status == RunStatus::Queued && run.queue_held != Some(true))
        .any(|run| {
            state
                .messages_by_id
                .get(&run.user_message_id)
                .is_some_and(|msg| msg.created_by == OrchestrationActor::Agent)
        })
}

/// Normalizes `cmd.dispatch_mode` against fresh context, typed output, slash commands, and
/// strict agent FIFO ordering behind `/compact`.
#[must_use]
pub fn effective_dispatch_mode(
    state: &DeciderThreadState,
    cmd: &MessageDispatchCommand,
) -> DispatchMode {
    let mut mode = cmd.dispatch_mode.clone();
    if (cmd.fresh_context == Some(true) || cmd.output_schema.is_some())
        && matches!(
            mode,
            DispatchMode::SteerActive { .. } | DispatchMode::RestartActive { .. }
        )
    {
        mode = DispatchMode::QueueAfterActive;
    }
    if is_slash_command(&cmd.text, &cmd.attachments)
        && matches!(mode, DispatchMode::SteerActive { .. })
    {
        mode = DispatchMode::QueueAfterActive;
    }
    if cmd.delivery_intent.as_deref() == Some("auto")
        && cmd.created_by == OrchestrationActor::Agent
        && matches!(
            mode,
            DispatchMode::SteerActive { .. } | DispatchMode::RestartActive { .. }
        )
        && has_unheld_queued_agent_message(state)
    {
        mode = DispatchMode::QueueAfterActive;
    }
    mode
}

/// Computes `followsThreadSelection` (`#40373`): queued user/agent sends or scheduled tasks with
/// `modelSelection: None` follow the thread's current model selection until executed.
#[must_use]
pub fn compute_follows_thread_selection(
    cmd: &MessageDispatchCommand,
    will_queue: bool,
) -> Option<bool> {
    let eligible = cmd.model_selection.is_none()
        && (cmd.scheduled_task_id.is_some()
            || (will_queue && cmd.linked_sender.is_none() && cmd.notification.is_none()));
    eligible.then_some(true)
}

/// Returns the model selection a queued run uses when popped for execution (`#40373`).
#[must_use]
pub fn resolve_queued_run_execution_model(
    thread: &AppThread,
    queued_run: &Run,
    queued_message: Option<&ConversationMessage>,
) -> ModelSelection {
    if queued_message.is_some_and(|m| m.follows_thread_selection == Some(true)) {
        thread.model_selection.clone()
    } else {
        queued_run.model_selection.clone()
    }
}

/// Computes the next 1-based `queuePosition` for newly queued runs on a thread.
#[must_use]
pub fn next_queue_position(runs: &[Run]) -> u64 {
    let max_pos = runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued)
        .filter_map(|r| r.queue_position.flatten())
        .max()
        .unwrap_or(0);
    max_pos + 1
}

/// Returns unheld queued runs in strict FIFO dispatch order (`(queue_position, ordinal)`).
#[must_use]
pub fn ordered_ready_queued_runs(runs: &[Run]) -> Vec<&Run> {
    let mut queued: Vec<&Run> = runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued && r.queue_held != Some(true))
        .collect();
    queued.sort_by_key(|r| (r.queue_position.flatten().unwrap_or(r.ordinal), r.ordinal));
    queued
}

/// Pops the head unheld queued run for execution once active work settles, inheriting the thread's
/// live `modelSelection` when `followsThreadSelection` is set (`#40373`, `#39417`).
pub fn decide_pop_next_queued_run(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread = require_live_thread(state, thread_id)?;
    if state
        .runs
        .iter()
        .any(|r| r.status != RunStatus::Queued && is_run_active(r))
    {
        return Err(DeciderError::Conflict(format!(
            "thread `{thread_id}` still has non-queued active work"
        )));
    }
    let Some(next) = ordered_ready_queued_runs(&state.runs).into_iter().next() else {
        return Err(DeciderError::InvalidRunState(format!(
            "thread `{thread_id}` has no ready queued runs"
        )));
    };
    let queued_msg = state.messages_by_id.get(&next.user_message_id);
    let model_selection = resolve_queued_run_execution_model(&thread, next, queued_msg);
    let mut updated = next.clone();
    updated.provider_instance_id = model_selection.instance_id.clone();
    updated.model_selection = model_selection;
    updated.status = RunStatus::Running;
    updated.queue_position = Some(None);
    updated.started_at = Some(now_iso.to_owned());

    let run_id = updated.id.clone();
    let ev = make_run_event(command_id, 0, now_iso, "run.updated", updated)?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "queued-run.pop".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![ev],
        outbox_effects: vec![PendingOutboxEffect {
            id: derived_effect_id(command_id, 0),
            command_id: command_id.clone(),
            thread_id: thread_id.clone(),
            request: OutboxEffectRequest::ProviderTurnStart { run_id },
            available_at: None,
        }],
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn build_user_message(
    cmd: &MessageDispatchCommand,
    run_id: Option<RunId>,
    will_queue: bool,
    now_iso: &str,
) -> ConversationMessage {
    ConversationMessage {
        notification: cmd.notification.clone(),
        created_by: cmd.created_by,
        creation_source: cmd.creation_source,
        scheduled_task_id: cmd.scheduled_task_id.clone(),
        follows_thread_selection: compute_follows_thread_selection(cmd, will_queue),
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

pub(crate) fn build_dispatched_run(
    run_id: &RunId,
    thread: &AppThread,
    ordinal: u64,
    status: RunStatus,
    now_iso: &str,
    cmd: &MessageDispatchCommand,
    output_schema: Option<RunOutputSchema>,
) -> Run {
    let selection: ModelSelection = cmd
        .model_selection
        .clone()
        .unwrap_or_else(|| thread.model_selection.clone());
    let _is_subagent_compact = thread.lineage.parent_thread_id.is_some()
        && is_compact_command(&cmd.text, &cmd.attachments);
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
        output_schema,
        output: None,
    }
}
