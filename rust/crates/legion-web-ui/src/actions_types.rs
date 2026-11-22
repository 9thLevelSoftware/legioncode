//! Supporting input DTOs and tab enums for [`crate::actions::WebAppAction`].

use legion_client_core::steady_shells::EnvironmentShellStatus;
use serde::{Deserialize, Serialize};

/// Active right-hand detail panel in the web workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RightPaneTab {
    /// Thread message & timeline stream + composer.
    #[default]
    Chat,
    /// Unified diff inspector with `UTF-16` word-diff highlights (`BUG-22`).
    Diff,
    /// `VT` terminal grid + `"LCVT"` snapshot inspector (`BUG-25`).
    Terminal,
}

/// Incoming project record matching `/api/projects` and `/api/projects/mutate` JSON shapes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    /// Stable project identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Workspace directory root.
    pub workspace_root: String,
    /// Optional pin timestamp.
    #[serde(default)]
    pub pinned_at: Option<String>,
    /// Optional archive timestamp.
    #[serde(default)]
    pub archived_at: Option<String>,
    /// Last update timestamp.
    #[serde(default)]
    pub updated_at: String,
}

/// Thread summary payload used in steady-shell and thread-family updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummaryInput {
    /// Thread identifier.
    pub id: String,
    /// Owning project identifier.
    pub project_id: String,
    /// Thread display title.
    pub title: String,
    /// Active branch if attached.
    #[serde(default)]
    pub branch: Option<String>,
    /// Active model name.
    #[serde(default)]
    pub model: Option<String>,
}

/// Steady-shell snapshot batch entry passed to `WebAppAction::StepSteadyShells`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteadyShellBatchItem {
    /// Environment identifier.
    pub environment_id: String,
    /// Optional thread summaries held by this environment shell.
    #[serde(default)]
    pub threads: Option<Vec<ThreadSummaryInput>>,
    /// Authoritative snapshot sequence number.
    #[serde(default)]
    pub server_sequence: Option<u64>,
    /// Current environment shell synchronization status.
    pub status: EnvironmentShellStatus,
    /// Monotonic shell generation.
    pub generation: u64,
    /// Whether the connection is active or connecting.
    #[serde(default = "default_true")]
    pub connecting: bool,
    /// Monotonic user row-action counter.
    #[serde(default)]
    pub row_actions: u64,
}

/// Operations driving `legion_client_core::thread_families::ThreadFamilyCache`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ThreadFamilyOp {
    /// Advance shell generation for the environment (`generation > 0`).
    SetGeneration { generation: u64 },
    /// Mount a thread family reference.
    MountFamily,
    /// Start fetching a thread family for the current generation.
    BeginFetch,
    /// Complete a family fetch with settled rows or an error message.
    CompleteFetch {
        generation: u64,
        #[serde(default)]
        threads: Vec<ThreadSummaryInput>,
        #[serde(default)]
        snapshot_sequence: u64,
        #[serde(default)]
        error: Option<String>,
    },
    /// Release the oldest active mount handle for `(environment_id, thread_id)`.
    Release,
    /// Evict idle unmounted entries whose TTL has elapsed.
    EvictExpired,
}

/// Sparse palette override entry passed to `WebAppAction::TerminalApplyTheme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteOverrideInput {
    /// ANSI palette index (`0..=255`).
    pub index: u8,
    /// Color literal (`#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb:R/G/B`).
    pub color: String,
}

const fn default_true() -> bool {
    true
}
