//! Wire shell trimming filters (`omitIdleSubagents` and 25-thread `omitOlderSettled` cut).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::projections::shell_types::SHELL_NEWEST_SETTLED_KEPT;
use crate::projections::wire_types::{
    WireOlderSettledCount, WireOlderSettledThreads, WireShellOptions, WireThreadShell,
};
use crate::time::{format_millis_iso, parse_iso_millis};

pub(crate) fn trim_wire_shell_threads(
    threads: Vec<WireThreadShell>,
    options: WireShellOptions,
    snapshot_sequence: i64,
    now_iso: &str,
) -> (Vec<WireThreadShell>, Option<WireOlderSettledThreads>) {
    let mut subagent_parent_ids = HashSet::<String>::new();
    let mut settled_times = Vec::<i64>::new();
    for thread in &threads {
        if is_subagent(thread) {
            if let Some(parent) = lineage_parent_id(thread) {
                subagent_parent_ids.insert(parent.to_owned());
            }
        } else if is_listed_lineage(thread)
            && thread.settled_override.as_deref() == Some("settled")
            && let Some(ms) = thread.settled_at.as_deref().and_then(parse_iso_millis)
        {
            settled_times.push(ms);
        }
    }
    settled_times.sort_by(|a, b| b.cmp(a));
    let older_settled_before_ms = settled_times
        .get(SHELL_NEWEST_SETTLED_KEPT.saturating_sub(1))
        .copied()
        .or_else(|| settled_times.last().copied());

    let after_subagents: Vec<WireThreadShell> = if options.omit_idle_subagents {
        threads
            .into_iter()
            .filter(|t| {
                !is_subagent(t)
                    || is_live_subagent(t)
                    || subagent_parent_ids.contains(&t.id)
                    || has_remote_child_tasks(t)
            })
            .collect()
    } else {
        threads
    };

    if !options.omit_older_settled {
        return (after_subagents, None);
    }

    let now_ms = parse_iso_millis(now_iso).unwrap_or(0);
    let before_ms = older_settled_before_ms.unwrap_or(now_ms);
    let mut count_map: HashMap<String, u64> = HashMap::new();
    if let Some(cut) = older_settled_before_ms {
        for thread in &after_subagents {
            if is_older_settled_thread(thread, cut) {
                *count_map.entry(thread.project_id.clone()).or_insert(0) += 1;
            }
        }
    }
    let ancestor_ids = hidden_child_ancestor_ids(&after_subagents);
    let kept = after_subagents
        .into_iter()
        .filter(|t| {
            !older_settled_before_ms.is_some_and(|cut| is_older_settled_thread(t, cut))
                || t.pinned_at.is_some()
                || t.snoozed_until
                    .as_deref()
                    .and_then(parse_iso_millis)
                    .is_some_and(|until| until > now_ms)
                || is_live_subagent(t)
                || ancestor_ids.contains(&t.id)
        })
        .collect();
    let mut counts: Vec<WireOlderSettledCount> = count_map
        .into_iter()
        .map(|(project_id, count)| WireOlderSettledCount { project_id, count })
        .collect();
    counts.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    let summary = WireOlderSettledThreads {
        before: format_millis_iso(before_ms),
        as_of_sequence: snapshot_sequence,
        counts,
    };
    (kept, Some(summary))
}

fn is_subagent(t: &WireThreadShell) -> bool {
    t.lineage
        .get("relationshipToParent")
        .and_then(Value::as_str)
        == Some("subagent")
}

fn is_listed_lineage(t: &WireThreadShell) -> bool {
    !is_subagent(t)
        && t.lineage.get("sideChat").and_then(Value::as_bool) != Some(true)
        && t.lineage.get("workflowStep").and_then(Value::as_bool) != Some(true)
        && t.workflow_run.as_ref().is_none_or(Value::is_null)
}

fn lineage_parent_id(t: &WireThreadShell) -> Option<&str> {
    t.lineage.get("parentThreadId").and_then(Value::as_str)
}

fn is_live_subagent(t: &WireThreadShell) -> bool {
    t.activity_run_status.is_some()
        || matches!(
            t.status.as_str(),
            "queued" | "preparing" | "starting" | "running" | "waiting"
        )
        || t.pending_runtime_request.is_some()
        || !t.pending_background_tasks.is_empty()
}

fn has_remote_child_tasks(_t: &WireThreadShell) -> bool {
    false
}

fn is_older_settled_thread(t: &WireThreadShell, before_ms: i64) -> bool {
    is_listed_lineage(t)
        && t.settled_override.as_deref() == Some("settled")
        && t.settled_at
            .as_deref()
            .and_then(parse_iso_millis)
            .is_some_and(|ms| ms < before_ms)
}

fn hidden_child_ancestor_ids(threads: &[WireThreadShell]) -> HashSet<String> {
    let mut set = HashSet::new();
    for t in threads {
        if is_listed_lineage(t) {
            continue;
        }
        if let Some(p) = lineage_parent_id(t) {
            set.insert(p.to_owned());
        }
        if let Some(root) = t.lineage.get("rootThreadId").and_then(Value::as_str) {
            set.insert(root.to_owned());
        }
    }
    set
}
