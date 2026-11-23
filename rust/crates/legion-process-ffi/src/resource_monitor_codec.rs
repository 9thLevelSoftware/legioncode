//! Rolling CPU-time delta computation and NDJSON v3 codec.

use std::collections::{HashMap, HashSet, VecDeque};

use thiserror::Error;

use crate::resource_monitor_events::MonitorEventV3;
use crate::resource_monitor_types::MonitorRequestV3;

const MAX_NDJSON_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Rolling CPU-time delta tracker computing per-process `cpu_percent` across sample ticks.
///
/// Tracks a bounded rolling history of `(observed_at_ms, cpu_time_ms)` observations keyed by
/// `(pid, start_time_ms)` so PID reuse or clock/counter resets cleanly re-anchor at `0.0%`.
#[derive(Debug, Clone)]
pub struct CpuDeltaTracker {
    window_samples: usize,
    entries: HashMap<u32, PidCpuHistory>,
}

#[derive(Debug, Clone)]
struct PidCpuHistory {
    start_time_ms: u64,
    points: VecDeque<(u64, u64)>,
}

impl Default for CpuDeltaTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuDeltaTracker {
    /// Creates a delta tracker using a 1-interval delta (`window_samples = 1`).
    pub fn new() -> Self {
        Self::with_window(1)
    }

    /// Creates a delta tracker averaging CPU delta over up to `window_samples` intervals (`>= 1`).
    pub fn with_window(window_samples: usize) -> Self {
        Self {
            window_samples: window_samples.max(1),
            entries: HashMap::new(),
        }
    }

    /// Records a new `(start_time_ms, cpu_time_ms, observed_at_ms)` observation for `pid`
    /// and returns the computed rolling CPU percentage (`100.0` = one full CPU core).
    pub fn record_and_compute(
        &mut self,
        pid: u32,
        start_time_ms: u64,
        cpu_time_ms: u64,
        observed_at_ms: u64,
    ) -> f64 {
        let entry = self.entries.entry(pid).or_insert_with(|| PidCpuHistory {
            start_time_ms,
            points: VecDeque::with_capacity(self.window_samples + 1),
        });
        if entry.start_time_ms != start_time_ms {
            entry.start_time_ms = start_time_ms;
            entry.points.clear();
        }
        if let Some(&(prev_at, prev_cpu)) = entry.points.back()
            && (observed_at_ms <= prev_at || cpu_time_ms < prev_cpu)
        {
            entry.points.clear();
        }

        let cpu_percent = if let Some(&(base_at, base_cpu)) = entry.points.front() {
            let delta_wall = observed_at_ms.saturating_sub(base_at);
            let delta_cpu = cpu_time_ms.saturating_sub(base_cpu);
            if delta_wall == 0 {
                0.0
            } else {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "CPU and wall deltas in ms fit inside 53-bit f64 mantissa"
                )]
                let pct = (delta_cpu as f64 / delta_wall as f64) * 100.0;
                if pct.is_finite() && pct >= 0.0 {
                    pct
                } else {
                    0.0
                }
            }
        } else {
            0.0
        };

        entry.points.push_back((observed_at_ms, cpu_time_ms));
        while entry.points.len() > self.window_samples.saturating_add(1) {
            entry.points.pop_front();
        }
        cpu_percent
    }

    /// Removes entries for PIDs no longer present in `active_pids`.
    pub fn prune<S: std::hash::BuildHasher>(&mut self, active_pids: &HashSet<u32, S>) {
        self.entries.retain(|pid, _| active_pids.contains(pid));
    }
}

/// Errors produced by the protocol v3 NDJSON codec.
#[derive(Debug, Error)]
pub enum MonitorCodecError {
    /// Input NDJSON line exceeded `16 MiB` without a newline delimiter.
    #[error("NDJSON frame exceeded maximum allowed size ({0} bytes)")]
    FrameTooLarge(usize),
    /// Input bytes were not valid UTF-8.
    #[error("NDJSON frame was not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    /// JSON serialization or deserialization failed.
    #[error("NDJSON JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Streaming LF-delimited NDJSON codec for protocol v3 requests and events.
#[derive(Debug, Default)]
pub struct NdjsonV3Codec {
    pending: Vec<u8>,
}

impl NdjsonV3Codec {
    /// Creates an empty [`NdjsonV3Codec`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends raw bytes and extracts any complete non-empty UTF-8 NDJSON lines.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<String>, MonitorCodecError> {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_NDJSON_FRAME_BYTES
            && !self.pending[..MAX_NDJSON_FRAME_BYTES].contains(&b'\n')
        {
            return Err(MonitorCodecError::FrameTooLarge(self.pending.len()));
        }

        let mut lines = Vec::new();
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let mut raw_line: Vec<u8> = self.pending.drain(..=pos).collect();
            if raw_line.last() == Some(&b'\n') {
                raw_line.pop();
            }
            if raw_line.last() == Some(&b'\r') {
                raw_line.pop();
            }
            let text = std::str::from_utf8(&raw_line)?;
            if !text.trim().is_empty() {
                lines.push(text.to_owned());
            }
        }
        Ok(lines)
    }

    /// Decodes a single NDJSON line into [`MonitorRequestV3`] (`None` for blank lines).
    pub fn decode_request_line(line: &str) -> Result<Option<MonitorRequestV3>, MonitorCodecError> {
        if line.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(line)?))
    }

    /// Encodes a [`MonitorRequestV3`] as a single `\n`-terminated NDJSON line.
    pub fn encode_request_line(request: &MonitorRequestV3) -> Result<String, MonitorCodecError> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        Ok(line)
    }

    /// Decodes a single NDJSON line into [`MonitorEventV3`] (`None` for blank lines).
    pub fn decode_event_line(line: &str) -> Result<Option<MonitorEventV3>, MonitorCodecError> {
        if line.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(line)?))
    }

    /// Encodes a [`MonitorEventV3`] as a single `\n`-terminated NDJSON line.
    pub fn encode_event_line(event: &MonitorEventV3) -> Result<String, MonitorCodecError> {
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        Ok(line)
    }
}
