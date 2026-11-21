//! Scheduled task thread-binding and explicit unbind helpers (`898c22f990` / #39639).

use t3_contracts::ids::{ProjectId, ThreadId};

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
