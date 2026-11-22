//! Protocol v3 history retention, process-tree selection, and utility helpers.
//!
//! Mirrors the bounded history buffer, backward-clock trimming, start-time reuse
//! filtering, and UTF-8 truncation rules of `native/resource-monitor/src/main.rs`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::resource_monitor::ResourceSampleV3;

/// Protocol version supported by the v3 resource monitor engine.
pub const RESOURCE_MONITOR_PROTOCOL_VERSION: u32 = 3;
/// Minimum allowed periodic sampling interval in milliseconds.
pub const MIN_SAMPLE_INTERVAL_MS: u64 = 250;
/// Maximum allowed periodic sampling interval in milliseconds.
pub const MAX_SAMPLE_INTERVAL_MS: u64 = 60_000;
/// Start-time rounding precision used when matching external process identities.
pub const PROCESS_START_TIME_PRECISION_MS: u64 = 1_000;
/// Maximum retention window for buffered snapshot history (1 hour).
pub const HISTORY_RETENTION_MS: u64 = 60 * 60_000;
/// Maximum number of retained snapshots in memory.
pub const MAX_HISTORY_SNAPSHOTS: usize = 3_600;
/// Maximum retained process + external entries across history snapshots.
pub const MAX_HISTORY_RETAINED_ENTRIES: usize = 20_000;
/// Maximum estimated history memory footprint (64 MiB).
pub const MAX_HISTORY_RETAINED_BYTES: usize = 64 * 1024 * 1024;
/// Maximum number of snapshots emitted per `historyChunk` event.
pub const HISTORY_CHUNK_SNAPSHOTS: usize = 32;
/// Maximum UTF-8 byte length for a sampled process name.
pub const MAX_PROCESS_NAME_BYTES: usize = 1_024;
/// Maximum UTF-8 byte length for a sampled command line.
pub const MAX_PROCESS_COMMAND_BYTES: usize = 16 * 1_024;
/// Maximum UTF-8 byte length for a sampled status string.
pub const MAX_PROCESS_STATUS_BYTES: usize = 256;

/// Clamps `sample_interval_ms` to `[MIN_SAMPLE_INTERVAL_MS, MAX_SAMPLE_INTERVAL_MS]`, or `None` when `0`.
pub fn clamp_sample_interval(sample_interval_ms: u64) -> Option<Duration> {
    (sample_interval_ms > 0).then(|| {
        Duration::from_millis(
            sample_interval_ms.clamp(MIN_SAMPLE_INTERVAL_MS, MAX_SAMPLE_INTERVAL_MS),
        )
    })
}

/// Truncates `value` to at most `max_bytes` bytes on a valid UTF-8 codepoint boundary.
pub fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    value.truncate(boundary);
    value
}

/// Returns current Unix epoch time in milliseconds.
pub fn unix_time_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

/// Returns `true` when an external process's observed start time matches its expected identity.
pub fn matches_external_identity(
    actual_start_time_ms: u64,
    expected_start_time_ms: Option<u64>,
) -> bool {
    expected_start_time_ms.is_none_or(|expected| {
        actual_start_time_ms == expected - (expected % PROCESS_START_TIME_PRECISION_MS)
    })
}

/// Returns the number of tracked processes that could not be materialized.
pub fn inaccessible_process_count(selected: usize, materialized: usize) -> usize {
    selected.saturating_sub(materialized)
}

/// Selects `roots` and all descendant PIDs whose start time is `>=` their parent's start time.
///
/// Rejects older processes attached via PID reuse (`child_start_time_ms < parent_start_time_ms`).
pub fn select_tracked_pids<S: std::hash::BuildHasher>(
    rows: &[(u32, u32, u64)],
    roots: &HashSet<u32, S>,
) -> HashSet<u32> {
    let mut children_by_parent = HashMap::<u32, Vec<(u32, u64)>>::new();
    let mut start_time_by_pid = HashMap::<u32, u64>::new();
    for &(pid, ppid, start_time_ms) in rows {
        children_by_parent
            .entry(ppid)
            .or_default()
            .push((pid, start_time_ms));
        start_time_by_pid.insert(pid, start_time_ms);
    }

    let mut tracked = HashSet::new();
    let mut visited_identities = HashSet::new();
    let mut queue = roots
        .iter()
        .filter_map(|pid| {
            start_time_by_pid
                .get(pid)
                .map(|start_time_ms| (*pid, *start_time_ms))
        })
        .collect::<VecDeque<_>>();

    while let Some((pid, start_time_ms)) = queue.pop_front() {
        if !visited_identities.insert((pid, start_time_ms)) {
            continue;
        }
        tracked.insert(pid);
        if let Some(children) = children_by_parent.get(&pid) {
            queue.extend(
                children
                    .iter()
                    .copied()
                    .filter(|&(_, child_start)| child_start >= start_time_ms),
            );
        }
    }

    tracked
}

