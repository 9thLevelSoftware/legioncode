//! Pure decider handlers for metadata, mode, visit, tag, reorder, and queue cancellation commands.

use legion_contracts::ids::{CommandId, RunId, ThreadId};
use legion_contracts::orchestration::{
    PatchValue, ProviderInteractionMode, RunStatus, ThreadMetadataUpdateCommand, ThreadVisitCommand,
};
use legion_contracts::policy::RuntimeMode;
use serde_json::json;

use crate::decider::message_ops::make_run_event;
use crate::decider::thread_ops::{make_thread_event, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError};
use crate::sink::projection_apply::DeciderThreadState;

pub(crate) fn decide_thread_metadata_update(
    state: &DeciderThreadState,
    cmd: &ThreadMetadataUpdateCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    if cmd.expected_empty == Some(true)
        && (!state.runs.is_empty() || !state.messages_by_id.is_empty())
    {
        return Err(DeciderError::Conflict(format!(
            "thread `{}` is not empty",
            cmd.thread_id
        )));
    }
    check_expected(&cmd.expected_branch, thread.branch.as_deref(), "branch")?;
    check_expected(
        &cmd.expected_worktree_path,
        thread.worktree_path.as_deref(),
        "worktreePath",
    )?;

    if let Some(title) = &cmd.title {
        title.clone_into(&mut thread.title);
        thread.title_renamed = Some(true);
    }
    apply_patch_string(&cmd.branch, &mut thread.branch);
    apply_patch_string(&cmd.worktree_path, &mut thread.worktree_path);
    match &cmd.limit_recovery {
        PatchValue::Absent => {}
        PatchValue::Null => thread.limit_recovery = Some(None),
        PatchValue::Set(upd) => {
            let prev = thread
                .limit_recovery
                .as_ref()
                .and_then(|o| o.as_ref())
                .cloned();
            let next = legion_contracts::orchestration::LimitRecovery {
                request_id: prev
                    .as_ref()
                    .and_then(|p| p.request_id.clone())
                    .or_else(|| Some(cmd.command_id.clone())),
                run_id: upd.run_id.clone(),
                reset_at: upd.reset_at.clone(),
                auto_resume: upd
                    .auto_resume
                    .unwrap_or_else(|| prev.as_ref().is_some_and(|p| p.auto_resume)),
                snooze: upd.snooze.or_else(|| prev.and_then(|p| p.snooze)),
            };
            thread.limit_recovery = Some(Some(next));
        }
    }
    match &cmd.linked_pull_request {
        PatchValue::Absent => {}
        PatchValue::Null => thread.linked_pull_request = Some(None),
        PatchValue::Set(pr) => thread.linked_pull_request = Some(Some(pr.clone())),
    }
    if let Some(regen) = cmd.regenerate_title {
        if regen {
            thread.title_regeneration = Some(Some(json!({
                "requestId": cmd.command_id.as_str(),
                "startedAt": now_iso,
            })));
        } else {
            thread.title_regeneration = Some(None);
        }
    }
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        &cmd.command_id,
        "thread.metadata.update",
        "thread.metadata-updated",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_runtime_mode_set(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    runtime_mode: RuntimeMode,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.runtime_mode = runtime_mode;
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        command_id,
        "thread.runtime-mode.set",
        "thread.runtime-mode-updated",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_interaction_mode_set(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    interaction_mode: ProviderInteractionMode,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.interaction_mode = interaction_mode;
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        command_id,
        "thread.interaction-mode.set",
        "thread.interaction-mode-updated",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_thread_visit(
    state: &DeciderThreadState,
    cmd: &ThreadVisitCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    thread.last_visited_at = Some(cmd.visited_at.clone());
    emit_thread_decision(
        &cmd.command_id,
        "thread.visit",
        "thread.visited",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_thread_mark_unread(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.last_visited_at = None;
    emit_thread_decision(
        command_id,
        "thread.mark-unread",
        "thread.marked-unread",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_auto_settle_set(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    enabled: bool,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.auto_settle_disabled_at = Some(if enabled {
        None
    } else {
        Some(now_iso.to_owned())
    });
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        command_id,
        "thread.auto-settle.set",
        "thread.auto-settle-set",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_pin_reorder(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    order_key: &str,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.pin_order_key = Some(Some(order_key.to_owned()));
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        command_id,
        "thread.pin.reorder",
        "thread.pin-reordered",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_active_reorder(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    order_key: &str,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.active_order_key = Some(Some(order_key.to_owned()));
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        command_id,
        "thread.active.reorder",
        "thread.active-reordered",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_tag_set(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    tag: &str,
    tagged: bool,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    let mut tags = thread.tags.unwrap_or_default();
    if tagged {
        if !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_owned());
        }
    } else {
        tags.retain(|existing| existing != tag);
    }
    thread.tags = Some(tags);
    now_iso.clone_into(&mut thread.updated_at);
    emit_thread_decision(
        command_id,
        "thread.tag.set",
        "thread.tags-updated",
        thread,
        now_iso,
    )
}

pub(crate) fn decide_queued_run_cancel(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    run_id: &RunId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread = require_live_thread(state, thread_id)?;
    let run =
        state.runs.iter().find(|r| &r.id == run_id).ok_or_else(|| {
            DeciderError::InvalidRunState(format!("queued run `{run_id}` not found"))
        })?;
    if run.status != RunStatus::Queued {
        return Err(DeciderError::InvalidRunState(format!(
            "run `{run_id}` is not queued"
        )));
    }
    let mut updated = run.clone();
    updated.status = RunStatus::Cancelled;
    updated.completed_at = Some(now_iso.to_owned());
    let event = make_run_event(command_id, 0, now_iso, "run.updated", updated)?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "queued-run.cancel".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

fn emit_thread_decision(
    command_id: &CommandId,
    command_type: &str,
    event_type: &str,
    thread: legion_contracts::orchestration::AppThread,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let thread_id = thread.id.clone();
    let event = make_thread_event(
        command_id,
        0,
        &thread_id,
        now_iso,
        event_type,
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: command_type.to_owned(),
        thread_id,
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

fn apply_patch_string(patch: &PatchValue<String>, dst: &mut Option<String>) {
    match patch {
        PatchValue::Absent => {}
        PatchValue::Null => *dst = None,
        PatchValue::Set(value) => *dst = Some(value.clone()),
    }
}

fn check_expected(
    expected: &PatchValue<String>,
    actual: Option<&str>,
    field: &str,
) -> Result<(), DeciderError> {
    match expected {
        PatchValue::Absent => Ok(()),
        PatchValue::Null if actual.is_none() => Ok(()),
        PatchValue::Set(exp) if actual == Some(exp.as_str()) => Ok(()),
        _ => Err(DeciderError::Conflict(format!(
            "expected `{field}` precondition did not match"
        ))),
    }
}
