//! Subagent delegation, idle compaction gating, and hook stop-reason preservation
//! (`3a60d0996a` / #38869, `ec275c7a19` / #39455, `9cc4018c75` / #39766, `c440fd19db` / #37859).

use serde_json::Value;
use t3_contracts::ids::{NodeId, ProviderInstanceId, RunId, ThreadId, TurnItemId};
use t3_contracts::orchestration::{
    AppThread, Run, RunStatus, Subagent, TurnItem, TurnItemKind, TurnItemStatus,
};

use crate::decider::queue_ops::is_compact_command;
use crate::decider::thread_ops::is_run_active;

/// Deterministic run signal identifier for provider hook stop reasons (`#37859`).
pub const PROVIDER_STOP_REASON_SIGNAL: &str = "provider-stop-reason";

/// Deterministic run signal identifier for interrupted-turn notices (`#37859`).
pub const INTERRUPT_RESULT_SIGNAL: &str = "interrupt-result";

/// Returns `true` when `pending_tasks` contains an active native subagent owned by
/// `provider_instance_id` (`#38869`).
#[must_use]
pub fn has_pending_background_subagent(
    pending_tasks: &[Value],
    provider_instance_id: Option<&ProviderInstanceId>,
) -> bool {
    let Some(expected) = provider_instance_id else {
        return false;
    };
    pending_tasks.iter().any(|entry| {
        let kind = entry.get("kind").and_then(Value::as_str);
        let provider = entry.get("providerInstanceId").and_then(Value::as_str);
        kind == Some("subagent") && provider == Some(expected.as_str())
    })
}

/// Returns `true` when a background subagent outlives its settled run so it may still execute
/// tools and delegate further child tasks (`#38869`).
#[must_use]
pub fn background_subagent_outlives_run(
    thread: &AppThread,
    active_run_id: Option<&RunId>,
    latest_run: Option<&Run>,
    pending_tasks: &[Value],
) -> bool {
    let Some(latest) = latest_run else {
        return false;
    };
    thread.archived_at.is_none()
        && thread.deleted_at.is_none()
        && active_run_id.is_none()
        && has_pending_background_subagent(pending_tasks, Some(&latest.provider_instance_id))
}

/// Selects the parent run onto which child delegation cohorts attach (`#38869`):
/// prefers an active run, or falls back to the latest settled run when a native background
/// subagent outlives it.
#[must_use]
pub fn pick_delegation_parent_run<'a>(
    thread: &AppThread,
    runs: &'a [Run],
    pending_tasks: &[Value],
) -> Option<&'a Run> {
    if let Some(active) = runs.iter().rev().find(|r| is_run_active(r)) {
        return Some(active);
    }
    let latest = runs.iter().max_by_key(|r| r.ordinal)?;
    background_subagent_outlives_run(thread, None, Some(latest), pending_tasks).then_some(latest)
}

/// Returns `true` when dispatching `message_text` on `thread` should track a delegated task
/// against its parent thread (`#39455`: `/compact` on a subagent thread never creates one).
#[must_use]
pub fn should_track_delegated_task_on_parent(
    thread: &AppThread,
    message_text: &str,
    attachments: &[Value],
) -> bool {
    thread.lineage.parent_thread_id.is_some() && !is_compact_command(message_text, attachments)
}

/// Returns `true` when an active turn item blocks automatic idle compaction (`#39766`).
///
/// `app_owned` delegated subagents report their completion back via their own wake message, so
/// an idle parent thread may compact while an `app_owned` child subagent keeps running.
#[must_use]
pub fn has_blocking_work_for_idle_compaction(turn_items: &[TurnItem]) -> bool {
    turn_items.iter().any(|item| {
        if !is_turn_item_active(item.base.status) {
            return false;
        }
        !matches!(
            &item.kind,
            TurnItemKind::Subagent { origin, .. } if origin == "app_owned"
        )
    })
}

/// Returns `true` when a [`TurnItemStatus`] represents active/unfinished work.
#[must_use]
pub const fn is_turn_item_active(status: TurnItemStatus) -> bool {
    matches!(
        status,
        TurnItemStatus::Idle
            | TurnItemStatus::Pending
            | TurnItemStatus::Running
            | TurnItemStatus::Waiting
    )
}

