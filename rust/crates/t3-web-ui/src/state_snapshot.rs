//! Serializable state snapshot structures emitted by [`crate::state::WebAppState::snapshot`].

use std::collections::BTreeMap;

use serde::Serialize;
use t3_client_core::backoff::BackoffDecision;
use t3_surfaces::mobile_i18n::AgentLiveActivityPresentation;
use t3_terminal_vt::Rgba;

use crate::actions::RightPaneTab;
use crate::state::{ConnectionStatus, WebAppState};
use crate::state_types::{
    AuthSnapshot, CommandPaletteState, ComposerState, DiffState, LocaleState, ProjectSummary,
    ThreadSummary, ThreadTimeline,
};

/// Formats an [`Rgba`] color as `#rrggbb` or `#rrggbbaa`.
pub fn format_rgba_hex(color: Rgba) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color.r, color.g, color.b, color.a
        )
    }
}

/// Serializable client state summary included in [`WebAppStateSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSnapshot {
    /// Latest explicit timestamp in milliseconds.
    pub now_ms: u64,
    /// WebSocket URL.
    pub ws_url: String,
    /// Connection lifecycle status.
    pub connection_status: ConnectionStatus,
    /// Total reconnect failures since reset.
    pub failure_count: u32,
    /// Exponential backoff steps consumed.
    pub backoff_failures: u32,
    /// Next scheduled retry timestamp in milliseconds.
    pub next_retry_at_ms: Option<u64>,
    /// Latest reconnect backoff decision.
    pub last_backoff: Option<BackoffDecision>,
    /// Whether steady-shell coordinator is holding newer rows (`BUG-21`).
    pub steady_shell_holding: bool,
    /// Pending steady-shell release deadline.
    pub steady_release_at_ms: Option<u64>,
    /// Active shell generation for the local environment.
    pub shell_generation: u64,
    /// Cached thread family entry count.
    pub cached_family_count: usize,
    /// Number of threads currently released/shown by steady shells.
    pub shown_thread_count: usize,
}

/// Serializable `VT` terminal summary included in [`WebAppStateSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshotSummary {
    /// Grid columns.
    pub cols: u16,
    /// Grid rows.
    pub rows: u16,
    /// Cursor `X` coordinate.
    pub cursor_x: u16,
    /// Cursor `Y` coordinate.
    pub cursor_y: u16,
    /// Cursor visibility flag.
    pub cursor_visible: bool,
    /// Alternate screen mode flag.
    pub alternate_screen: bool,
    /// Window title (`OSC 0`/`OSC 2`).
    pub title: String,
    /// Working directory (`OSC 7`).
    pub pwd: String,
    /// Bell counter.
    pub bell_count: u64,
    /// Retained scrollback lines.
    pub scrollback_len: usize,
    /// Effective foreground hex color.
    pub foreground: String,
    /// Effective background hex color.
    pub background: String,
    /// Sparse 256-color palette overrides (`BUG-25`).
    pub palette_overrides: Vec<(u8, String)>,
    /// Visible viewport text lines joined by `\n`.
    pub viewport_text: String,
    /// Byte length of the most recent `"T3VT"` binary export, if any.
    pub last_exported_t3vt_len: Option<usize>,
    /// Most recent `"T3VT"` or theme error message, if any.
    pub snapshot_error: Option<String>,
}

/// Full JSON-serializable state snapshot returned by [`crate::update::WebAppEngine::state_snapshot_json`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAppStateSnapshot {
    /// Active locale catalogs (`t3_edge::i18n` + `t3_surfaces::mobile_i18n`).
    pub locale: LocaleState,
    /// Client runtime coordination metrics.
    pub client: ClientSnapshot,
    /// Authentication summary.
    pub auth: AuthSnapshot,
    /// Loaded workspace projects.
    pub projects: Vec<ProjectSummary>,
    /// Active project ID.
    pub active_project_id: Option<String>,
    /// Loaded threads.
    pub threads: Vec<ThreadSummary>,
    /// Active thread ID.
    pub active_thread_id: Option<String>,
    /// Per-thread projection timelines.
    pub timelines: BTreeMap<String, ThreadTimeline>,
    /// Localized Live Activity summary (`BUG-24`).
    pub activity: AgentLiveActivityPresentation,
    /// Composer draft and context pills (`BUG-23`).
    pub composer: ComposerState,
    /// Decorated diff review state (`BUG-22`).
    pub diff: DiffState,
    /// Terminal `VT` grid and sparse palette summary (`BUG-25`).
    pub terminal: TerminalSnapshotSummary,
    /// Command palette modal state (`t3_shared::search`).
    pub command_palette: CommandPaletteState,
    /// Active right-pane tab.
    pub right_tab: RightPaneTab,
    /// Responsive sidebar visibility.
    pub sidebar_open: bool,
    /// Wave 5 feature state summary observables.
    pub features: WebFeatureSnapshotSummary,
}

