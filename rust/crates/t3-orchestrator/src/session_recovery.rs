//! Provider session release cleanup (`97e7834f4b` / #39734) and unexpected runtime-exit
//! background-work notices + continuation recovery (`87707319cf` / #40170).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use t3_contracts::ids::{CommandId, EventId, NodeId, ProviderThreadId, RunId, ThreadId};
use t3_contracts::orchestration::{
    DomainEvent, DomainEventPayload, ExecutionNode, Run, RunBackgroundWorkCancelled, RunStatus,
    TurnItem, TurnItemKind, TurnItemStatus,
};

use crate::subagent_lifecycle::is_turn_item_active;

const MAX_NOTE_ENTRIES: usize = 10;

/// Cause of process-bound background work cancellation (`#40170`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundWorkLossCause {
    /// Host T3 server restarted cleanly or after crash.
    ServerRestart,
    /// Provider runtime subprocess exited unexpectedly while the T3 server stayed live.
    ProviderExit,
}

/// Summary record stored in `RunBackgroundWorkCancelled.restart_cancelled_background_work`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledBackgroundWorkEntry {
    /// Work category (`shell` or `task`).
    pub kind: String,
    /// Human-readable command or tool description.
    pub label: String,
    /// Underlying turn item or shell identifier.
    pub id: String,
}

/// Outcome returned when releasing a provider session cleanly or after unexpected exit.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReleaseResult {
    /// `node.updated` and `turn-item.updated` events transitioning open work to `interrupted`.
    pub termination_events: Vec<DomainEvent>,
    /// `run.background-work-cancelled` notices emitted when background tasks died on exit.
    pub cancelled_work_events: Vec<DomainEvent>,
    /// Runs that need an automatic continuation (`provider_exit` cause) on unexpected exit.
    pub continuation_run_ids: Vec<(CommandId, RunId)>,
}

/// Returns `true` when `kind` represents an active provider tool or command item bound to a
/// running session (`#39734`).
#[must_use]
pub const fn is_session_bound_tool_item(kind: &TurnItemKind) -> bool {
    matches!(
        kind,
        TurnItemKind::CommandExecution { .. }
            | TurnItemKind::DynamicTool { .. }
            | TurnItemKind::FileChange { .. }
            | TurnItemKind::FileSearch { .. }
            | TurnItemKind::WebSearch { .. }
    )
}

/// Returns `true` when an execution node status is still open (`idle`/`pending`/`running`/`waiting`).
#[must_use]
pub fn is_node_work_active(status: &str) -> bool {
    matches!(status, "idle" | "pending" | "running" | "waiting")
}

/// Releases open tool-call items AND their backing execution nodes bound to `released_provider_threads`
/// by transitioning both to `"interrupted"` (`97e7834f4b` / #39734).
pub fn build_session_release_termination_events(
    thread_id: &ThreadId,
    released_provider_threads: &[ProviderThreadId],
    turn_items: &[TurnItem],
    nodes: &[ExecutionNode],
    now_iso: &str,
    mut alloc_event_id: impl FnMut() -> EventId,
) -> Vec<DomainEvent> {
    if released_provider_threads.is_empty() {
        return Vec::new();
    }
    let released_set: HashSet<&ProviderThreadId> = released_provider_threads.iter().collect();
    let mut interrupted_nodes: HashSet<NodeId> = HashSet::new();
    let mut events = Vec::new();

    for item in turn_items {
        if !is_turn_item_active(item.base.status) || !is_session_bound_tool_item(&item.kind) {
            continue;
        }
        let Some(pt_id) = item.base.provider_thread_id.as_ref() else {
            continue;
        };
        if !released_set.contains(pt_id) {
            continue;
        }

        if let Some(node_id) = item.base.node_id.as_ref()
            && interrupted_nodes.insert(node_id.clone())
            && let Some(node) = nodes.iter().find(|n| &n.id == node_id)
            && is_node_work_active(&node.status)
        {
            let mut next_node = node.clone();
            "interrupted".clone_into(&mut next_node.status);
            next_node.completed_at = Some(now_iso.to_owned());
            events.push(DomainEvent {
                id: alloc_event_id(),
                thread_id: thread_id.clone(),
                run_id: next_node.run_id.clone(),
                node_id: Some(next_node.id.clone()),
                driver: None,
                provider_instance_id: None,
                raw_event_id: None,
                occurred_at: now_iso.to_owned(),
                payload: DomainEventPayload::NodeUpdated(next_node),
            });
        }

        let mut next_item = item.clone();
        next_item.base.status = TurnItemStatus::Interrupted;
        next_item.base.completed_at = Some(now_iso.to_owned());
        now_iso.clone_into(&mut next_item.base.updated_at);
        events.push(DomainEvent {
            id: alloc_event_id(),
            thread_id: thread_id.clone(),
            run_id: next_item.base.run_id.clone(),
            node_id: next_item.base.node_id.clone(),
            driver: None,
            provider_instance_id: None,
            raw_event_id: None,
            occurred_at: now_iso.to_owned(),
            payload: DomainEventPayload::TurnItemUpdated(Box::new(next_item)),
        });
    }

    events
}

