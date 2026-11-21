//! `native/resource-monitor` protocol v3 engine and re-exports.
//!
//! Provides exact v3 JSON wire parity (`MonitorRequestV3`, `ResourceSampleV3`,
//! `ProcessNodeMetrics`), rolling CPU-time delta computation (`CpuDeltaTracker`),
//! bounded history retention (`HistoryRecorder`), and a subprocess-free OS sampler.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

pub use crate::resource_monitor_codec::{CpuDeltaTracker, MonitorCodecError, NdjsonV3Codec};
pub use crate::resource_monitor_events::{
    HelloEventV3, HistoryChunkEventV3, MonitorCapabilitiesV3, MonitorErrorEventV3, MonitorEventV3,
    ProcessTableEntryV3, ProcessTableEventV3,
};
pub use crate::resource_monitor_history::{
    HISTORY_CHUNK_SNAPSHOTS, HISTORY_RETENTION_MS, HistoryRecorder, MAX_HISTORY_RETAINED_BYTES,
    MAX_HISTORY_RETAINED_ENTRIES, MAX_HISTORY_SNAPSHOTS, MAX_PROCESS_COMMAND_BYTES,
    MAX_PROCESS_NAME_BYTES, MAX_PROCESS_STATUS_BYTES, MAX_SAMPLE_INTERVAL_MS,
    MIN_SAMPLE_INTERVAL_MS, PROCESS_START_TIME_PRECISION_MS, RESOURCE_MONITOR_PROTOCOL_VERSION,
    clamp_sample_interval, inaccessible_process_count, matches_external_identity,
    select_tracked_pids, truncate_utf8, unix_time_ms,
};
pub use crate::resource_monitor_sampler::{OsProcessSampler, ProcessSampler, RawProcessSample};
pub use crate::resource_monitor_types::{
    ExternalProcessV3, IoSemantics, MonitorRequestV3, ProcessNodeMetrics, ResourceSampleV3,
    platform_io_semantics,
};

#[derive(Debug, Clone)]
struct EngineConfig {
    root_pid: u32,
    sample_interval: Option<Duration>,
    external_processes: HashMap<u32, Option<u64>>,
}

/// Stateful protocol v3 resource monitor engine driven by a [`ProcessSampler`].
///
/// # Examples
///
/// ```
/// use t3_process_ffi::resource_monitor::{
///     MonitorEventV3, MonitorRequestV3, RESOURCE_MONITOR_PROTOCOL_VERSION, ResourceMonitorEngine,
/// };
///
/// let mut engine = ResourceMonitorEngine::os_default();
/// assert!(matches!(engine.hello_event(), MonitorEventV3::Hello(_)));
/// let events = engine.handle_request(MonitorRequestV3::SampleNow {
///     version: RESOURCE_MONITOR_PROTOCOL_VERSION,
///     request_id: "r1".to_owned(),
/// });
/// assert!(matches!(&events[0], MonitorEventV3::Error(e) if e.code == "not-configured"));
/// ```
#[derive(Debug)]
pub struct ResourceMonitorEngine<S: ProcessSampler> {
    sampler: S,
    history: HistoryRecorder,
    cpu_tracker: CpuDeltaTracker,
    config: Option<EngineConfig>,
    sequence: u64,
    streaming_enabled: bool,
    shutdown_requested: bool,
}

impl ResourceMonitorEngine<OsProcessSampler> {
    /// Creates an engine backed by the subprocess-free [`OsProcessSampler`].
    pub fn os_default() -> Self {
        Self::new(OsProcessSampler::new())
    }
}

impl<S: ProcessSampler> ResourceMonitorEngine<S> {
    /// Creates a new protocol v3 engine around `sampler`.
    pub fn new(sampler: S) -> Self {
        Self {
            sampler,
            history: HistoryRecorder::new(),
            cpu_tracker: CpuDeltaTracker::new(),
            config: None,
            sequence: 0,
            streaming_enabled: false,
            shutdown_requested: false,
        }
    }

    /// Replaces the rolling CPU window size (`>= 1` interval).
    #[must_use]
    pub fn with_cpu_window(mut self, window_samples: usize) -> Self {
        self.cpu_tracker = CpuDeltaTracker::with_window(window_samples);
        self
    }

    /// Returns `true` after a `Shutdown` request has been processed.
    pub const fn is_shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// Returns whether periodic streaming is currently enabled.
    pub const fn is_streaming_enabled(&self) -> bool {
        self.streaming_enabled
    }

    /// Returns the configured sampling interval, if configured and non-zero.
    pub fn sample_interval(&self) -> Option<Duration> {
        self.config.as_ref().and_then(|cfg| cfg.sample_interval)
    }

