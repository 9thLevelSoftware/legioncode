//! Data types and configuration options for [`crate::projections::shell`].

use legion_contracts::ids::{ProjectId, ThreadId};
use legion_contracts::orchestration::{AppThread, Run};
use serde::{Deserialize, Serialize};

/// Maximum number of newest settled threads kept in the primary shell sidebar before omission.
pub const SHELL_NEWEST_SETTLED_KEPT: usize = 25;

/// Options controlling [`crate::projections::query_shell_snapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellSnapshotOptions {
    /// Omit non-live descendant subagents unless they have live descendants. Defaults to `true`.
    pub omit_idle_subagents: bool,
    /// Omit settled threads older than the 25 newest settled cut (except pinned/snoozed/live/ancestors). Defaults to `true`.
    pub omit_older_settled: bool,
    /// Include archived threads. Defaults to `false`.
    pub include_archived: bool,
}

impl Default for ShellSnapshotOptions {
    fn default() -> Self {
        Self {
            omit_idle_subagents: true,
            omit_older_settled: true,
            include_archived: false,
        }
    }
}

/// Activity/settlement status of a projected thread in [`ShellSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellThreadStatus {
    /// Thread currently has an active (`preparing|queued|starting|running|waiting`) run.
    Active,
    /// Thread is idle/open (`unsettled` or newly created).
    Unsettled,
    /// Thread is settled (`settled_override = "settled"` or terminal latest run).
    Settled,
}

/// Enriched thread projection entry included in [`ShellSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellThread {
    /// Materialized thread record.
    pub thread: AppThread,
    /// Latest run by `ordinal`, if any.
    pub latest_run: Option<Run>,
    /// Effective lifecycle status (`Active`, `Unsettled`, or `Settled`).
    pub status: ShellThreadStatus,
    /// True when pinned (`pinned_at` set).
    pub pinned: bool,
    /// True when snoozed strictly in the future relative to `now_iso`.
    pub future_snoozed: bool,
    /// Effective timestamp used when ranking settled recency.
    pub settled_sort_timestamp: Option<String>,
}

impl ShellThread {
    /// Returns `true` when this thread currently has an active run.
    pub const fn has_active_run(&self) -> bool {
        matches!(self.status, ShellThreadStatus::Active)
    }

    /// Returns `true` when this thread is settled.
    pub const fn is_settled(&self) -> bool {
        matches!(self.status, ShellThreadStatus::Settled)
    }
}

/// Root thread paired with its retained subagent children (`shellThreadFamilies`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellThreadFamily {
    /// Family root thread (or orphan top-level entry).
    pub root: ShellThread,
    /// Retained descendant child subagents belonging to `root`.
    pub children: Vec<ShellThread>,
}

/// Per-project older settled omission counters (`shellOlderSettled` breakdown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OlderSettledProjectCount {
    /// Project ID.
    pub project_id: ProjectId,
    /// Count of threads omitted from the active shell sidebar for this project.
    pub omitted_count: usize,
    /// Total older settled threads below the 25-thread cutoff (including kept pinned/snoozed/live/ancestors).
    pub total_older_count: usize,
    /// True if at least one older settled thread was omitted for this project.
    pub has_more: bool,
}

/// Summary of the `shellOlderSettled` 25-thread cut (`ShellStream.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OlderSettledSummary {
    /// Cutoff window size (`SHELL_NEWEST_SETTLED_KEPT = 25`).
    pub newest_settled_kept: usize,
    /// Total threads omitted across all projects.
    pub total_omitted_count: usize,
    /// Total threads below the 25-thread cutoff across all projects (including kept exceptions).
    pub total_older_settled_count: usize,
    /// Per-project counts ordered by `project_id`.
    pub by_project: Vec<OlderSettledProjectCount>,
    /// IDs of omitted older settled threads (newest-first).
    pub older_thread_ids: Vec<ThreadId>,
}

/// Materialized sidebar snapshot produced by [`crate::projections::query_shell_snapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSnapshot {
    /// Last projected `orchestration_events.sequence` recorded in `thread-projections`.
    pub snapshot_sequence: i64,
    /// Retained flat list of shell threads after subagent and 25-settled filtering.
    pub threads: Vec<ShellThread>,
    /// Grouped `shellThreadFamilies` preserving parent/child subagent relationships.
    pub shell_thread_families: Vec<ShellThreadFamily>,
    /// `shellOlderSettled` cut statistics and per-project counts.
    pub shell_older_settled: OlderSettledSummary,
}
