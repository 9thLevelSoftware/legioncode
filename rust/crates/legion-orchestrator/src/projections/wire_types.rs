//! Effect-Schema-exact wire JSON structs matching `packages/contracts/src/{project,orchestrationProject,orchestrationV2}.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use super::wire_projects::{WireProject, WireProjectSnapshot};
pub use super::wire_thread_types::{
    WireProjectedTurnItem, WireThreadBoundedSnapshot, WireThreadDetailSnapshot,
    WireThreadHistoryPage, WireThreadProjection, WireThreadTranscript,
};

/// Shell representation of a project matching `OrchestrationProjectShell`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProjectShell {
    /// Stable project identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Whether the user explicitly customized the title.
    pub has_custom_title: bool,
    /// Absolute workspace root directory path.
    pub workspace_root: String,
    /// Repository identity descriptor (`null` until enriched).
    pub repository_identity: Option<Value>,
    /// Optional default model selection.
    pub default_model_selection: Option<Value>,
    /// Optional default thread environment mode.
    pub default_thread_env_mode: Option<String>,
    /// Whether auto-pull is enabled.
    pub auto_pull: bool,
    /// Optional favicon path.
    pub favicon_path: Option<String>,
    /// Optional project icon descriptor.
    pub project_icon: Option<Value>,
    /// Configured setup/dev scripts.
    pub scripts: Vec<Value>,
    /// Pin timestamp.
    pub pinned_at: Option<String>,
    /// Archive timestamp.
    pub archived_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Per-project count of older settled threads (`OrchestrationV2OlderSettledCount`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireOlderSettledCount {
    /// Project ID.
    pub project_id: String,
    /// Number of omitted older settled threads in this project.
    pub count: u64,
}

/// Summary of omitted older settled threads (`OrchestrationV2OlderSettledThreads`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireOlderSettledThreads {
    /// Cut timestamp before which top-level settled threads are omitted.
    pub before: String,
    /// Sequence at which these counts were computed.
    pub as_of_sequence: i64,
    /// Per-project counts of omitted older settled threads.
    pub counts: Vec<WireOlderSettledCount>,
}

/// Keyset cursor for `list_wire_older_settled_threads` (`OrchestrationV2OlderSettledCursor`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireOlderSettledCursor {
    /// Settled timestamp cut (`settledAt`).
    pub settled_at: String,
    /// Optional tie-breaker thread ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Paged older settled threads response (`OrchestrationV2OlderSettledThreadsPage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireOlderSettledThreadsPage {
    /// Sequence at which the page was queried.
    pub snapshot_sequence: i64,
    /// Matching older settled thread shells (empty when `limit == 0`).
    pub threads: Vec<WireThreadShell>,
    /// Per-project counts below cursor (filled when `limit == 0`).
    pub counts: Vec<WireOlderSettledCount>,
}

/// Pending runtime request summary inside [`WireThreadShell`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePendingRuntimeRequestSummary {
    /// Request identifier.
    pub id: String,
    /// Request kind (`approval`, `user_input`, etc.).
    pub kind: String,
    /// Creation timestamp.
    pub created_at: String,
    /// False for async prompts that do not block the active run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
    /// True for secret/link cards answerable only by a human user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_only: Option<bool>,
}