    /// Returns a reference to the retained [`HistoryRecorder`].
    pub const fn history(&self) -> &HistoryRecorder {
        &self.history
    }

    /// Builds the v3 `hello` handshake event.
    pub fn hello_event(&self) -> MonitorEventV3 {
        MonitorEventV3::Hello(HelloEventV3 {
            version: RESOURCE_MONITOR_PROTOCOL_VERSION,
            event_type: "hello".to_owned(),
            sidecar_version: env!("CARGO_PKG_VERSION").to_owned(),
            sidecar_pid: std::process::id(),
            platform: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            capabilities: MonitorCapabilitiesV3::default(),
        })
    }

    /// Handles one decoded [`MonitorRequestV3`] and returns zero or more outbound events.
    pub fn handle_request(&mut self, request: MonitorRequestV3) -> Vec<MonitorEventV3> {
        if request.version() != RESOURCE_MONITOR_PROTOCOL_VERSION {
            return vec![Self::error_event(
                "protocol-mismatch",
                format!(
                    "unsupported protocol version {}; expected {RESOURCE_MONITOR_PROTOCOL_VERSION}",
                    request.version()
                ),
                false,
            )];
        }
        match request {
            MonitorRequestV3::Configure {
                root_pid,
                sample_interval_ms,
                external_processes,
                ..
            } => {
                self.config = Some(EngineConfig {
                    root_pid,
                    sample_interval: clamp_sample_interval(sample_interval_ms),
                    external_processes: external_processes
                        .into_iter()
                        .map(|p| (p.pid, p.start_time_ms))
                        .collect(),
                });
                Vec::new()
            }
            MonitorRequestV3::SetExternalProcesses { processes, .. } => {
                let Some(cfg) = self.config.as_mut() else {
                    return vec![Self::not_configured_err(
                        "configure must be sent before external processes",
                    )];
                };
                cfg.external_processes = processes
                    .into_iter()
                    .map(|p| (p.pid, p.start_time_ms))
                    .collect();
                Vec::new()
            }
            MonitorRequestV3::SetSampleInterval {
                sample_interval_ms, ..
            } => {
                let Some(cfg) = self.config.as_mut() else {
                    return vec![Self::not_configured_err(
                        "configure must be sent before changing the sample interval",
                    )];
                };
                cfg.sample_interval = clamp_sample_interval(sample_interval_ms);
                Vec::new()
            }
            MonitorRequestV3::SetStreaming { enabled, .. } => {
                self.streaming_enabled = enabled;
                Vec::new()
            }
            MonitorRequestV3::SampleNow { request_id, .. } => {
                let Some(sample) = self.collect_sample(Some(request_id), unix_time_ms()) else {
                    return vec![Self::not_configured_err(
                        "configure must be sent before sampling",
                    )];
                };
                self.history.record(&sample);
                vec![MonitorEventV3::Snapshot(sample)]
            }
            MonitorRequestV3::ProcessTable { request_id, .. } => {
                vec![MonitorEventV3::ProcessTable(ProcessTableEventV3 {
                    version: RESOURCE_MONITOR_PROTOCOL_VERSION,
                    event_type: "processTable".to_owned(),
                    request_id,
                    processes: self.sampler.process_table(),
                })]
            }
            MonitorRequestV3::ReadHistory {
                request_id,
                window_ms,
                ..
            } => {
                if self.config.is_none() {
                    return vec![Self::not_configured_err(
                        "configure must be sent before reading history",
                    )];
                }
                let snapshots = self.history.read(window_ms, unix_time_ms());
                crate::resource_monitor_events::build_history_chunks(request_id, &snapshots)
            }
            MonitorRequestV3::Shutdown { .. } => {
                self.shutdown_requested = true;
                Vec::new()
            }
        }
    }

    /// Decodes and handles one raw NDJSON line, returning outbound events (empty on blank lines).
    pub fn handle_ndjson_line(&mut self, line: &str) -> Vec<MonitorEventV3> {
        match NdjsonV3Codec::decode_request_line(line) {
            Ok(Some(req)) => self.handle_request(req),
            Ok(None) => Vec::new(),
            Err(err) => vec![Self::error_event(
                "invalid-command",
                format!("invalid command: {err}"),
                true,
            )],
        }
    }

    /// Captures a periodic sample at `now_unix_ms`, recording it into history.
    pub fn sample_tick_at(&mut self, now_unix_ms: u64) -> Option<ResourceSampleV3> {
        let sample = self.collect_sample(None, now_unix_ms)?;
        self.history.record(&sample);
        Some(sample)
    }

