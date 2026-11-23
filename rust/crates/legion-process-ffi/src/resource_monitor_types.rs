//! Protocol v3 inbound requests, process telemetry nodes, and sample structures.

use serde::{Deserialize, Serialize};

/// External root process tracked alongside the main `root_pid` tree.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalProcessV3 {
    /// OS process identifier.
    pub pid: u32,
    /// Optional expected start timestamp in Unix milliseconds for PID-reuse safety.
    #[serde(default)]
    pub start_time_ms: Option<u64>,
}

impl ExternalProcessV3 {
    pub(crate) const fn estimated_history_bytes() -> usize {
        std::mem::size_of::<Self>()
    }
}

/// Inbound protocol v3 NDJSON request sent by the `LegionCode` server to the resource monitor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MonitorRequestV3 {
    /// Initializes root PID, sampling interval, and initial external roots.
    Configure {
        /// Protocol version (`3`).
        version: u32,
        /// Root process identifier to trace recursively.
        root_pid: u32,
        /// Periodic sampling interval in milliseconds (`0` disables timer sampling).
        sample_interval_ms: u64,
        /// Additional root processes tracked independently of `root_pid`.
        #[serde(default)]
        external_processes: Vec<ExternalProcessV3>,
    },
    /// Replaces the tracked external root list.
    SetExternalProcesses {
        /// Protocol version (`3`).
        version: u32,
        /// Replacement external process roots.
        processes: Vec<ExternalProcessV3>,
    },
    /// Updates the periodic sampling interval in milliseconds (`0` disables timer sampling).
    SetSampleInterval {
        /// Protocol version (`3`).
        version: u32,
        /// New sampling interval in milliseconds.
        sample_interval_ms: u64,
    },
    /// Enables or disables unsolicited streaming snapshot emission.
    SetStreaming {
        /// Protocol version (`3`).
        version: u32,
        /// Whether periodic snapshots should be streamed automatically.
        enabled: bool,
    },
    /// Requests an immediate on-demand snapshot tagged with `request_id`.
    SampleNow {
        /// Protocol version (`3`).
        version: u32,
        /// Correlation identifier echoed on the response snapshot.
        request_id: String,
    },
    /// Requests a flat process table snapshot (`pid`, `ppid`, `name`).
    ProcessTable {
        /// Protocol version (`3`).
        version: u32,
        /// Correlation identifier echoed on the `processTable` response.
        request_id: String,
    },
    /// Requests retained history snapshots within `window_ms`.
    ReadHistory {
        /// Protocol version (`3`).
        version: u32,
        /// Correlation identifier echoed on `historyChunk` responses.
        request_id: String,
        /// Lookback window in milliseconds (capped at 1 hour).
        window_ms: u64,
    },
    /// Requests clean monitor termination.
    Shutdown {
        /// Protocol version (`3`).
        version: u32,
    },
}

impl MonitorRequestV3 {
    /// Returns the protocol version carried by this request.
    pub const fn version(&self) -> u32 {
        match self {
            Self::Configure { version, .. }
            | Self::SetExternalProcesses { version, .. }
            | Self::SetSampleInterval { version, .. }
            | Self::SetStreaming { version, .. }
            | Self::SampleNow { version, .. }
            | Self::ProcessTable { version, .. }
            | Self::ReadHistory { version, .. }
            | Self::Shutdown { version } => *version,
        }
    }
}

/// Platform I/O counter semantics advertised by the sampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IoSemantics {
    /// Linux/POSIX block/storage I/O accounting.
    Storage,
    /// Windows combined disk + pipe/device I/O accounting.
    AllIo,
}

/// Returns the host platform's native [`IoSemantics`].
pub const fn platform_io_semantics() -> IoSemantics {
    if cfg!(windows) {
        IoSemantics::AllIo
    } else {
        IoSemantics::Storage
    }
}

/// Per-process resource telemetry node inside a [`ResourceSampleV3`] snapshot.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessNodeMetrics {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier.
    pub ppid: u32,
    /// Process start timestamp in Unix milliseconds.
    pub start_time_ms: u64,
    /// Process elapsed run time in milliseconds.
    pub run_time_ms: u64,
    /// Process short name.
    pub name: String,
    /// Command line or executable path.
    pub command: String,
    /// Process run state.
    pub status: String,
    /// Rolling CPU usage percentage (`100.0` = one full core).
    pub cpu_percent: f64,
    /// Cumulative CPU time (user + kernel) in milliseconds.
    pub cpu_time_ms: u64,
    /// Resident working set bytes.
    pub resident_bytes: u64,
    /// Virtual / private memory bytes.
    pub virtual_bytes: u64,
    /// Cumulative bytes read.
    pub io_read_bytes: u64,
    /// Cumulative bytes written.
    pub io_write_bytes: u64,
    /// Accounting scope for `io_read_bytes` / `io_write_bytes`.
    pub io_semantics: IoSemantics,
}

impl ProcessNodeMetrics {
    pub(crate) fn estimated_history_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.name.len())
            .saturating_add(self.command.len())
            .saturating_add(self.status.len())
    }
}

/// Protocol v3 snapshot payload (`type: "snapshot"`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSampleV3 {
    /// Protocol version (`3`).
    pub version: u32,
    /// Wire event discriminant (`"snapshot"`).
    #[serde(rename = "type")]
    pub event_type: String,
    /// Monotonic sample sequence counter.
    pub sequence: u64,
    /// Unix epoch timestamp in milliseconds when sampled.
    pub sampled_at_unix_ms: u64,
    /// Duration of collection in microseconds.
    pub collection_duration_micros: u64,
    /// Total number of OS processes scanned.
    pub scanned_process_count: usize,
    /// Number of retained processes in `processes`.
    pub retained_process_count: usize,
    /// Number of tracked PIDs that exited or could not be opened.
    pub inaccessible_process_count: usize,
    /// Optional request correlation ID (set on `sampleNow` responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Live external processes matching their identity checks.
    #[serde(default)]
    pub external_processes: Vec<ExternalProcessV3>,
    /// Sampled metrics for tracked processes sorted by PID.
    pub processes: Vec<ProcessNodeMetrics>,
}

impl ResourceSampleV3 {
    pub(crate) fn retained_entry_count(&self) -> usize {
        self.processes
            .len()
            .saturating_add(self.external_processes.len())
    }

    pub(crate) fn estimated_history_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.processes
                    .iter()
                    .map(ProcessNodeMetrics::estimated_history_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(
                self.external_processes
                    .len()
                    .saturating_mul(ExternalProcessV3::estimated_history_bytes()),
            )
    }
}