/// Wire shell summary for a single thread (`OrchestrationV2ThreadShell`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "1:1 wire projection schema fields matching TS OrchestrationV2ThreadShell"
)]
pub struct WireThreadShell {
    /// Creator actor.
    pub created_by: String,
    /// Creation surface.
    pub creation_source: String,
    /// Thread ID.
    pub id: String,
    /// Project ID.
    pub project_id: String,
    /// Display title.
    pub title: String,
    /// Active provider instance ID.
    pub provider_instance_id: String,
    /// Model selection object (`{ instanceId, model, options? }`).
    pub model_selection: Value,
    /// Runtime permission mode.
    pub runtime_mode: String,
    /// Provider interaction mode.
    pub interaction_mode: String,
    /// Checked-out branch or `null`.
    pub branch: Option<String>,
    /// Worktree path or `null`.
    pub worktree_path: Option<String>,
    /// Multi-PR links array.
    pub pull_requests: Vec<Value>,
    /// Legacy single linked pull request when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_pull_request: Option<Value>,
    /// Current branch pull request when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_pull_request: Option<Value>,
    /// Linked issues when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<Vec<Value>>,
    /// Active sort order key when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_order_key: Option<Value>,
    /// Thread lineage coordinates.
    pub lineage: Value,
    /// Fork source point or `null`.
    pub forked_from: Option<Value>,
    /// Active provider thread ID or `null`.
    pub active_provider_thread_id: Option<String>,
    /// History origin when present (`"native"` or `"v1_import"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_origin: Option<String>,
    /// MCP link origin when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_origin: Option<Value>,
    /// Remote parent provenance when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_from: Option<Value>,
    /// Workflow coordinator step origin when present (`#38882`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<Value>,
    /// Latest presented run ID or `null`.
    pub latest_run_id: Option<String>,
    /// Latest run requested timestamp or `null`.
    pub latest_run_requested_at: Option<String>,
    /// Latest run started timestamp or `null`.
    pub latest_run_started_at: Option<String>,
    /// Latest run completed timestamp or `null`.
    pub latest_run_completed_at: Option<String>,
    /// Whether an interrupt request was recorded on the latest run.
    pub latest_run_stop_requested: bool,
    /// Whether the latest run answered a non-terminal background-work wake.
    pub latest_run_background_wake: bool,
    /// Active run ID or `null`.
    pub active_run_id: Option<String>,
    /// Whether the active run is waiting on child thread(s).
    pub blocked_on_thread_wait: bool,
    /// Active run activity status or `null`.
    pub activity_run_status: Option<String>,
    /// Active run activity start timestamp or `null`.
    pub activity_run_started_at: Option<String>,
    /// Presented run status or `"idle"`.
    pub status: String,
    /// Last error string or `null`.
    pub last_error: Option<String>,
    /// Last error class or `null`.
    pub last_error_class: Option<String>,
    /// Last error code or `null`.
    pub last_error_code: Option<String>,
    /// Quota reset timestamp or `null`.
    pub usage_limit_reset_at: Option<String>,
    /// Pending approval or user-input request summary, or `null`.
    pub pending_runtime_request: Option<WirePendingRuntimeRequestSummary>,
    /// Pending guest-submitted message count.
    pub pending_guest_message_count: u64,
    /// Always `null` in V2 shell snapshots (thread detail owns message bodies).
    pub latest_visible_message: Option<Value>,
    /// Latest user message timestamp or `null`.
    pub latest_user_message_at: Option<String>,
    /// Latest human-authored user message timestamp or `null`.
    pub latest_user_authored_message_at: Option<String>,
    /// Whether an active proposed plan artifact exists.
    pub has_actionable_proposed_plan: bool,
    /// Pending background task summaries.
    pub pending_background_tasks: Vec<Value>,
    /// Ordered provider instances attached over the thread's lifetime.
    pub provider_instance_history: Vec<String>,
    /// Active provider thread goal or `null`.
    pub goal: Option<Value>,
    /// Visible local turn item count.
    pub item_count: u64,
    /// Total visible turn item count including inherited fork items.
    pub visible_item_count: u64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Archive timestamp or `null`.
    pub archived_at: Option<String>,
    /// Settlement override (`"settled"`, `"active"`, or `null`).
    pub settled_override: Option<String>,
    /// Settlement timestamp or `null`.
    pub settled_at: Option<String>,
    /// Unsettled timestamp or `null`.
    pub unsettled_at: Option<String>,
    /// Snooze expiration timestamp or `null`.
    pub snoozed_until: Option<String>,
    /// Snooze start timestamp or `null`.
    pub snoozed_at: Option<String>,
    /// Pin timestamp or `null`.
    pub pinned_at: Option<String>,
    /// Auto-settle opt-out timestamp or `null`.
    pub auto_settle_disabled_at: Option<String>,
    /// Pin order key or `null`.
    pub pin_order_key: Option<String>,
    /// Non-empty thread tags (omitted when empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Last visited timestamp or `null`.
    pub last_visited_at: Option<String>,
    /// Open ranking state or `null`.
    pub opens: Option<Value>,
    /// In-flight title regeneration state or `null`.
    pub title_regeneration: Option<Value>,
    /// Last completed title regeneration state or `null`.
    pub last_title_regeneration: Option<Value>,
    /// Limit recovery state or `null`.
    pub limit_recovery: Option<Value>,
    /// Soft deletion timestamp or `null`.
    pub deleted_at: Option<String>,
}

