//! Delegated child progress/stop-race protections (`#41140`), parent stop-interrupted task
//! notes (`#34499`), open-task error listings (`#40886`), parent delivery sync retry policy
//! (`#37225`), and linear run-to-provider-turn lookup (`#38791`).

use std::collections::HashSet;
use std::hash::BuildHasher;

use legion_contracts::ids::{NodeId, ProviderThreadId, RunAttemptId, RunId, ThreadId};
use legion_contracts::orchestration::{
    ConversationMessage, OrchestrationActor, ProviderThread, ProviderTurn, Run, RunAttempt,
    RunStatus, Subagent, TurnItem, TurnItemKind,
};
use serde_json::Value;

use crate::time::parse_iso_millis;

/// Maximum open delegated tasks formatted in `task_not_found` errors (`#40886`).
pub const MAX_LISTED_DELEGATED_TASKS: usize = 10;
/// Maximum stopped delegated tasks expanded in restart notes before collapsing to count (`#34499`).
const MAX_STOPPED_TASK_NOTE_ENTRIES: usize = 12;
/// Base retry backoff (`1` second) for parent task delivery/finalization sync (`#37225`).
pub const PARENT_TASK_SYNC_BASE_DELAY_MS: u64 = 1_000;
/// Maximum retry backoff cap (`30` seconds) per retry step (`#37225`).
pub const PARENT_TASK_SYNC_MAX_DELAY_MS: u64 = 30_000;
/// Total retry window (`1` hour) matching `Schedule.upTo({ duration: Duration.hours(1) })` (`#37225`).
pub const PARENT_TASK_SYNC_MAX_WINDOW_MS: u64 = 3_600_000;

/// Coarse progress state of a delegated child thread (`packages/provider-core/src/server/subagentProjection.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedTaskProgressState {
    /// Child is actively executing or waiting on a user-written queued run.
    Working,
    /// Child itself finished, but nested child subagents or provider threads remain active.
    WaitingForChildren,
    /// Result is available to report back to the parent.
    ResultAvailable,
}

/// Progress snapshot computed by [`delegated_task_progress`] (`#41140`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedTaskProgress<'a> {
    /// Aggregate child task state.
    pub state: DelegatedTaskProgressState,
    /// Latest settled run carrying the child's result, if any.
    pub result_run: Option<&'a Run>,
    /// `true` when working ONLY because a user-authored run is queued and held (`#41140`).
    pub only_awaits_user_run: bool,
}

fn is_terminal_run_status(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed
            | RunStatus::Failed
            | RunStatus::Interrupted
            | RunStatus::Cancelled
            | RunStatus::RolledBack
    )
}

fn is_held_queued_run(run: &Run) -> bool {
    run.status == RunStatus::Queued && run.queue_held == Some(true)
}

fn subagent_holds_completion(task: &Subagent) -> bool {
    let terminal = matches!(
        task.status.as_str(),
        "completed" | "failed" | "interrupted" | "cancelled"
    );
    let delivery_done = task
        .completion_delivery
        .as_ref()
        .and_then(|val| val.get("state"))
        .and_then(Value::as_str)
        .is_some_and(|st| matches!(st, "acknowledged" | "delivered" | "disposed"));
    !terminal || !delivery_done
}

/// Returns queued held run IDs whose `user` messages must be read when inspecting child progress (`#41140`).
#[must_use]
pub fn held_queued_run_ids(runs: &[Run]) -> Vec<RunId> {
    runs.iter()
        .filter(|run| is_held_queued_run(run))
        .map(|run| run.id.clone())
        .collect()
}

/// Computes child delegated task progress while distinguishing held wakes from held user messages (`#41140`).
#[must_use]
pub fn delegated_task_progress<'a>(
    runs: &'a [Run],
    messages: &[ConversationMessage],
    subagents: &[Subagent],
    provider_threads: &[ProviderThread],
) -> DelegatedTaskProgress<'a> {
    let monitor_runs: HashSet<&RunId> = messages
        .iter()
        .filter(|msg| {
            msg.notification.as_ref().is_some_and(|notif| {
                notif.source.get("kind").and_then(Value::as_str) == Some("monitor")
            })
        })
        .filter_map(|msg| msg.run_id.as_ref())
        .collect();
    let work_runs: Vec<&Run> = runs
        .iter()
        .filter(|run| !monitor_runs.contains(&run.id) && run.status != RunStatus::RolledBack)
        .collect();
    let active = work_runs
        .iter()
        .any(|run| !is_terminal_run_status(run.status) && !is_held_queued_run(run));
    // Held wakes wait on nobody, whereas a held message written by the user is next work.
    let user_run_ids: HashSet<&RunId> = messages
        .iter()
        .filter(|msg| msg.created_by == OrchestrationActor::User && msg.notification.is_none())
        .filter_map(|msg| msg.run_id.as_ref())
        .collect();
    let awaits_user_run = work_runs
        .iter()
        .any(|run| is_held_queued_run(run) && user_run_ids.contains(&run.id));
    let children = subagents.iter().any(subagent_holds_completion)
        || provider_threads
            .iter()
            .any(|pt| !pt.pending_background_tasks.is_empty());
    let result_run = work_runs
        .into_iter()
        .filter(|run| run.started_at.is_some() && is_terminal_run_status(run.status))
        .max_by_key(|run| run.ordinal);

    let state = if active || awaits_user_run || result_run.is_none() {
        DelegatedTaskProgressState::Working
    } else if children {
        DelegatedTaskProgressState::WaitingForChildren
    } else {
        DelegatedTaskProgressState::ResultAvailable
    };

    DelegatedTaskProgress {
        state,
        result_run,
        only_awaits_user_run: awaits_user_run && !active && result_run.is_some(),
    }
}

