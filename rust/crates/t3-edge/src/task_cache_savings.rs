//! Telemetry summary and lifetime savings models shared with [`crate::task_cache::D1BatchLedger`].

use serde::{Deserialize, Serialize};

/// Task execution source recorded in telemetry summaries (`Summary.ts:21`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSavingsSource {
    /// Restored from local cache.
    Local,
    /// Restored from remote edge cache.
    Remote,
    /// Cache hit whose source tier was unspecified.
    Unspecified,
    /// Task executed because of a cache miss.
    Executed,
    /// Cache restore failed and fell back to execution.
    Failed,
}

/// One task execution record inside a telemetry [`SummaryRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSavingsEntry {
    /// Workspace package name.
    pub package: String,
    /// Task target name.
    pub task: String,
    /// Cache outcome classification.
    pub source: TaskSavingsSource,
    /// Estimated milliseconds saved by cache restoration.
    pub saved_ms: u64,
}

/// Telemetry summary payload deduplicated by `id` inside a D1 batch (`Summary.ts:9-25`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRecord {
    /// 64-char hex receipt identifier.
    pub id: String,
    /// Epoch milliseconds when the summary was recorded.
    pub recorded_at_ms: u64,
    /// Developer identity key.
    pub developer: String,
    /// Host name.
    pub host: String,
    /// Operating system identifier.
    pub platform: String,
    /// CPU architecture identifier.
    pub arch: String,
    /// Whether the run executed in CI.
    pub ci: bool,
    /// Individual task entries.
    pub tasks: Vec<TaskSavingsEntry>,
}

/// Lifetime savings row keyed by `(developer, host, platform, arch, ci)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SavingsKey {
    /// Developer identifier.
    pub developer: String,
    /// Host name.
    pub host: String,
    /// OS platform.
    pub platform: String,
    /// CPU architecture.
    pub arch: String,
    /// CI flag.
    pub ci: bool,
}

/// Accumulated lifetime savings counters matching `Savings.ts:9-23`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavingsTotals {
    /// Local cache saved milliseconds.
    pub local_ms: u64,
    /// Unspecified tier saved milliseconds.
    pub unspecified_ms: u64,
    /// Unspecified tier cache hits.
    pub unspecified_hits: u64,
    /// Remote cache saved milliseconds.
    pub remote_ms: u64,
    /// Local cache hits.
    pub local_hits: u64,
    /// Remote cache hits.
    pub remote_hits: u64,
    /// Executed tasks.
    pub executed: u64,
    /// Failed cache restores.
    pub restore_failures: u64,
}