/// Options controlling shell snapshot trimming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireShellOptions {
    /// Omit idle subagent children unless active or parented above an active child.
    pub omit_idle_subagents: bool,
    /// Omit settled top-level threads beyond the newest 25 cut.
    pub omit_older_settled: bool,
}

impl Default for WireShellOptions {
    fn default() -> Self {
        Self {
            omit_idle_subagents: true,
            omit_older_settled: true,
        }
    }
}

/// Base shell snapshot payload (`OrchestrationV2ShellSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireShellSnapshot {
    /// Projection schema version (`2`).
    pub schema_version: i64,
    /// Highest application event sequence reflected in this snapshot.
    pub snapshot_sequence: i64,
    /// Active non-deleted threads matching trimming rules.
    pub threads: Vec<WireThreadShell>,
    /// Archived threads (always `[]` in active HTTP/stream shell snapshots).
    pub archived_threads: Vec<WireThreadShell>,
    /// Non-deleted projects ordered by creation timestamp.
    pub projects: Vec<WireProjectShell>,
    /// Summary of omitted older settled threads when `omit_older_settled` trimmed any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_settled: Option<WireOlderSettledThreads>,
}

/// HTTP shell snapshot payload (`OrchestrationV2ShellHttpSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireShellHttpSnapshot {
    /// Projection schema version (`2`).
    pub schema_version: i64,
    /// Highest application event sequence reflected in this snapshot.
    pub snapshot_sequence: i64,
    /// Active non-deleted threads.
    pub threads: Vec<WireThreadShell>,
    /// Archived threads (`[]` for active shell snapshots).
    pub archived_threads: Vec<WireThreadShell>,
    /// Non-deleted projects.
    pub projects: Vec<WireProjectShell>,
    /// Optional summary of omitted older settled threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_settled: Option<WireOlderSettledThreads>,
    /// Base64url SHA-256 digest of `[project.id, project.repositoryIdentity ?? null]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_identities: Option<String>,
}

/// Incremental or reset stream item (`OrchestrationV2ShellStreamItem`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum WireShellStreamEvent {
    /// Client replay caught up to live tail.
    #[serde(rename = "synchronized")]
    Synchronized,
    /// Gap/prune prevented replay; client must reload full HTTP snapshot.
    #[serde(rename = "snapshot-required")]
    SnapshotRequired,
    /// Full shell reset snapshot.
    #[serde(rename = "snapshot", rename_all = "camelCase")]
    Snapshot {
        /// Authoritative shell snapshot.
        snapshot: Box<WireShellSnapshot>,
        /// Optional resolved repository identity workspace roots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_repository_identity_roots: Option<Vec<String>>,
    },
    /// Created or updated project shell.
    #[serde(rename = "project.updated", rename_all = "camelCase")]
    ProjectUpdated {
        /// Application event sequence.
        sequence: i64,
        /// Updated project shell.
        project: Box<WireProjectShell>,
    },
    /// Deleted project ID.
    #[serde(rename = "project.removed", rename_all = "camelCase")]
    ProjectRemoved {
        /// Application event sequence.
        sequence: i64,
        /// Removed project ID.
        project_id: String,
    },
    /// Created or updated thread shell.
    #[serde(rename = "thread.updated", rename_all = "camelCase")]
    ThreadUpdated {
        /// Application event sequence.
        sequence: i64,
        /// Shell partition (`"active"` or `"archive"`).
        location: String,
        /// Updated thread shell.
        thread: Box<WireThreadShell>,
    },
    /// Archived or deleted thread removed from `location`.
    #[serde(rename = "thread.removed", rename_all = "camelCase")]
    ThreadRemoved {
        /// Application event sequence.
        sequence: i64,
        /// Shell partition (`"active"` or `"archive"`).
        location: String,
        /// Removed thread ID.
        thread_id: String,
    },
}
