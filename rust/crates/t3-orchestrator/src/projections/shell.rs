//! `ShellSnapshot` query with `shellThreadFamilies` and `shellOlderSettled` 25-thread cut.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use t3_contracts::ids::{ProjectId, ThreadId};
use t3_contracts::orchestration::{AppThread, Run, RunStatus};

use crate::projections::ProjectionError;
use crate::projections::shell_focus::{
    apply_older_settled_cut, build_thread_families, collect_parent_ids, filter_idle_subagents,
};
pub use crate::projections::shell_types::{
    OlderSettledProjectCount, OlderSettledSummary, SHELL_NEWEST_SETTLED_KEPT, ShellSnapshot,
    ShellSnapshotOptions, ShellThread, ShellThreadFamily, ShellThreadStatus,
};
use crate::time::parse_iso_millis;

/// Builds a [`ShellSnapshot`] from V2 `SQLite` projection tables at `now_iso`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_orchestrator::projections::{ShellSnapshotOptions, query_shell_snapshot};
/// use t3_persistence::sqlite::SqliteStore;
///
/// let store = SqliteStore::open_in_memory()?;
/// let snap = query_shell_snapshot(
///     store.connection(),
///     "2026-11-20T00:00:00.000Z",
///     ShellSnapshotOptions::default(),
/// )?;
/// assert!(snap.threads.is_empty());
/// assert_eq!(snap.shell_older_settled.newest_settled_kept, 25);
/// # Ok(())
/// # }
/// ```
pub fn query_shell_snapshot(
    conn: &Connection,
    now_iso: &str,
    options: ShellSnapshotOptions,
) -> Result<ShellSnapshot, ProjectionError> {
    let snapshot_sequence = load_projection_sequence(conn);
    let all_rows = load_all_shell_threads(conn, now_iso, options.include_archived)?;
    let (after_subagents, ancestor_ids) = if options.omit_idle_subagents {
        filter_idle_subagents(&all_rows)
    } else {
        let ancestors = collect_parent_ids(&all_rows);
        (all_rows, ancestors)
    };

    let (kept_threads, older_summary) = if options.omit_older_settled {
        apply_older_settled_cut(&after_subagents, &ancestor_ids)
    } else {
        (
            after_subagents,
            OlderSettledSummary {
                newest_settled_kept: SHELL_NEWEST_SETTLED_KEPT,
                ..OlderSettledSummary::default()
            },
        )
    };

    let families = build_thread_families(&kept_threads);
    Ok(ShellSnapshot {
        snapshot_sequence,
        threads: kept_threads,
        shell_thread_families: families,
        shell_older_settled: older_summary,
    })
}

/// Lists omitted settled threads (older than the 25-thread cut), optionally filtered by project.
pub fn list_older_settled_threads(
    conn: &Connection,
    project_id: Option<&ProjectId>,
    now_iso: &str,
) -> Result<Vec<ShellThread>, ProjectionError> {
    let snap = query_shell_snapshot(conn, now_iso, ShellSnapshotOptions::default())?;
    let omitted_set: HashSet<ThreadId> = snap
        .shell_older_settled
        .older_thread_ids
        .into_iter()
        .collect();
    let all_rows = load_all_shell_threads(conn, now_iso, false)?;
    Ok(all_rows
        .into_iter()
        .filter(|row| omitted_set.contains(&row.thread.id))
        .filter(|row| project_id.is_none_or(|pid| &row.thread.project_id == pid))
        .collect())
}

fn load_projection_sequence(conn: &Connection) -> i64 {
    let seq: Option<i64> = conn
        .query_row(
            "SELECT last_sequence FROM orchestration_v2_projection_metadata
             WHERE projection_name = 'thread-projections' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();
    seq.unwrap_or(0)
}

fn load_all_shell_threads(
    conn: &Connection,
    now_iso: &str,
    include_archived: bool,
) -> Result<Vec<ShellThread>, ProjectionError> {
    let now_ms = parse_iso_millis(now_iso);
    let mut run_stmt = conn.prepare(
        "SELECT payload_json FROM orchestration_v2_projection_runs ORDER BY thread_id ASC, ordinal ASC",
    )?;
    let run_jsons = run_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut runs_by_thread: HashMap<ThreadId, Vec<Run>> = HashMap::new();
    for raw in run_jsons {
        let run: Run =
            serde_json::from_str(&raw).map_err(|err| ProjectionError::Codec(err.to_string()))?;
        runs_by_thread
            .entry(run.thread_id.clone())
            .or_default()
            .push(run);
    }

    let sql = if include_archived {
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL"
    } else {
        "SELECT payload_json FROM orchestration_v2_projection_threads
         WHERE deleted_at IS NULL AND archived_at IS NULL"
    };
    let mut thread_stmt = conn.prepare(sql)?;
    let thread_jsons = thread_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::with_capacity(thread_jsons.len());
    for raw in thread_jsons {
        let thread: AppThread =
            serde_json::from_str(&raw).map_err(|err| ProjectionError::Codec(err.to_string()))?;
        let runs = runs_by_thread.remove(&thread.id).unwrap_or_default();
        let has_active_run = runs.iter().any(is_active_run);
        let latest_run = runs.into_iter().last();
        let pinned = thread.pinned_at.as_ref().and_then(|o| o.as_ref()).is_some();
        let future_snoozed = thread
            .snoozed_until
            .as_ref()
            .and_then(|o| o.as_ref())
            .and_then(|ts| Some((parse_iso_millis(ts)?, now_ms?)))
            .is_some_and(|(until_ms, cur_ms)| until_ms > cur_ms);
        let explicitly_unsettled = thread.settled_override.as_deref() == Some("unsettled");
        let settled = thread.settled_override.as_deref() == Some("settled")
            || thread.settled_at.is_some()
            || (!explicitly_unsettled
                && !has_active_run
                && latest_run.as_ref().is_some_and(|r| !is_active_run(r))
                && thread
                    .unsettled_at
                    .as_ref()
                    .and_then(|o| o.as_ref())
                    .is_none());
        let settled_sort_timestamp = if settled {
            Some(
                thread
                    .settled_at
                    .clone()
                    .or_else(|| latest_run.as_ref().and_then(|r| r.completed_at.clone()))
                    .unwrap_or_else(|| thread.updated_at.clone()),
            )
        } else {
            None
        };
        let status = if has_active_run {
            ShellThreadStatus::Active
        } else if settled {
            ShellThreadStatus::Settled
        } else {
            ShellThreadStatus::Unsettled
        };
        out.push(ShellThread {
            thread,
            latest_run,
            status,
            pinned,
            future_snoozed,
            settled_sort_timestamp,
        });
    }
    out.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.thread.updated_at.cmp(&a.thread.updated_at))
            .then_with(|| a.thread.id.cmp(&b.thread.id))
    });
    Ok(out)
}

fn is_active_run(run: &Run) -> bool {
    matches!(
        run.status,
        RunStatus::Preparing
            | RunStatus::Queued
            | RunStatus::Starting
            | RunStatus::Running
            | RunStatus::Waiting
    )
}