    /// Runs a synchronous NDJSON v3 session over `reader` and `writer` starting with `hello`.
    pub fn run_stdio_session<R: BufRead, W: Write>(
        &mut self,
        reader: R,
        writer: &mut W,
    ) -> io::Result<()> {
        Self::write_event(writer, &self.hello_event())?;
        for line in reader.lines() {
            let line = line?;
            for event in self.handle_ndjson_line(&line) {
                Self::write_event(writer, &event)?;
            }
            if self.shutdown_requested {
                break;
            }
        }
        Ok(())
    }

    fn write_event<W: Write>(writer: &mut W, event: &MonitorEventV3) -> io::Result<()> {
        let line = NdjsonV3Codec::encode_event_line(event)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        writer.write_all(line.as_bytes())?;
        writer.flush()
    }

    fn collect_sample(
        &mut self,
        request_id: Option<String>,
        sampled_at_unix_ms: u64,
    ) -> Option<ResourceSampleV3> {
        let cfg = self.config.as_ref()?.clone();
        let started = Instant::now();
        let mut requested_roots: Vec<u32> = cfg.external_processes.keys().copied().collect();
        requested_roots.push(cfg.root_pid);

        let (scanned_process_count, candidates) = self.sampler.sample_candidates(&requested_roots);
        let by_pid: HashMap<u32, RawProcessSample> =
            candidates.into_iter().map(|s| (s.pid, s)).collect();
        let external_processes = cfg
            .external_processes
            .iter()
            .filter_map(|(&pid, &expected)| {
                let proc_sample = by_pid.get(&pid)?;
                matches_external_identity(proc_sample.start_time_ms, expected).then_some(
                    ExternalProcessV3 {
                        pid,
                        start_time_ms: Some(proc_sample.start_time_ms),
                    },
                )
            })
            .collect::<Vec<_>>();

        let mut active_roots: HashSet<u32> = external_processes.iter().map(|p| p.pid).collect();
        active_roots.insert(cfg.root_pid);
        let rows: Vec<(u32, u32, u64)> = by_pid
            .values()
            .map(|s| (s.pid, s.ppid, s.start_time_ms))
            .collect();
        let tracked = select_tracked_pids(&rows, &active_roots);
        let selected_count = tracked.len();

        let mut processes: Vec<ProcessNodeMetrics> = tracked
            .iter()
            .filter_map(|pid| {
                let raw = by_pid.get(pid)?;
                Some(self.build_node_metric(raw, sampled_at_unix_ms))
            })
            .collect();
        processes.sort_by_key(|p| p.pid);
        self.cpu_tracker.prune(&tracked);
        self.sequence = self.sequence.saturating_add(1);

        let elapsed_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        Some(ResourceSampleV3 {
            version: RESOURCE_MONITOR_PROTOCOL_VERSION,
            event_type: "snapshot".to_owned(),
            sequence: self.sequence,
            sampled_at_unix_ms,
            collection_duration_micros: elapsed_micros,
            scanned_process_count,
            retained_process_count: processes.len(),
            inaccessible_process_count: inaccessible_process_count(selected_count, processes.len()),
            request_id,
            external_processes,
            processes,
        })
    }

    fn build_node_metric(
        &mut self,
        raw: &RawProcessSample,
        observed_at_ms: u64,
    ) -> ProcessNodeMetrics {
        let cpu_percent = self.cpu_tracker.record_and_compute(
            raw.pid,
            raw.start_time_ms,
            raw.cpu_time_ms,
            observed_at_ms,
        );
        ProcessNodeMetrics {
            pid: raw.pid,
            ppid: raw.ppid,
            start_time_ms: raw.start_time_ms,
            run_time_ms: raw.run_time_ms,
            name: truncate_utf8(raw.name.clone(), MAX_PROCESS_NAME_BYTES),
            command: truncate_utf8(raw.command.clone(), MAX_PROCESS_COMMAND_BYTES),
            status: truncate_utf8(raw.status.clone(), MAX_PROCESS_STATUS_BYTES),
            cpu_percent,
            cpu_time_ms: raw.cpu_time_ms,
            resident_bytes: raw.resident_bytes,
            virtual_bytes: raw.virtual_bytes,
            io_read_bytes: raw.io_read_bytes,
            io_write_bytes: raw.io_write_bytes,
            io_semantics: platform_io_semantics(),
        }
    }

    fn not_configured_err(msg: &str) -> MonitorEventV3 {
        Self::error_event("not-configured", msg, true)
    }

    fn error_event(
        code: impl Into<String>,
        message: impl Into<String>,
        recoverable: bool,
    ) -> MonitorEventV3 {
        crate::resource_monitor_events::build_error_event(code, message, recoverable)
    }
}