/// Derives the signal turn-item identifier convention for `run_id` and `signal`.
#[must_use]
pub fn run_signal_turn_item_id(run_id: &RunId, signal: &str) -> Option<TurnItemId> {
    TurnItemId::new(format!("{}:{signal}", run_id.as_str())).ok()
}

/// Finds the hook stop-reason or interrupt-result message recorded for `run_id` (`#37859`).
#[must_use]
pub fn subagent_stop_notice<'a>(turn_items: &'a [TurnItem], run_id: &RunId) -> Option<&'a str> {
    let stop_id = format!("{}:{PROVIDER_STOP_REASON_SIGNAL}", run_id.as_str());
    let interrupt_id = format!("{}:{INTERRUPT_RESULT_SIGNAL}", run_id.as_str());
    for item in turn_items {
        if item.base.run_id.as_ref() != Some(run_id) {
            continue;
        }
        let raw_id = item.base.id.as_str();
        let is_signal_item = raw_id == stop_id
            || raw_id == interrupt_id
            || raw_id.ends_with(&format!(":{PROVIDER_STOP_REASON_SIGNAL}"))
            || raw_id.ends_with(&format!(":{INTERRUPT_RESULT_SIGNAL}"));
        if !is_signal_item {
            continue;
        }
        match &item.kind {
            TurnItemKind::SystemNotice { message, .. }
            | TurnItemKind::RunInterruptResult { message } => {
                return Some(message.as_str());
            }
            _ => {}
        }
    }
    None
}

/// Builds the finalized delegated-task result string for `run`, appending any recorded
/// `hook_stop_reason` / interrupt notice (`#37859`) even if no assistant report was emitted.
#[must_use]
pub fn resolve_delegated_task_result(
    run: &Run,
    assistant_text: Option<&str>,
    failure_message: Option<&str>,
    assistant_before_failure: Option<&str>,
    turn_items: &[TurnItem],
) -> String {
    let base_text = if let Some(err) = failure_message {
        match assistant_before_failure {
            Some(prev) => {
                format!("{err}\n\nLast assistant message before the run failed:\n\n{prev}")
            }
            None => err.to_owned(),
        }
    } else if let Some(report) = assistant_text {
        report.to_owned()
    } else if run.status == RunStatus::Completed {
        "Child task completed without an assistant result.".to_owned()
    } else {
        format!(
            "Child task ended with status {}.",
            run_status_label(run.status)
        )
    };

    match subagent_stop_notice(turn_items, &run.id) {
        Some(notice) if !base_text.contains(notice) => format!("{base_text}\n\n{notice}"),
        _ => base_text,
    }
}

/// Applies an incoming subagent progress update without erasing finalized result text or
/// hook stop reasons that were already recorded (`#37859`).
#[must_use]
pub fn apply_subagent_progress_preserving_result(
    current: &Subagent,
    progress: Option<String>,
    now_iso: &str,
) -> Subagent {
    let mut updated = current.clone();
    updated.progress = progress;
    now_iso.clone_into(&mut updated.updated_at);
    // Preserve finalized `status`, `completed_at`, and `result` (including appended stop notice).
    updated
}

/// Placeholder helper ensuring `ThreadId`/`NodeId` imports remain self-contained for callers.
#[must_use]
pub fn is_subagent_node(node_id: &NodeId, subagent: &Subagent) -> bool {
    subagent.id == *node_id
}

/// Placeholder helper checking whether `thread_id` matches a child subagent thread.
#[must_use]
pub fn is_child_subagent_thread(child_thread_id: &ThreadId, subagent: &Subagent) -> bool {
    subagent.child_thread_id.as_ref() == Some(child_thread_id)
}

const fn run_status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Preparing => "preparing",
        RunStatus::Queued => "queued",
        RunStatus::Starting => "starting",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Completed => "completed",
        RunStatus::Interrupted => "interrupted",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::RolledBack => "rolled_back",
    }
}
