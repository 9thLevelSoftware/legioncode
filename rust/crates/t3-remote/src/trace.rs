//! Streaming OTLP NDJSON trace reader and span summary aggregator.
//!
//! Mirrors the rotated file discovery and span quantile/rate aggregation from
//! `apps/server/src/diagnostics/TraceDiagnostics.ts` and `apps/server/src/cli/trace.ts`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::trace_format::{
    format_duration_ms, format_empty_trace_message, format_trace_summary,
};
use crate::trace_parse::parse_trace_span_line;

/// Default maximum number of rotated backup trace files checked alongside the active trace file.
pub const DEFAULT_TRACE_MAX_FILES: usize = 5;

/// Errors returned while reading local trace NDJSON files.
#[derive(Debug, Error)]
pub enum TraceError {
    /// Failed to open or read a trace file on disk.
    #[error("failed to read trace file `{path}`: {source}")]
    Io {
        /// Path of the trace file being read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Aggregated statistics for a single span name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceSpanStats {
    /// Span operation name.
    pub name: String,
    /// Total number of ingested spans with this name inside the window.
    pub count: usize,
    /// Average invocations per minute across the observed trace window (`None` when span width is `0ms`).
    pub per_minute: Option<f64>,
    /// 50th percentile duration in milliseconds.
    pub p50_ms: f64,
    /// 90th percentile duration in milliseconds.
    pub p90_ms: f64,
    /// Maximum duration in milliseconds.
    pub max_ms: f64,
    /// Number of spans tagged as interrupted.
    pub interrupted_count: usize,
    /// Number of spans tagged as failed.
    pub failure_count: usize,
}

/// Summary produced from streaming one or more NDJSON trace files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceSummary {
    /// Total number of valid spans matching the optional `since_ms` filter.
    pub span_count: usize,
    /// Number of non-empty lines that failed span schema validation.
    pub skipped_line_count: usize,
    /// Earliest `endTimeUnixNano` (in milliseconds) observed across counted spans.
    pub first_end_ms: Option<i64>,
    /// Latest `endTimeUnixNano` (in milliseconds) observed across counted spans.
    pub last_end_ms: Option<i64>,
    /// Duration between `first_end_ms` and `last_end_ms` in minutes (`0.0` when `< 2` timestamps differ).
    pub minutes: f64,
    /// Per-span aggregations sorted by `count` descending, then `name` ascending.
    pub spans: Vec<TraceSpanStats>,
}

/// Returns candidate rotated trace file paths ordered oldest-to-newest ending at `trace_file_path`.
///
/// ```
/// use std::path::Path;
/// use t3_remote::trace::to_rotated_trace_paths;
///
/// let paths = to_rotated_trace_paths(Path::new("/tmp/trace.ndjson"), 2);
/// assert_eq!(paths.len(), 3);
/// assert!(paths[0].ends_with("trace.ndjson.2"));
/// assert!(paths[1].ends_with("trace.ndjson.1"));
/// assert!(paths[2].ends_with("trace.ndjson"));
/// ```
#[must_use]
pub fn to_rotated_trace_paths(trace_file_path: &Path, max_files: usize) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(max_files.saturating_add(1));
    for index in (1..=max_files).rev() {
        let mut os = trace_file_path.as_os_str().to_os_string();
        os.push(format!(".{index}"));
        paths.push(PathBuf::from(os));
    }
    paths.push(trace_file_path.to_path_buf());
    paths
}

#[derive(Debug, Default)]
struct SpanBucket {
    durations_ms: Vec<f64>,
    interrupted_count: usize,
    failure_count: usize,
}

/// Incremental aggregator that reads NDJSON lines without loading whole trace files into memory.
#[derive(Debug, Default)]
pub struct TraceSpanSummaryBuilder {
    since_ms: Option<i64>,
    span_count: usize,
    skipped_line_count: usize,
    first_end_ms: Option<i64>,
    last_end_ms: Option<i64>,
    buckets: BTreeMap<String, SpanBucket>,
}

impl TraceSpanSummaryBuilder {
    /// Creates an empty builder filtering out spans strictly older than `since_ms` when provided.
    #[must_use]
    pub fn new(since_ms: Option<i64>) -> Self {
        Self {
            since_ms,
            ..Self::default()
        }
    }

    /// Ingests one line from an OTLP NDJSON trace file.
    ///
    /// Empty or whitespace-only lines are ignored without incrementing [`TraceSummary::skipped_line_count`].
    pub fn ingest_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let Some(parsed) = parse_trace_span_line(line) else {
            self.skipped_line_count = self.skipped_line_count.saturating_add(1);
            return;
        };
        if self.since_ms.is_some_and(|since| parsed.end_ms < since) {
            return;
        }

        self.span_count = self.span_count.saturating_add(1);
        self.first_end_ms = Some(
            self.first_end_ms
                .map_or(parsed.end_ms, |prev| prev.min(parsed.end_ms)),
        );
        self.last_end_ms = Some(
            self.last_end_ms
                .map_or(parsed.end_ms, |prev| prev.max(parsed.end_ms)),
        );

