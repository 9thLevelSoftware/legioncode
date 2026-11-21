//! Feature state models for repeatable thread error banners, Environments Map & P2P batch
//! linking, unified linked-thread pane, multi-repo project picker, secret cards, MCP sign-in,
//! `composerEmpty` keybinding evaluation/search, and sidebar update countdown capsule.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use t3_client_core::linked_threads::{
    HistoryLoadStatus, LinkedThreadTarget, LinkedThreadViewState, PinnedThreadRepository,
};
use t3_contracts::peer_links::{PeerLinkFailureKind, PeerMcpClientAccess};
use t3_contracts::workspace_folders::{TaskFolder, WorkspaceScannedFolder};
use t3_surfaces::mobile_review::SecretCardState;

/// Source classification of a thread error banner (`5872b0a488` / `aa6fe390db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadErrorSource {
    /// Persisted server runtime failure banner.
    Runtime,
    /// Transient local send/composer validation failure banner.
    Local,
}

/// Active thread error banner tagged with a strictly monotonic `occurrence_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadErrorBanner {
    /// Monotonically increasing occurrence counter ensuring repeat errors re-appear.
    pub occurrence_id: u64,
    /// Origin of this failure banner.
    pub source: ThreadErrorSource,
    /// Human-readable error message.
    pub message: String,
    /// Optional server runtime correlation key so duplicate re-projections stay dismissed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_key: Option<String>,
}

/// Edge relationship displayed inside Environments Map (`ccabed086a`, `6f644f6e91` — no cloud relay).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FleetEdgeKind {
    /// Direct local/tailnet/LAN client route configured for this environment.
    ClientRoute,
    /// Direct P2P pairing link between two environments.
    DirectPeerLink,
    /// Transitive reachability routed through a full-access intermediate peer (`6f644f6e91`).
    TransitivePeerLink { via_peer_id: String },
}

/// Node in the local + P2P Environments Map (never models Ping cloud relay).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetEnvironmentNode {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub linked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<PeerMcpClientAccess>,
    #[serde(default)]
    pub reachable: bool,
}

/// Edge in the local + P2P Environments Map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetEnvironmentEdge {
    pub from_id: String,
    pub to_id: String,
    #[serde(flatten)]
    pub edge: FleetEdgeKind,
}

/// Item state inside sequential/concurrency-limited batch environment linking (`8e8ff9b243`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchLinkRowState {
    pub environment_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<PeerLinkFailureKind>,
}

/// Environments Map & P2P batch-linking state (`ccabed086a`, `8e8ff9b243`, `6f644f6e91`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentsMapState {
    pub open: bool,
    pub nodes: Vec<FleetEnvironmentNode>,
    pub edges: Vec<FleetEnvironmentEdge>,
    pub list_state: String,
    pub batch_active: bool,
    pub batch_rows: Vec<BatchLinkRowState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_node_id: Option<String>,
}

/// Surface variant rendered inside the unified right-hand read-only thread pane (`1cb3a8c753`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkedPaneSurfaceMode {
    SideChat,
    Subagent,
    LinkedThread,
}

/// Read-only timeline preview message inside [`LinkedThreadPaneState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedPaneMessage {
    pub id: String,
    pub role: String,
    pub text: String,
}

/// Unified Side/Subagent/Linked Thread read-only pane (`1cb3a8c753`, `7c6e889ddb`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "Serialized view state flags mapped 1:1 to JSON snapshot fields"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedThreadPaneState {
    pub open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<LinkedPaneSurfaceMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<LinkedThreadTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_target: Option<LinkedThreadTarget>,
    pub working: bool,
    pub read_only: bool,
    #[serde(default)]
    pub status_label: String,
    #[serde(default)]
    pub messages: Vec<LinkedPaneMessage>,
    pub has_earlier: bool,
    #[serde(default)]
    pub loading_earlier: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl LinkedThreadPaneState {
    /// Applies [`LinkedThreadViewState`] computed by `t3_client_core`.
    pub fn apply_view_state(&mut self, view: &LinkedThreadViewState) {
        self.working = view.working;
        self.has_earlier = match view.history_status {
            HistoryLoadStatus::Idle | HistoryLoadStatus::Loading => view.has_more_history,
            HistoryLoadStatus::Failed => false,
        };
        self.loading_earlier = matches!(view.history_status, HistoryLoadStatus::Loading);
    }
}