/// Returns `true` when cancelling a delegated child must still wait on real active work (`#41140`).
///
/// A child thread that only holds an unstarted user-authored queued message does NOT block
/// finalizing the cancelled task.
#[must_use]
pub const fn should_wait_for_cancelled_task(progress: &DelegatedTaskProgress<'_>) -> bool {
    matches!(progress.state, DelegatedTaskProgressState::Working) && !progress.only_awaits_user_run
}

/// Returns `true` when `completion_delivery` was disposed by parent Stop and a
/// `run_interrupt_request` turn item targeted a real provider turn (`#34499`).
#[must_use]
pub fn is_child_interrupted_by_parent_stop(
    completion_delivery: Option<&Value>,
    turn_items: &[TurnItem],
) -> bool {
    let disposed = completion_delivery
        .and_then(|val| val.get("state"))
        .and_then(Value::as_str)
        == Some("disposed");
    if !disposed {
        return false;
    }
    turn_items.iter().any(|item| {
        matches!(item.kind, TurnItemKind::RunInterruptRequest { .. })
            && item.base.provider_turn_id.is_some()
    })
}

/// Finds delegated tasks on `provider_thread_id` that Stop interrupted and that have not yet been
/// reported to the user/model by a completed non-compaction turn requested after `completedAt` (`#34499`).
#[must_use]
pub fn pending_stopped_delegated_tasks<'a, S: BuildHasher>(
    provider_thread_id: Option<&ProviderThreadId>,
    runs: &[Run],
    provider_turns: &[ProviderTurn],
    attempts: &[RunAttempt],
    subagents: &'a [Subagent],
    compaction_message_ids: &HashSet<String, S>,
) -> Vec<&'a Subagent> {
    let Some(pt_id) = provider_thread_id else {
        return Vec::new();
    };
    let stopped_run_ids: HashSet<&RunId> = runs
        .iter()
        .filter(|run| {
            run.provider_thread_id.as_ref() == Some(pt_id)
                && run
                    .delegated_completion
                    .as_ref()
                    .and_then(|dc| dc.get("disposition"))
                    .and_then(Value::as_str)
                    == Some("stopped")
        })
        .map(|run| &run.id)
        .collect();
    if stopped_run_ids.is_empty() {
        return Vec::new();
    }
    let completed_attempt_ids: HashSet<&RunAttemptId> = provider_turns
        .iter()
        .filter(|turn| turn.status == "completed")
        .filter_map(|turn| turn.run_attempt_id.as_ref())
        .collect();
    let told_requests_ms: Vec<i64> = runs
        .iter()
        .filter(|run| {
            run.provider_thread_id.as_ref() == Some(pt_id)
                && !compaction_message_ids.contains(run.user_message_id.as_str())
                && attempts.iter().any(|attempt| {
                    attempt.run_id == run.id && completed_attempt_ids.contains(&attempt.id)
                })
        })
        .filter_map(|run| parse_iso_millis(&run.requested_at))
        .collect();

    subagents
        .iter()
        .filter(|task| {
            task.origin == "app_owned"
                && task
                    .run_id
                    .as_ref()
                    .is_some_and(|rid| stopped_run_ids.contains(rid))
                && task.status == "interrupted"
                && task.cancelled_run_id.is_none()
                && task
                    .completion_delivery
                    .as_ref()
                    .and_then(|cd| cd.get("state"))
                    .and_then(Value::as_str)
                    == Some("disposed")
                && task
                    .completed_at
                    .as_deref()
                    .and_then(parse_iso_millis)
                    .is_some_and(|completed_ms| {
                        !told_requests_ms.iter().any(|&req_ms| req_ms > completed_ms)
                    })
        })
        .collect()
}

