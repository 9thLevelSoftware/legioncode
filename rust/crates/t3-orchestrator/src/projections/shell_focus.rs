//! Focus-window filtering, older-settled 25-thread cut, and parent-child family assembly
//! for [`super::shell::query_shell_snapshot`].

use std::collections::{BTreeMap, HashMap, HashSet};

use t3_contracts::ids::{ProjectId, ThreadId};

use crate::projections::shell_types::{
    OlderSettledProjectCount, OlderSettledSummary, SHELL_NEWEST_SETTLED_KEPT, ShellThread,
    ShellThreadFamily,
};

pub(super) fn filter_idle_subagents(rows: &[ShellThread]) -> (Vec<ShellThread>, HashSet<ThreadId>) {
    let by_id: HashMap<&ThreadId, &ShellThread> = rows.iter().map(|r| (&r.thread.id, r)).collect();
    let mut keep_ids: HashSet<ThreadId> = HashSet::new();
    let mut ancestor_ids: HashSet<ThreadId> = HashSet::new();

    for row in rows {
        let is_subagent = row
            .thread
            .lineage
            .parent_thread_id
            .as_ref()
            .is_some_and(|pid| by_id.contains_key(pid));
        if !is_subagent || row.has_active_run() {
            keep_ids.insert(row.thread.id.clone());
            if is_subagent {
                let mut cur = row.thread.lineage.parent_thread_id.as_ref();
                while let Some(pid) = cur {
                    if !ancestor_ids.insert(pid.clone()) {
                        break;
                    }
                    keep_ids.insert(pid.clone());
                    cur = by_id
                        .get(pid)
                        .and_then(|p| p.thread.lineage.parent_thread_id.as_ref());
                }
            }
        }
    }
    let filtered = rows
        .iter()
        .filter(|r| keep_ids.contains(&r.thread.id))
        .cloned()
        .collect();
    (filtered, ancestor_ids)
}

pub(super) fn collect_parent_ids(rows: &[ShellThread]) -> HashSet<ThreadId> {
    rows.iter()
        .filter_map(|r| r.thread.lineage.parent_thread_id.clone())
        .collect()
}

pub(super) fn apply_older_settled_cut(
    rows: &[ShellThread],
    ancestor_ids: &HashSet<ThreadId>,
) -> (Vec<ShellThread>, OlderSettledSummary) {
    let mut settled_candidates: Vec<&ShellThread> =
        rows.iter().filter(|r| r.is_settled()).collect();
    settled_candidates.sort_by(|a, b| {
        b.settled_sort_timestamp
            .cmp(&a.settled_sort_timestamp)
            .then_with(|| b.thread.updated_at.cmp(&a.thread.updated_at))
            .then_with(|| a.thread.id.cmp(&b.thread.id))
    });

    if settled_candidates.len() <= SHELL_NEWEST_SETTLED_KEPT {
        return (
            rows.to_vec(),
            OlderSettledSummary {
                newest_settled_kept: SHELL_NEWEST_SETTLED_KEPT,
                ..OlderSettledSummary::default()
            },
        );
    }

    let cutoff_ts = settled_candidates[SHELL_NEWEST_SETTLED_KEPT - 1]
        .settled_sort_timestamp
        .as_deref()
        .unwrap_or("");
    let mut omitted_ids: HashSet<ThreadId> = HashSet::new();
    let mut older_thread_ids: Vec<ThreadId> = Vec::new();
    let mut by_project: BTreeMap<ProjectId, (usize, usize)> = BTreeMap::new();

    for row in &settled_candidates[SHELL_NEWEST_SETTLED_KEPT..] {
        let ts = row.settled_sort_timestamp.as_deref().unwrap_or("");
        if ts >= cutoff_ts {
            continue;
        }
        let exempt = row.pinned
            || row.future_snoozed
            || row.has_active_run()
            || ancestor_ids.contains(&row.thread.id);
        let counts = by_project
            .entry(row.thread.project_id.clone())
            .or_insert((0, 0));
        counts.1 += 1;
        if !exempt {
            counts.0 += 1;
            omitted_ids.insert(row.thread.id.clone());
            older_thread_ids.push(row.thread.id.clone());
        }
    }

    let kept: Vec<ShellThread> = rows
        .iter()
        .filter(|r| !omitted_ids.contains(&r.thread.id))
        .cloned()
        .collect();
    let total_omitted_count = omitted_ids.len();
    let total_older_settled_count: usize = by_project.values().map(|(_, total)| *total).sum();
    let project_counts = by_project
        .into_iter()
        .map(
            |(project_id, (omitted_count, total_older_count))| OlderSettledProjectCount {
                project_id,
                omitted_count,
                total_older_count,
                has_more: omitted_count > 0,
            },
        )
        .collect();

    (
        kept,
        OlderSettledSummary {
            newest_settled_kept: SHELL_NEWEST_SETTLED_KEPT,
            total_omitted_count,
            total_older_settled_count,
            by_project: project_counts,
            older_thread_ids,
        },
    )
}

pub(super) fn build_thread_families(kept: &[ShellThread]) -> Vec<ShellThreadFamily> {
    let kept_ids: HashSet<&ThreadId> = kept.iter().map(|r| &r.thread.id).collect();
    let mut children_by_root: HashMap<ThreadId, Vec<ShellThread>> = HashMap::new();
    let mut roots: Vec<ShellThread> = Vec::new();

    for row in kept {
        let parent = row.thread.lineage.parent_thread_id.as_ref();
        if let Some(pid) = parent
            && kept_ids.contains(pid)
        {
            children_by_root
                .entry(row.thread.lineage.root_thread_id.clone())
                .or_default()
                .push(row.clone());
        } else {
            roots.push(row.clone());
        }
    }

    roots
        .into_iter()
        .map(|root| {
            let mut children = children_by_root.remove(&root.thread.id).unwrap_or_default();
            children.sort_by(|a, b| {
                a.thread
                    .created_at
                    .cmp(&b.thread.created_at)
                    .then_with(|| a.thread.id.cmp(&b.thread.id))
            });
            ShellThreadFamily { root, children }
        })
        .collect()
}
