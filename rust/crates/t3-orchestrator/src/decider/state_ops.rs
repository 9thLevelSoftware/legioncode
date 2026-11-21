//! Pure thread settle/unsettle, snooze/unsnooze, and pin/unpin command transitions.

use t3_contracts::ids::{CommandId, RunId, ThreadId};
use t3_contracts::orchestration::ThreadUnsettleCommand;

use crate::decider::thread_ops::{is_run_active, make_thread_event, require_live_thread};
use crate::decider::types::{CommandDecision, DeciderError};
use crate::sink::projection_apply::DeciderThreadState;
use crate::time::parse_iso_millis;

pub(crate) fn decide_thread_settle(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    expected_latest_run_id: Option<&RunId>,
    settled_at: Option<&str>,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    if let Some(expected) = expected_latest_run_id {
        let actual = state.runs.last().map(|r| &r.id);
        if actual != Some(expected) {
            return Err(DeciderError::Conflict(format!(
                "expected latest run `{expected}` does not match `{actual:?}`"
            )));
        }
    }
    if state.runs.iter().any(is_run_active) {
        return Err(DeciderError::Conflict(format!(
            "cannot settle thread `{thread_id}` while a run is active"
        )));
    }
    thread.settled_override = Some("settled".to_owned());
    thread.settled_at = Some(settled_at.unwrap_or(now_iso).to_owned());
    thread.unsettled_at = Some(None);
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.settled",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.settle".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_unsettle(
    state: &DeciderThreadState,
    cmd: &ThreadUnsettleCommand,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, &cmd.thread_id)?;
    thread.settled_override = Some("unsettled".to_owned());
    thread.settled_at = None;
    thread.unsettled_at = Some(Some(now_iso.to_owned()));
    thread.active_order_key = Some(Some(now_iso.to_owned()));
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        &cmd.command_id,
        0,
        &cmd.thread_id,
        now_iso,
        "thread.unsettled",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: cmd.command_id.clone(),
        command_type: "thread.unsettle".to_owned(),
        thread_id: cmd.thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_snooze(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    snoozed_until: &str,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    if thread.archived_at.is_some() {
        return Err(DeciderError::ThreadArchived(thread_id.clone()));
    }
    if state.runs.iter().any(is_run_active) {
        return Err(DeciderError::CannotSnoozeActiveThread(thread_id.clone()));
    }
    let until_ms = parse_iso_millis(snoozed_until);
    let now_ms = parse_iso_millis(now_iso);
    if matches!((until_ms, now_ms), (Some(u), Some(n)) if u > n) {
        thread.snoozed_until = Some(Some(snoozed_until.to_owned()));
        thread.snoozed_at = Some(Some(now_iso.to_owned()));
        thread.last_snooze_wake_at = Some(None);
        now_iso.clone_into(&mut thread.updated_at);
        let event = make_thread_event(
            command_id,
            0,
            thread_id,
            now_iso,
            "thread.snoozed",
            thread.clone(),
        )?;
        Ok(CommandDecision {
            command_id: command_id.clone(),
            command_type: "thread.snooze".to_owned(),
            thread_id: thread_id.clone(),
            events: vec![event],
            outbox_effects: Vec::new(),
            rollback_intent: None,
            resulting_thread: Some(thread),
        })
    } else {
        Err(DeciderError::InvalidSnoozeTimestamp {
            snoozed_until: snoozed_until.to_owned(),
            now_iso: now_iso.to_owned(),
        })
    }
}

pub(crate) fn decide_thread_unsnooze(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    if thread
        .snoozed_until
        .as_ref()
        .and_then(|o| o.as_ref())
        .is_none()
    {
        return Err(DeciderError::ThreadNotSnoozed(thread_id.clone()));
    }
    thread.snoozed_until = Some(None);
    thread.snoozed_at = Some(None);
    thread.last_snooze_wake_at = Some(Some(now_iso.to_owned()));
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.unsnoozed",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.unsnooze".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_pin(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    order_key: Option<&str>,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    if thread.archived_at.is_some() {
        return Err(DeciderError::ThreadArchived(thread_id.clone()));
    }
    thread.pinned_at = Some(Some(now_iso.to_owned()));
    thread.pin_order_key = Some(Some(order_key.unwrap_or(now_iso).to_owned()));
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.pinned",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.pin".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}

pub(crate) fn decide_thread_unpin(
    state: &DeciderThreadState,
    command_id: &CommandId,
    thread_id: &ThreadId,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut thread = require_live_thread(state, thread_id)?;
    thread.pinned_at = Some(None);
    thread.pin_order_key = Some(None);
    now_iso.clone_into(&mut thread.updated_at);
    let event = make_thread_event(
        command_id,
        0,
        thread_id,
        now_iso,
        "thread.unpinned",
        thread.clone(),
    )?;
    Ok(CommandDecision {
        command_id: command_id.clone(),
        command_type: "thread.unpin".to_owned(),
        thread_id: thread_id.clone(),
        events: vec![event],
        outbox_effects: Vec::new(),
        rollback_intent: None,
        resulting_thread: Some(thread),
    })
}