/// Formats the continuation note telling a parent agent which delegated tasks Stop interrupted (`#34499`).
#[must_use]
pub fn stopped_delegated_tasks_note(tasks: &[&Subagent]) -> String {
    let mut lines = vec![
        "Note: Stop on this thread also stopped these delegated tasks before they finished. They will not report back on their own, so check them with task_status and start again what you still need:".to_owned(),
    ];
    for task in tasks.iter().take(MAX_STOPPED_TASK_NOTE_ENTRIES) {
        let raw_label = task
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(task.prompt.as_str());
        let compact = raw_label.split_whitespace().collect::<Vec<_>>().join(" ");
        let bounded: String = compact.chars().take(80).collect();
        let display = if bounded.is_empty() {
            "delegated task".to_owned()
        } else {
            bounded
        };
        lines.push(format!("- {display} (task {})", task.id));
    }
    let omitted = tasks.len().saturating_sub(MAX_STOPPED_TASK_NOTE_ENTRIES);
    if omitted > 0 {
        lines.push(format!("- and {omitted} more"));
    }
    lines.join("\n")
}

/// Formats `task_not_found` errors including up to `10` open `app_owned` delegated tasks (`#40886`).
#[must_use]
pub fn format_task_not_found(
    thread_id: &ThreadId,
    task_id: &NodeId,
    subagents: &[Subagent],
) -> String {
    let open: Vec<&Subagent> = subagents
        .iter()
        .filter(|task| {
            task.origin == "app_owned"
                && (task.child_thread_id.is_some() || task.remote_child.is_some())
        })
        .collect();
    if open.is_empty() {
        return format!(
            "No delegated task with id {task_id} exists on thread {thread_id}. This thread has no open delegated tasks."
        );
    }
    let listed: String = open
        .iter()
        .take(MAX_LISTED_DELEGATED_TASKS)
        .map(|task| match task.title.as_deref() {
            Some(title) if !title.trim().is_empty() => format!("{} ({title})", task.id),
            _ => task.id.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let more = if open.len() > MAX_LISTED_DELEGATED_TASKS {
        format!(", and {} more", open.len() - MAX_LISTED_DELEGATED_TASKS)
    } else {
        String::new()
    };
    format!(
        "No delegated task with id {task_id} exists on thread {thread_id}. Open delegated tasks on this thread: {listed}{more}."
    )
}

/// Computes bounded exponential backoff (`1s`..`30s` cap, stopped after `1h` total) for
/// `parentTaskSyncRetry` (`#37225`).
#[must_use]
pub fn parent_task_sync_retry_delay_ms(attempt_zero_based: u32, elapsed_ms: u64) -> Option<u64> {
    if elapsed_ms >= PARENT_TASK_SYNC_MAX_WINDOW_MS {
        return None;
    }
    let shift = attempt_zero_based.min(10);
    let raw = PARENT_TASK_SYNC_BASE_DELAY_MS.saturating_mul(1_u64 << shift);
    let capped = raw.min(PARENT_TASK_SYNC_MAX_DELAY_MS);
    let remaining = PARENT_TASK_SYNC_MAX_WINDOW_MS.saturating_sub(elapsed_ms);
    Some(capped.min(remaining))
}

/// Collects parent `run_id`s associated with `child_thread_id` so retries offer pending wake
/// deliveries before re-finalizing parent tasks (`#37225`).
#[must_use]
pub fn parent_run_ids_for_child(subagents: &[Subagent], child_thread_id: &ThreadId) -> Vec<RunId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for task in subagents {
        if task.child_thread_id.as_ref() == Some(child_thread_id)
            && let Some(ref run_id) = task.run_id
            && seen.insert(run_id.clone())
        {
            out.push(run_id.clone());
        }
    }
    out
}

/// Resolves the active provider turn for `run` in linear `O(N)` time instead of quadratic nested
/// scans (`#38791`).
#[must_use]
pub fn provider_turn_for_run<'a>(
    provider_turns: &'a [ProviderTurn],
    attempts: &[RunAttempt],
    run: &Run,
) -> Option<&'a ProviderTurn> {
    let active_attempt_id = run.active_attempt_id.as_ref()?;
    if let Some(latest) = provider_turns
        .iter()
        .filter(|turn| turn.run_attempt_id.as_ref() == Some(active_attempt_id))
        .max_by_key(|turn| turn.ordinal)
    {
        return Some(latest);
    }
    let named_turn_id = attempts
        .iter()
        .find(|candidate| &candidate.id == active_attempt_id)?
        .provider_turn_id
        .as_ref()?;
    provider_turns.iter().find(|turn| &turn.id == named_turn_id)
}
