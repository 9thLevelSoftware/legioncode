//! Date-windowed skipped/malformed usage record tracking, scan-cache migration rules,
//! and outside-window duplicate retention (`#41157`).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Query window used when scoping skipped/malformed record diagnostics and unkeyed usage (`#41157`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageDateWindow {
    /// Earliest inclusive epoch ms for hourly queries (`None` for day queries).
    pub since_time_ms: Option<i64>,
    /// Exclusive upper bound epoch ms for hourly queries (`None` for day queries).
    pub until_time_ms: Option<i64>,
    /// Inclusive start day (`YYYY-MM-DD`).
    pub since_day: String,
    /// Inclusive end day (`YYYY-MM-DD`).
    pub until_day: String,
}

/// Evaluates whether `timestamp_ms` falls within `window`, resolving its local calendar day
/// through `day_of` (`YYYY-MM-DD`) for day-level queries (`#41157`).
#[must_use]
pub fn usage_timestamp_in_window(
    timestamp_ms: i64,
    window: &UsageDateWindow,
    day_of: impl FnOnce(i64) -> String,
) -> bool {
    if let (Some(since_ms), Some(until_ms)) = (window.since_time_ms, window.until_time_ms) {
        return timestamp_ms >= since_ms && timestamp_ms < until_ms;
    }
    let day = day_of(timestamp_ms);
    day >= window.since_day && day <= window.until_day
}

/// Per-file skipped/malformed record timestamps paired with legacy scalar count (`#41157`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MalformedRecordDiagnostics {
    /// Legacy whole-file count kept in cache (`mr`).
    pub count: usize,
    /// Per-skipped-record event timestamps in epoch ms (`mt`).
    pub timestamps_ms: Vec<i64>,
}

impl MalformedRecordDiagnostics {
    /// Records `repeat` malformed records occurring at `timestamp_ms` (`#41157`).
    pub fn push(&mut self, timestamp_ms: i64, repeat: usize) {
        self.count = self.count.saturating_add(repeat);
        for _ in 0..repeat {
            self.timestamps_ms.push(timestamp_ms);
        }
    }

    /// Counts only those skipped/malformed records whose timestamp falls in `window` (`#41157`).
    #[must_use]
    pub fn count_in_window(
        &self,
        window: &UsageDateWindow,
        mut day_of: impl FnMut(i64) -> String,
    ) -> usize {
        self.timestamps_ms
            .iter()
            .filter(|&&ts| usage_timestamp_in_window(ts, window, &mut day_of))
            .count()
    }
}

/// Scan-cache file summary handling backward-compatible `mr` / `mt` migration (`#41157`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedScanCacheDiagnostics {
    /// Validated per-record timestamps (empty when `undated`).
    pub malformed_timestamps_ms: Vec<i64>,
    /// True when legacy cache stored `mr > 0` without `mt`, forcing a full non-resumable re-parse.
    pub requires_full_reparse: bool,
}

/// Validates serialized cache `mr` + optional `mt` fields (`#41157`).
///
/// Returns `None` when counts mismatch or invalid; returns `requires_full_reparse: true`
/// when a non-zero `mr` lacks `mt` timestamps so an upgrade re-parses once to gather dates.
#[must_use]
pub fn decode_cached_malformed_diagnostics(
    malformed_count: usize,
    malformed_timestamps_ms: Option<&[i64]>,
) -> Option<DecodedScanCacheDiagnostics> {
    if malformed_count == 0 {
        if let Some(ts) = malformed_timestamps_ms
            && !ts.is_empty()
        {
            return None;
        }
        return Some(DecodedScanCacheDiagnostics {
            malformed_timestamps_ms: Vec::new(),
            requires_full_reparse: false,
        });
    }
    let Some(timestamps) = malformed_timestamps_ms else {
        return Some(DecodedScanCacheDiagnostics {
            malformed_timestamps_ms: Vec::new(),
            requires_full_reparse: true,
        });
    };
    if timestamps.len() != malformed_count {
        return None;
    }
    Some(DecodedScanCacheDiagnostics {
        malformed_timestamps_ms: timestamps.to_vec(),
        requires_full_reparse: false,
    })
}

/// Usage record summary used when filtering cached files against a date window (`#41157`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowedUsageRecordRef<T> {
    /// Non-empty dedupe key (`Some(...)`) when duplicates across transcript copies must still be claimed.
    pub dedupe_key: Option<String>,
    /// Record timestamp in epoch milliseconds.
    pub timestamp_ms: i64,
    /// Caller payload.
    pub payload: T,
}

/// Retained records and scoped warning count for a cached transcript file (`#41157`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowedUsageFile<T> {
    /// Kept records (in-window records plus outside-window keyed records for deduplication).
    pub records: Vec<WindowedUsageRecordRef<T>>,
    /// Malformed record timestamps falling inside the requested date window.
    pub malformed_timestamps_ms: Vec<i64>,
}

/// Filters cached transcript records for `window` while keeping outside-window keyed copies
/// so a subsequent transcript copy does not double-count (`#41157`).
#[must_use]
pub fn filter_cached_file_for_window<T>(
    records: Vec<WindowedUsageRecordRef<T>>,
    malformed_timestamps_ms: &[i64],
    window: &UsageDateWindow,
    mut day_of: impl FnMut(i64) -> String,
) -> Option<WindowedUsageFile<T>> {
    let kept_records: Vec<WindowedUsageRecordRef<T>> = records
        .into_iter()
        .filter(|rec| {
            rec.dedupe_key.is_some()
                || usage_timestamp_in_window(rec.timestamp_ms, window, &mut day_of)
        })
        .collect();
    let kept_malformed: Vec<i64> = malformed_timestamps_ms
        .iter()
        .copied()
        .filter(|&ts| usage_timestamp_in_window(ts, window, &mut day_of))
        .collect();
    if kept_records.is_empty() && kept_malformed.is_empty() {
        None
    } else {
        Some(WindowedUsageFile {
            records: kept_records,
            malformed_timestamps_ms: kept_malformed,
        })
    }
}

/// Deduplicates `records` across transcript files in walk order while dropping outside-window
/// records ONLY after recording their `dedupe_key` (`#41157`).
#[must_use]
pub fn collect_windowed_deduped_usage<T>(
    files: impl IntoIterator<Item = WindowedUsageFile<T>>,
    window: &UsageDateWindow,
    mut day_of: impl FnMut(i64) -> String,
) -> (Vec<T>, usize) {
    let mut seen_keys = HashSet::new();
    let mut kept = Vec::new();
    let mut skipped_warnings = 0usize;

    for file in files {
        skipped_warnings = skipped_warnings.saturating_add(file.malformed_timestamps_ms.len());
        for rec in file.records {
            if let Some(key) = rec.dedupe_key
                && !seen_keys.insert(key)
            {
                continue;
            }
            if usage_timestamp_in_window(rec.timestamp_ms, window, &mut day_of) {
                kept.push(rec.payload);
            }
        }
    }

    (kept, skipped_warnings)
}