/// JSON-serializable summary of Wave 5 feature states (`state_snapshot_json`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebFeatureSnapshotSummary {
    pub active_error_occurrence_id: Option<u64>,
    pub environments_map_open: bool,
    pub environment_node_count: usize,
    pub linked_pane_open: bool,
    pub secret_card_count: usize,
    pub mcp_sign_in_count: usize,
    pub keybinding_match_count: usize,
    pub update_capsule: crate::state_features::UpdateCapsuleState,
}

pub(crate) fn build_state_snapshot(state: &WebAppState) -> WebAppStateSnapshot {
    let term_snap = state.terminal.active_snapshot();
    let palette_overrides: Vec<(u8, String)> = term_snap
        .palette
        .override_pairs()
        .into_iter()
        .map(|(idx, rgba)| (idx, format_rgba_hex(rgba)))
        .collect();
    let shell_generation = state
        .client
        .environment_id
        .as_ref()
        .map_or(0, |env| state.client.thread_families.shell_generation(env));
    let steady_inputs: Vec<_> = state.client.steady_inputs.values().cloned().collect();
    let shown_thread_count = state
        .client
        .environment_id
        .as_ref()
        .and_then(|env| state.client.steady_shells.shown(env))
        .map_or(0, Vec::len);

    WebAppStateSnapshot {
        locale: state.locale.clone(),
        client: ClientSnapshot {
            now_ms: state.client.now_ms,
            ws_url: state.client.ws_url.clone(),
            connection_status: state.client.connection_status,
            failure_count: state.client.reconnect_backoff.failure_count(),
            backoff_failures: state.client.reconnect_backoff.backoff_failures(),
            next_retry_at_ms: state.client.next_retry_at_ms,
            last_backoff: state.client.last_backoff,
            steady_shell_holding: state.client.steady_shells.is_holding(&steady_inputs),
            steady_release_at_ms: state.client.steady_shells.release_at_ms(),
            shell_generation,
            cached_family_count: state.client.thread_families.cached_entry_count(),
            shown_thread_count,
        },
        auth: AuthSnapshot {
            paired: state.auth.paired,
            has_bearer_token: state.auth.bearer_token.is_some(),
            session_id: state.auth.session_id.clone(),
            role: state.auth.role.clone(),
            scopes: state.auth.scopes.clone(),
            pairing_input: state.auth.pairing_input.clone(),
            last_error: state.auth.last_error.clone(),
        },
        projects: state.projects.clone(),
        active_project_id: state.active_project_id.clone(),
        threads: state.threads.values().cloned().collect(),
        active_thread_id: state.active_thread_id.clone(),
        timelines: state.timelines.clone(),
        activity: state.localized_activity(),
        composer: state.composer.clone(),
        diff: state.diff.clone(),
        terminal: TerminalSnapshotSummary {
            cols: term_snap.cols,
            rows: term_snap.rows,
            cursor_x: term_snap.cursor_x,
            cursor_y: term_snap.cursor_y,
            cursor_visible: term_snap.cursor_visible,
            alternate_screen: term_snap.alternate_screen,
            title: state.terminal.grid.title().to_owned(),
            pwd: state.terminal.grid.pwd().to_owned(),
            bell_count: state.terminal.grid.bell_count(),
            scrollback_len: state.terminal.grid.scrollback_len(),
            foreground: format_rgba_hex(term_snap.foreground),
            background: format_rgba_hex(term_snap.background),
            palette_overrides,
            viewport_text: state.terminal.viewport_text(),
            last_exported_t3vt_len: state.terminal.last_exported_t3vt.as_ref().map(Vec::len),
            snapshot_error: state.terminal.snapshot_error.clone(),
        },
        command_palette: state.command_palette.clone(),
        right_tab: state.right_tab,
        sidebar_open: state.sidebar_open,
        features: WebFeatureSnapshotSummary {
            active_error_occurrence_id: state
                .active_thread_id
                .as_deref()
                .and_then(|tid| state.features.thread_errors.get(tid))
                .map(|b| b.occurrence_id),
            environments_map_open: state.features.environments_map.open,
            environment_node_count: state.features.environments_map.nodes.len(),
            linked_pane_open: state.features.linked_pane.open,
            secret_card_count: state.features.secret_cards.len(),
            mcp_sign_in_count: state.features.mcp_sign_ins.len(),
            keybinding_match_count: crate::update_settings_features::filter_keybindings(
                &state.features.keybindings,
                &state.features.keybinding_query,
            )
            .len(),
            update_capsule: state.features.update_capsule.clone(),
        },
    }
}
