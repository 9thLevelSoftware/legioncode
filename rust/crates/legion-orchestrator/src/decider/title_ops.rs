//! Pre-first-message custom title preservation (`#40567`) and internal `/compact`
//! user-attribution exclusions (`#40934`).

use legion_contracts::orchestration::{
    AppThread, ConversationMessage, OrchestrationActor, ThreadCreateCommand,
};
use serde_json::Value;

use crate::decider::create_ops::decide_thread_create;
use crate::decider::types::{CommandDecision, DeciderError};
use crate::sink::projection_apply::DeciderThreadState;

const PLACEHOLDER_TITLES: &[&str] = &["", "New thread", "Untitled thread"];

/// Returns `true` when `thread` has an explicit user-chosen title or matches `title_seed` before
/// its first prompt (`#40567`).
#[must_use]
pub fn has_custom_title_before_first_message(thread: &AppThread, title_seed: Option<&str>) -> bool {
    if thread.title_renamed == Some(true) {
        return true;
    }
    let trimmed = thread.title.trim();
    if trimmed.is_empty() || PLACEHOLDER_TITLES.contains(&trimmed) {
        return false;
    }
    title_seed.is_some_and(|seed| seed.trim() == trimmed)
}

/// Returns `true` when sending a message should seed an initial thread title (`#40567`).
///
/// Explicitly preserves titles where `thread.title_renamed == Some(true)` even if the only prior
/// history on the thread is maintenance commands such as `/compact`.
#[must_use]
pub fn should_seed_title_from_first_message(
    thread: &AppThread,
    title_seed: Option<&str>,
    is_native_maintenance: bool,
    created_by: OrchestrationActor,
    only_maintenance_history: bool,
) -> bool {
    if is_native_maintenance {
        return false;
    }
    if !has_custom_title_before_first_message(thread, title_seed) {
        return true;
    }
    created_by == OrchestrationActor::User
        && thread.title_renamed != Some(true)
        && only_maintenance_history
}

/// Applies `ThreadCreateCommand` and sets `thread.title_renamed = Some(true)` when requested
/// (`#40567`) until Wave 1 adds `title_renamed` directly onto `ThreadCreateCommand`.
pub fn decide_thread_create_with_title_renamed(
    state: &DeciderThreadState,
    cmd: &ThreadCreateCommand,
    title_renamed: bool,
    now_iso: &str,
) -> Result<CommandDecision, DeciderError> {
    let mut decision = decide_thread_create(state, cmd, now_iso)?;
    if !title_renamed {
        return Ok(decision);
    }
    if let Some(ref mut thread) = decision.resulting_thread {
        thread.title_renamed = Some(true);
    }
    for event in &mut decision.events {
        if let legion_contracts::orchestration::DomainEventPayload::ThreadState { thread, .. } =
            &mut event.payload
        {
            thread.title_renamed = Some(true);
        }
    }
    Ok(decision)
}

/// Returns effective `createdBy` actor for read/MCP views: internal compact-before-send
/// messages (`idle_compaction.is_some()`) report [`OrchestrationActor::System`] (`#40934`).
#[must_use]
pub const fn effective_message_created_by(message: &ConversationMessage) -> OrchestrationActor {
    if message.idle_compaction.is_some() {
        OrchestrationActor::System
    } else {
        message.created_by
    }
}

/// Wire-JSON variant of [`effective_message_created_by`] (`#40934`): returns `"system"` when
/// `idleCompaction` is non-null.
#[must_use]
pub fn effective_message_created_by_json(message: &Value) -> &str {
    if message
        .get("idleCompaction")
        .is_some_and(|value| !value.is_null())
    {
        "system"
    } else {
        message
            .get("createdBy")
            .and_then(Value::as_str)
            .unwrap_or("user")
    }
}