/// Multi-repo picker state and pinned thread repositories (`60863edc41`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiRepoPickerState {
    pub open: bool,
    #[serde(default)]
    pub query: String,
    /// Per-project tracked repository folder paths.
    #[serde(default)]
    pub tracked_folders_by_project: BTreeMap<String, Vec<String>>,
    /// Per-project scanned workspace candidate folders.
    #[serde(default)]
    pub scanned_folders_by_project: BTreeMap<String, Vec<WorkspaceScannedFolder>>,
    /// Per-project selected worktree repositories.
    #[serde(default)]
    pub worktree_repos_by_project: BTreeMap<String, Vec<String>>,
    /// Per-thread pinned repositories.
    #[serde(default)]
    pub pinned_repos_by_thread: BTreeMap<String, Vec<PinnedThreadRepository>>,
    /// Optional task folder override configuration per project.
    #[serde(default)]
    pub task_folders_by_project: BTreeMap<String, Vec<TaskFolder>>,
}

/// Interactive secret request card state wrapped with card identifiers (`0bc9b57b93`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRequestCardEntry {
    pub thread_id: String,
    pub turn_item_id: String,
    pub label: String,
    pub reason: Option<String>,
    pub url: Option<String>,
    pub delivery_file: bool,
    pub card: SecretCardState,
    pub submitted: Option<String>,
}

impl SecretRequestCardEntry {
    /// Builds a card entry initialized from orchestration field metadata.
    #[must_use]
    pub fn from_payload(p: &crate::actions_features::UpsertSecretCardPayload) -> Self {
        Self {
            thread_id: p.thread_id.clone(),
            turn_item_id: p.turn_item_id.clone(),
            label: p.label.clone(),
            reason: p.reason.clone(),
            url: p.url.clone(),
            delivery_file: p.delivery_file,
            card: SecretCardState::new(&p.label, p.placeholder.as_deref(), p.fields.as_deref()),
            submitted: None,
        }
    }
}

/// UI phase of an MCP OAuth sign-in action embedded inside a system notice (`669d8941e2`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpSignInUiStatus {
    #[default]
    Pending,
    Authorizing,
    Completing,
    SignedIn,
    Error,
}

/// UI state of an MCP OAuth sign-in action embedded inside a system notice (`669d8941e2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSignInCardState {
    pub notice_id: String,
    pub thread_id: String,
    #[serde(default)]
    pub provider_thread_id: String,
    pub server: String,
    pub status: McpSignInUiStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Keybinding record searchable in settings and palette (`342b58a79f`, `3d31ffcd9a`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingEntry {
    pub command_id: String,
    pub label: String,
    pub shortcut: String,
    #[serde(default)]
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_expr: Option<String>,
}

/// Context passed into `when` condition evaluation (`composerEmpty`, etc., `342b58a79f`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent keybinding evaluation context flags"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeybindingWhenContext {
    pub composer_empty: bool,
    pub composer_focus: bool,
    pub dictating: bool,
    pub terminal_focus: bool,
    pub diff_open: bool,
    pub modal_open: bool,
}

/// Lifecycle phase of the compact sidebar update capsule countdown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateCapsulePhase {
    #[default]
    Hidden,
    Checking,
    Downloading,
    Providers,
    Servers,
    Countdown,
    Installing,
    Error,
}

/// Compact sidebar update capsule state with countdown timer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCapsuleState {
    pub phase: UpdateCapsulePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub progress_percent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_at_ms: Option<i64>,
    #[serde(default)]
    pub countdown_seconds: u32,
    #[serde(default)]
    pub dismissed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Container stored on [`crate::state::WebAppState`] for Wave 5 UI sub-states.
#[derive(Debug, Clone, Default)]
pub struct WebFeatureState {
    pub next_error_occurrence_id: u64,
    pub thread_errors: BTreeMap<String, ThreadErrorBanner>,
    pub dismissed_runtime_keys: BTreeSet<String>,
    pub environments_map: EnvironmentsMapState,
    pub linked_pane: LinkedThreadPaneState,
    pub repo_picker: MultiRepoPickerState,
    pub secret_cards: IndexMap<String, SecretRequestCardEntry>,
    pub mcp_sign_ins: IndexMap<String, McpSignInCardState>,
    pub keybindings: Vec<KeybindingEntry>,
    pub keybinding_query: String,
    pub terminal_focused: bool,
    pub update_capsule: UpdateCapsuleState,
}