        let bucket = self.buckets.entry(parsed.name).or_default();
        bucket.durations_ms.push(parsed.duration_ms);
        if parsed.interrupted {
            bucket.interrupted_count = bucket.interrupted_count.saturating_add(1);
        }
        if parsed.failed {
            bucket.failure_count = bucket.failure_count.saturating_add(1);
        }
    }

    /// Streams lines from any [`BufRead`] source into the summary builder.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::Io`] if reading from `reader` fails.
    pub fn ingest_reader<R: BufRead>(
        &mut self,
        mut reader: R,
        source_path: &Path,
    ) -> Result<(), TraceError> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|source| TraceError::Io {
                    path: source_path.to_path_buf(),
                    source,
                })?;
            if bytes == 0 {
                break;
            }
            self.ingest_line(&line);
        }
        Ok(())
    }

    /// Streams lines from `path` when the file exists.
    ///
    /// Returns `Ok(false)` when `path` does not exist instead of failing.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::Io`] for I/O errors other than [`std::io::ErrorKind::NotFound`].
    pub fn ingest_file(&mut self, path: &Path) -> Result<bool, TraceError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(TraceError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let reader = BufReader::new(file);
        self.ingest_reader(reader, path)?;
        Ok(true)
    }

    /// Streams all existing rotated files for `trace_file_path` (up to `max_files` backups) oldest-first.
    ///
    /// Returns the paths that existed and were read.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::Io`] if an existing rotated file fails to open or read.
    pub fn ingest_rotated_files(
        &mut self,
        trace_file_path: &Path,
        max_files: usize,
    ) -> Result<Vec<PathBuf>, TraceError> {
        let mut read_paths = Vec::new();
        for path in to_rotated_trace_paths(trace_file_path, max_files) {
            if self.ingest_file(&path)? {
                read_paths.push(path);
            }
        }
        Ok(read_paths)
    }

    /// Finalizes quantile calculations and builds the sorted [`TraceSummary`].
    #[must_use]
    pub fn build(self) -> TraceSummary {
        let minutes = match (self.first_end_ms, self.last_end_ms) {
            (Some(first), Some(last)) if last > first => {
                nonneg_i128_to_f64(i128::from(last) - i128::from(first)) / 60_000.0
            }
            _ => 0.0,
        };

        let mut spans: Vec<TraceSpanStats> = self
            .buckets
            .into_iter()
            .filter_map(|(name, mut bucket)| {
                let count = bucket.durations_ms.len();
                if count == 0 {
                    return None;
                }
                bucket.durations_ms.sort_by(f64::total_cmp);
                let p50_idx = (count.saturating_mul(50).div_ceil(100)).saturating_sub(1);
                let p90_idx = (count.saturating_mul(90).div_ceil(100)).saturating_sub(1);
                let p50_ms = bucket.durations_ms[p50_idx.min(count - 1)];
                let p90_ms = bucket.durations_ms[p90_idx.min(count - 1)];
                let max_ms = bucket.durations_ms[count - 1];
                let per_minute = if minutes > 0.0 {
                    Some(usize_to_f64(count) / minutes)
                } else {
                    None
                };

                Some(TraceSpanStats {
                    name,
                    count,
                    per_minute,
                    p50_ms,
                    p90_ms,
                    max_ms,
                    interrupted_count: bucket.interrupted_count,
                    failure_count: bucket.failure_count,
                })
            })
            .collect();

        spans.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.name.cmp(&right.name))
        });

        TraceSummary {
            span_count: self.span_count,
            skipped_line_count: self.skipped_line_count,
            first_end_ms: self.first_end_ms,
            last_end_ms: self.last_end_ms,
            minutes,
            spans,
        }
    }
}

/// Convenience helper that streams `trace_file_path` and up to `max_files` backups into a [`TraceSummary`].
///
/// # Errors
///
/// Returns [`TraceError::Io`] if reading an existing file fails.
pub fn summarize_trace_files(
    trace_file_path: &Path,
    max_files: usize,
    since_ms: Option<i64>,
) -> Result<TraceSummary, TraceError> {
    let mut builder = TraceSpanSummaryBuilder::new(since_ms);
    builder.ingest_rotated_files(trace_file_path, max_files)?;
    Ok(builder.build())
}

fn nonneg_i128_to_f64(value: i128) -> f64 {
    if value <= 0 {
        return 0.0;
    }
    let hi = u32::try_from(value >> 32).unwrap_or(u32::MAX);
    let lo = u32::try_from(value & 0xFFFF_FFFF).unwrap_or(0);
    f64::from(hi) * 4_294_967_296.0 + f64::from(lo)
}

fn usize_to_f64(value: usize) -> f64 {
    let val_u128 = u128::try_from(value).unwrap_or(u128::MAX);
    let hi = u32::try_from(val_u128 >> 32).unwrap_or(u32::MAX);
    let lo = u32::try_from(val_u128 & 0xFFFF_FFFF).unwrap_or(0);
    f64::from(hi) * 4_294_967_296.0 + f64::from(lo)
}