/// Finds the latest started non-queued, non-rolled-back run attached to `provider_thread_id`
/// (`#40170`).
#[must_use]
pub fn latest_started_run<'a>(
    runs: &'a [Run],
    provider_thread_id: &ProviderThreadId,
) -> Option<&'a Run> {
    runs.iter()
        .filter(|r| {
            r.provider_thread_id.as_ref() == Some(provider_thread_id)
                && !matches!(r.status, RunStatus::Queued | RunStatus::RolledBack)
                && (r.started_at.is_some()
                    || r.work_started_at.is_some()
                    || matches!(
                        r.status,
                        RunStatus::Running | RunStatus::Completed | RunStatus::Waiting
                    ))
        })
        .max_by_key(|r| r.ordinal)
}

/// Records cancelled background work notices (`run.background-work-cancelled`) and schedules
/// continuation wakeups when an unexpected provider runtime exit kills running tasks (`#40170`).
///
/// Intentional `detach` (`unexpected_exit == false`) leaves continuation wakeups unscheduled.
pub fn plan_unexpected_exit_background_work_recovery(
    thread_id: &ThreadId,
    released_provider_threads: &[ProviderThreadId],
    runs: &[Run],
    turn_items: &[TurnItem],
    unexpected_exit: bool,
    now_iso: &str,
    mut alloc_event_id: impl FnMut() -> EventId,
) -> (Vec<DomainEvent>, Vec<(CommandId, RunId)>) {
    if !unexpected_exit || released_provider_threads.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let released_set: HashSet<&ProviderThreadId> = released_provider_threads.iter().collect();
    let mut by_run: BTreeMap<u64, (&Run, Vec<Value>)> = BTreeMap::new();

    for item in turn_items {
        if !is_turn_item_active(item.base.status) || !is_session_bound_tool_item(&item.kind) {
            continue;
        }
        let Some(pt_id) = item.base.provider_thread_id.as_ref() else {
            continue;
        };
        if !released_set.contains(pt_id) {
            continue;
        }
        let Some(run) = latest_started_run(runs, pt_id) else {
            continue;
        };
        let (kind, label) = match &item.kind {
            TurnItemKind::CommandExecution { input, .. } => ("shell", input.clone()),
            TurnItemKind::DynamicTool { tool_name, .. } => (
                "task",
                tool_name
                    .clone()
                    .unwrap_or_else(|| "dynamic_tool".to_owned()),
            ),
            _ => (
                "task",
                item.base
                    .title
                    .clone()
                    .unwrap_or_else(|| "background_work".to_owned()),
            ),
        };
        if let Ok(entry_val) = serde_json::to_value(CancelledBackgroundWorkEntry {
            kind: kind.to_owned(),
            label,
            id: item.base.id.as_str().to_owned(),
        }) {
            by_run
                .entry(run.ordinal)
                .or_insert_with(|| (run, Vec::new()))
                .1
                .push(entry_val);
        }
    }

    let mut events = Vec::new();
    let mut continuations = Vec::new();
    for (_, (run, work)) in by_run {
        let mut merged = run
            .restart_cancelled_background_work
            .clone()
            .unwrap_or_default();
        for w in work {
            if !merged.contains(&w) {
                merged.push(w);
            }
        }
        events.push(DomainEvent {
            id: alloc_event_id(),
            thread_id: thread_id.clone(),
            run_id: Some(run.id.clone()),
            node_id: None,
            driver: None,
            provider_instance_id: Some(run.provider_instance_id.clone()),
            raw_event_id: None,
            occurred_at: now_iso.to_owned(),
            payload: DomainEventPayload::RunBackgroundWorkCancelled(RunBackgroundWorkCancelled {
                run_id: run.id.clone(),
                restart_cancelled_background_work: merged,
            }),
        });
        if let Ok(cmd_id) =
            CommandId::new(format!("provider-exit-continuation:{}", run.id.as_str()))
        {
            continuations.push((cmd_id, run.id.clone()));
        }
    }
    (events, continuations)
}

/// Formats the user/agent-visible cancellation notice inserted on continuation (`#40170`).
#[must_use]
pub fn format_cancelled_background_work_note(
    work: &[CancelledBackgroundWorkEntry],
    cause: BackgroundWorkLossCause,
) -> String {
    let intro = match cause {
        BackgroundWorkLossCause::ProviderExit => {
            "Note: the agent runtime process exited, and this background work was cancelled before it finished. It will not report back on its own, so resume or restart what you still need and wait for its result:"
        }
        BackgroundWorkLossCause::ServerRestart => {
            "Note: the T3 server restarted, and this background work was cancelled before it finished. It will not report back on its own, so resume or restart what you still need and wait for its result:"
        }
    };
    let mut lines = vec![intro.to_owned()];
    for entry in work.iter().take(MAX_NOTE_ENTRIES) {
        lines.push(format!("- {}: {}", entry.kind, entry.label));
    }
    if work.len() > MAX_NOTE_ENTRIES {
        lines.push(format!("- and {} more", work.len() - MAX_NOTE_ENTRIES));
    }
    lines.join("\n")
}
