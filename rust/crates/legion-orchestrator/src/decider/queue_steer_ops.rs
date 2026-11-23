//! Verbatim slash-command context deferral (`#30261`), steer guard (`#40316`), and
//! missed-steer queue placement / fallback model resolution (`#40622`, `#40373`).

use legion_contracts::orchestration::{ConversationMessage, ModelSelection, Run, RunStatus};
use serde_json::Value;

use crate::decider::queue_ops::{is_slash_command, next_queue_position};
use crate::decider::types::DeciderError;

/// Returns `true` when `message` is a user slash command opening the prompt (`#30261`), allowing
/// non-whitespace non-slash command tokens (`^\s*/[^\s/]+(?:\s|$)`).
#[must_use]
pub fn is_slash_command_message(message: &ConversationMessage) -> bool {
    message.role == "user" && is_slash_command_token(&message.text)
}

fn is_slash_command_token(text: &str) -> bool {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first.is_whitespace() || first == '/' {
        return false;
    }
    for ch in chars {
        if ch.is_whitespace() {
            return true;
        }
        if ch == '/' {
            return false;
        }
    }
    true
}

/// Returns `true` when a turn carries a native slash command and must pass its prompt verbatim
/// while deferring pending inline context and carry-over attachments to the next turn (`#30261`).
#[must_use]
pub fn should_defer_context_for_turn(message: &ConversationMessage) -> bool {
    is_slash_command_message(message)
}

/// Formats provider prompt text for a turn: native slash commands pass verbatim when pending
/// context is deferred (`#30261`).
#[must_use]
pub fn format_provider_turn_prompt(
    message: &ConversationMessage,
    inline_context: &str,
    request_text: &str,
) -> String {
    if should_defer_context_for_turn(message) || inline_context.is_empty() {
        request_text.to_owned()
    } else {
        format!("{inline_context}\n\nUser message:\n{request_text}")
    }
}

/// Rejects steering an active turn with a slash command unless `force_restart` (`RestartActive`)
/// explicitly yields the active turn (`#40316`).
pub fn ensure_steer_allows_command(
    force_restart: bool,
    text: &str,
    attachments: &[Value],
) -> Result<(), DeciderError> {
    if !force_restart && is_slash_command(text, attachments) {
        return Err(DeciderError::InvalidRunState(
            "Commands must run as a separate turn. Queue it or wait for the active turn to finish."
                .to_owned(),
        ));
    }
    Ok(())
}

/// Resolves fallback `model_selection` when a missed steer requeues (`#40373`): messages marked
/// `follows_thread_selection: Some(true)` omit an explicit model so they pick up the thread's
/// current selection instead of freezing the superseded turn's model.
#[must_use]
pub fn resolve_steer_fallback_model_selection(
    message: Option<&ConversationMessage>,
    steer_or_run_model: ModelSelection,
) -> Option<ModelSelection> {
    if message.is_some_and(|msg| msg.follows_thread_selection == Some(true)) {
        None
    } else {
        Some(steer_or_run_model)
    }
}

/// Queue placement computed when an attempted steer missed its active turn (`#40622`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedSteerPlacement {
    /// Target `queuePosition` placed ahead of runs queued after `after_run_ordinal`.
    pub queue_position: u64,
    /// Queued runs displaced backward (`queuePosition + 1`) and released from `queueHeld` if held.
    pub shifted_runs: Vec<Run>,
}

/// Computes queue placement for a missed steer targeting `after_run_ordinal` (`#40622`), even when
/// `steerFollowUpOfRunId` was absent, and releases held runs pushed back by the insertion.
#[must_use]
pub fn plan_missed_steer_requeue(runs: &[Run], after_run_ordinal: u64) -> MissedSteerPlacement {
    let mut queued: Vec<&Run> = runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued)
        .collect();
    queued.sort_by_key(|r| (r.queue_position.flatten().unwrap_or(r.ordinal), r.ordinal));
    let insert_idx = queued
        .iter()
        .position(|r| r.ordinal > after_run_ordinal)
        .unwrap_or(queued.len());
    let queue_position = queued
        .get(insert_idx)
        .and_then(|r| r.queue_position.flatten().or(Some(r.ordinal)))
        .unwrap_or_else(|| next_queue_position(runs));
    let shifted_runs = queued[insert_idx..]
        .iter()
        .map(|queued_run| {
            let mut updated = (*queued_run).clone();
            let base = updated.queue_position.flatten().unwrap_or(updated.ordinal);
            updated.queue_position = Some(Some(base.saturating_add(1)));
            if updated.queue_held == Some(true) {
                updated.queue_held = Some(false);
            }
            updated
        })
        .collect();
    MissedSteerPlacement {
        queue_position,
        shifted_runs,
    }
}
