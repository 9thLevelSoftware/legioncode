//! Scheduled task thread-binding and explicit unbind helpers (`898c22f990` / #39639).

use legion_contracts::ids::{ProjectId, ScheduledTaskId, ThreadId};
use thiserror::Error;

/// Maximum UTF-16 code units / byte ceiling kept in `scheduled_task_webhook_deliveries.rendered_prompt` (`#40726`).
pub const WEBHOOK_DELIVERY_LOG_PROMPT_LIMIT: usize = 64 * 1024;

/// Error returned when saving/editing a scheduled task that was already deleted (`#41084`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScheduledTaskEditError {
    /// Edit requested `require_existing: true`, but the schedule no longer exists.
    #[error("Scheduled task `{0}` was deleted and cannot be recreated by an edit.")]
    DeletedOrMissing(ScheduledTaskId),
}

/// Ensures `require_existing` edits fail cleanly instead of resurrecting deleted schedules (`#41084`).
pub fn ensure_scheduled_task_exists_for_edit(
    task_id: &ScheduledTaskId,
    exists: bool,
    require_existing: bool,
) -> Result<(), ScheduledTaskEditError> {
    if require_existing && !exists {
        return Err(ScheduledTaskEditError::DeletedOrMissing(task_id.clone()));
    }
    Ok(())
}

/// Truncates `prompt` to at most [`WEBHOOK_DELIVERY_LOG_PROMPT_LIMIT`] without ever splitting a
/// multi-byte UTF-8 scalar boundary or leaving a broken trailing surrogate pair (`#40726`).
#[must_use]
pub fn truncate_webhook_delivery_prompt(prompt: &str) -> &str {
    truncate_prompt_to_limit(prompt, WEBHOOK_DELIVERY_LOG_PROMPT_LIMIT)
}

/// Bounded UTF-8 & UTF-16 safe prefix truncation (`#40726`).
#[must_use]
pub fn truncate_prompt_to_limit(prompt: &str, max_utf16_or_bytes: usize) -> &str {
    let mut byte_end = 0;
    let mut utf16_used = 0;
    for (idx, ch) in prompt.char_indices() {
        let ch_bytes = ch.len_utf8();
        let ch_utf16 = ch.len_utf16();
        if idx + ch_bytes > max_utf16_or_bytes || utf16_used + ch_utf16 > max_utf16_or_bytes {
            break;
        }
        byte_end = idx + ch_bytes;
        utf16_used += ch_utf16;
    }
    &prompt[..byte_end]
}

/// Input describing an upsert mutation on a scheduled task's bound `thread_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTaskBindingInput<'a> {
    /// Currently persisted `thread_id` before saving, if any.
    pub existing_thread_id: Option<&'a ThreadId>,
    /// Currently persisted `project_id` before saving, if any.
    pub existing_project_id: Option<&'a ProjectId>,
    /// Target `project_id` supplied by the save request.
    pub target_project_id: &'a ProjectId,
    /// `thread_id` supplied by the save request (`None` when omitted or explicitly cleared).
    pub requested_thread_id: Option<&'a ThreadId>,
    /// Explicit signal from the UI/caller that the user cleared `thread_id` (`#39639`).
    pub unbind_thread: bool,
    /// Effective thread execution mode (`"new_thread"`, `"continue"`, or `"fresh_session"`).
    pub thread_mode: &'a str,
}

/// Output of resolving scheduled-task thread retention vs explicit unbind (`#39639`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTaskThreadBinding {
    /// Stored `thread_id` value to persist (`None` clears the stored field).
    pub resolved_thread_id: Option<ThreadId>,
    /// Specific prior thread ID cleared by this unbind so SQL `NULLIF(thread_id, unbound)` clears
    /// stale rows without wiping a concurrently started new thread.
    pub unbound_thread_id: Option<ThreadId>,
    /// True when `thread_mode == "new_thread"` or moving across projects clears unconditionally.
    pub clear_unconditionally: bool,
}

/// Resolves whether updating a scheduled task keeps its bound `thread_id` or overwrites it with
/// `None` (`898c22f990` / #39639).
#[must_use]
pub fn resolve_scheduled_task_thread_binding(
    input: &ScheduledTaskBindingInput<'_>,
) -> ScheduledTaskThreadBinding {
    if input.thread_mode == "new_thread" {
        return ScheduledTaskThreadBinding {
            resolved_thread_id: None,
            unbound_thread_id: input.existing_thread_id.cloned(),
            clear_unconditionally: true,
        };
    }

    let moved_off_project = input
        .existing_project_id
        .is_some_and(|prev| prev != input.target_project_id);

    let base_thread_id = if moved_off_project {
        input.requested_thread_id.cloned()
    } else if let Some(explicit) = input.requested_thread_id {
        Some(explicit.clone())
    } else if input.unbind_thread {
        None
    } else {
        input.existing_thread_id.cloned()
    };

    let unbound_thread_id = if base_thread_id.is_none() && input.unbind_thread {
        input.existing_thread_id.cloned()
    } else {
        None
    };

    ScheduledTaskThreadBinding {
        clear_unconditionally: moved_off_project && base_thread_id.is_none(),
        resolved_thread_id: base_thread_id,
        unbound_thread_id,
    }
}

/// Applies the conflict-safe SQLite merge expression
/// `COALESCE(excluded_thread_id, NULLIF(stored_thread_id, unbound_thread_id))` in pure Rust,
/// returning the resulting `thread_id` (`None` when unbound cleanly, or preserving any concurrently
/// launched new thread ID).
#[must_use]
pub fn apply_scheduled_task_thread_unbind<'a>(
    stored_thread_id: Option<&'a ThreadId>,
    binding: &'a ScheduledTaskThreadBinding,
) -> Option<&'a ThreadId> {
    if binding.clear_unconditionally {
        return None;
    }
    if let Some(ref resolved) = binding.resolved_thread_id {
        return Some(resolved);
    }
    match (stored_thread_id, &binding.unbound_thread_id) {
        (Some(stored), Some(unbound)) if stored == unbound => None,
        (stored, _) => stored,
    }
}