/// Bounded ring buffer of [`ResourceSampleV3`] history snapshots with clock-rollback protection.
#[derive(Debug, Default)]
pub struct HistoryRecorder {
    snapshots: VecDeque<ResourceSampleV3>,
    retained_entry_count: usize,
    retained_bytes: usize,
}

impl HistoryRecorder {
    /// Creates an empty [`HistoryRecorder`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of currently retained snapshots.
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Returns `true` when no snapshots are retained.
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// Returns the total retained entry count (process samples + external processes).
    pub fn retained_entry_count(&self) -> usize {
        self.retained_entry_count
    }

    /// Returns the estimated retained memory footprint in bytes.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// Records `snapshot` using the default protocol v3 limits, stripping `request_id`.
    pub fn record(&mut self, snapshot: &ResourceSampleV3) {
        self.record_with_limits(
            snapshot,
            MAX_HISTORY_SNAPSHOTS,
            MAX_HISTORY_RETAINED_ENTRIES,
            MAX_HISTORY_RETAINED_BYTES,
        );
    }

    /// Records `snapshot` while enforcing explicit snapshot count, entry count, and byte bounds.
    pub fn record_with_limits(
        &mut self,
        snapshot: &ResourceSampleV3,
        max_snapshots: usize,
        max_retained_entries: usize,
        max_retained_bytes: usize,
    ) {
        let clock_moved_backward = self
            .snapshots
            .back()
            .is_some_and(|previous| previous.sampled_at_unix_ms > snapshot.sampled_at_unix_ms);
        let mut retained = snapshot.clone();
        retained.request_id = None;
        self.retained_entry_count = self
            .retained_entry_count
            .saturating_add(retained.retained_entry_count());
        self.retained_bytes = self
            .retained_bytes
            .saturating_add(retained.estimated_history_bytes());
        self.snapshots.push_back(retained);
        self.trim_to_limits(
            snapshot.sampled_at_unix_ms,
            max_snapshots,
            max_retained_entries,
            max_retained_bytes,
            clock_moved_backward,
        );
    }

    fn trim_to_limits(
        &mut self,
        now_ms: u64,
        max_snapshots: usize,
        max_retained_entries: usize,
        max_retained_bytes: usize,
        clock_moved_backward: bool,
    ) {
        if clock_moved_backward {
            let mut future_entries = 0usize;
            let mut future_bytes = 0usize;
            self.snapshots.retain(|snapshot| {
                let keep = snapshot.sampled_at_unix_ms <= now_ms;
                if !keep {
                    future_entries = future_entries.saturating_add(snapshot.retained_entry_count());
                    future_bytes = future_bytes.saturating_add(snapshot.estimated_history_bytes());
                }
                keep
            });
            self.retained_entry_count = self.retained_entry_count.saturating_sub(future_entries);
            self.retained_bytes = self.retained_bytes.saturating_sub(future_bytes);
        }

        while self.snapshots.front().is_some_and(|snapshot| {
            snapshot.sampled_at_unix_ms < now_ms.saturating_sub(HISTORY_RETENTION_MS)
                || self.snapshots.len() > max_snapshots
                || self.retained_entry_count > max_retained_entries
                || self.retained_bytes > max_retained_bytes
        }) {
            if let Some(removed) = self.snapshots.pop_front() {
                self.retained_entry_count = self
                    .retained_entry_count
                    .saturating_sub(removed.retained_entry_count());
                self.retained_bytes = self
                    .retained_bytes
                    .saturating_sub(removed.estimated_history_bytes());
            }
        }
    }

    /// Reads snapshots falling inside `[now_ms - min(window_ms, 1h), now_ms]`.
    pub fn read(&self, window_ms: u64, now_ms: u64) -> Vec<ResourceSampleV3> {
        let started_at_ms = now_ms.saturating_sub(window_ms.min(HISTORY_RETENTION_MS));
        self.snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.sampled_at_unix_ms >= started_at_ms
                    && snapshot.sampled_at_unix_ms <= now_ms
            })
            .cloned()
            .collect()
    }
}
