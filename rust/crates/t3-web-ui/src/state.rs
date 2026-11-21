//! Complete Model-View-Update [`WebAppState`] root.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::Serialize;
use t3_client_core::backoff::{BackoffDecision, ReconnectBackoff};
use t3_client_core::steady_shells::{
    STEADY_SHELL_RELEASE_WINDOW_MS, SteadyShellCoordinator, SteadyShellInput, SteadyShellStep,
};
use t3_client_core::thread_families::{MountHandle, ThreadFamilyCache, ThreadFamilyRow};
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_shared::frame_parts::FramePartAssembler;
use t3_surfaces::mobile_i18n::{AgentActivityInput, AgentLiveActivityPresentation};
use t3_terminal_vt::{AnsiPalette256, TerminalGrid, TerminalSnapshot, VtParser, encode_t3vt};

use crate::actions::RightPaneTab;
use crate::state_snapshot::build_state_snapshot;
use crate::state_types::{
    AuthState, CommandPaletteState, ComposerState, DiffState, LocaleState, ProjectSummary,
    ThreadSummary, ThreadTimeline,
};

pub use crate::state_snapshot::{
    ClientSnapshot, TerminalSnapshotSummary, WebAppStateSnapshot, format_rgba_hex,
};

/// Default WebSocket RPC v2 URL with `orchestrationProtocol=2` and `frameParts=1`.
pub const DEFAULT_WS_URL: &str = "/ws?orchestrationProtocol=2&frameParts=1";
/// Default `VT` terminal grid columns.
pub const DEFAULT_TERMINAL_COLS: u16 = 80;
/// Default `VT` terminal grid rows.
pub const DEFAULT_TERMINAL_ROWS: u16 = 24;

/// High-level WebSocket lifecycle status displayed in the workspace header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    /// Socket is not connected.
    #[default]
    Disconnected,
    /// Socket handshake or initial sync in progress.
    Connecting,
    /// Streaming initial snapshots before `StreamItem::Synchronized`.
    Synchronizing,
    /// Live connected session receiving real-time deltas.
    Live,
    /// Waiting for host pairing approval (`approval-pending`).
    AwaitingApproval,
}

/// Client runtime coordination state wrapping `t3_client_core` and `t3_shared::frame_parts`.
#[derive(Debug, Clone)]
pub struct ClientRuntimeState {
    /// Latest explicit timestamp in milliseconds.
    pub now_ms: u64,
    /// WebSocket endpoint path + query params.
    pub ws_url: String,
    /// Current connection status.
    pub connection_status: ConnectionStatus,
    /// Deterministic jittered reconnect backoff state machine.
    pub reconnect_backoff: ReconnectBackoff,
    /// Most recent reconnect backoff decision, if disconnected.
    pub last_backoff: Option<BackoffDecision>,
    /// Planned timestamp (`now_ms + delay_ms`) for next reconnect attempt.
    pub next_retry_at_ms: Option<u64>,
    /// Timestamp when the active WebSocket connection opened.
    pub connected_since_ms: Option<u64>,
    /// Monotonic client RPC request sequence counter.
    pub next_request_seq: u64,
    /// Reassembler for incoming `\u{001e}`-prefixed `?frameParts` WebSocket chunks (`BUG-21`).
    pub frame_assembler: FramePartAssembler,
    /// Mount-scoped steady-shell release coordinator (`BUG-21`).
    pub steady_shells: SteadyShellCoordinator<Vec<ThreadSummary>>,
    /// Latest steady-shell inputs keyed by environment ID.
    pub steady_inputs: BTreeMap<EnvironmentId, SteadyShellInput<Vec<ThreadSummary>>>,
    /// Latest steady-shell step summary.
    pub last_steady_step: Option<SteadyShellStep>,
    /// Generation-keyed, ref-counted thread-family cache.
    pub thread_families: ThreadFamilyCache<ThreadFamilyRow<ThreadSummary>>,
    /// Active mount handles keyed by `(environment_id, thread_id)`.
    pub family_handles: BTreeMap<(EnvironmentId, ThreadId), Vec<MountHandle>>,
    /// Active environment ID used for local client coordination.
    pub environment_id: Option<EnvironmentId>,
}

impl ClientRuntimeState {
    /// Creates an initialized client runtime state at `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        let mut steady_shells = SteadyShellCoordinator::new(STEADY_SHELL_RELEASE_WINDOW_MS);
        steady_shells.mount();
        Self {
            now_ms,
            ws_url: DEFAULT_WS_URL.to_owned(),
            connection_status: ConnectionStatus::Disconnected,
            reconnect_backoff: ReconnectBackoff::new(),
            last_backoff: None,
            next_retry_at_ms: None,
            connected_since_ms: None,
            next_request_seq: 1,
            frame_assembler: FramePartAssembler::new(),
            steady_shells,
            steady_inputs: BTreeMap::new(),
            last_steady_step: None,
            thread_families: ThreadFamilyCache::default(),
            family_handles: BTreeMap::new(),
            environment_id: EnvironmentId::new("env-local").ok(),
        }
    }

    /// Allocates and increments the monotonic RPC request ID counter.
    pub const fn alloc_request_id(&mut self) -> u64 {
        let id = self.next_request_seq;
        self.next_request_seq = self.next_request_seq.saturating_add(1);
        id
    }
}

/// Embedded `VT` terminal parser, grid, sparse 256-color palette (`BUG-25`), and `"T3VT"` codec.
#[derive(Debug, Clone)]
pub struct TerminalState {
    /// Streaming `UTF-8` / CSI / OSC parser.
    pub vt_parser: VtParser,
    /// Bounded terminal cell buffer and scrollback.
    pub grid: TerminalGrid,
    /// Active sparse 256-color ANSI palette (`BUG-25`).
    pub palette: AnsiPalette256,
    /// Optional decoded `"T3VT"` snapshot overriding live grid display until next `VT` feed/reset.
    pub imported_snapshot: Option<TerminalSnapshot>,
    /// Latest sanitized terminal paste draft.
    pub input_draft: String,
    /// Latest exported `"T3VT"` v2 binary bytes.
    pub last_exported_t3vt: Option<Vec<u8>>,
    /// Last `"T3VT"` import or color-parse error message.
    pub snapshot_error: Option<String>,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            vt_parser: VtParser::new(),
            grid: TerminalGrid::new(DEFAULT_TERMINAL_COLS, DEFAULT_TERMINAL_ROWS),
            palette: AnsiPalette256::new(),
            imported_snapshot: None,
            input_draft: String::new(),
            last_exported_t3vt: None,
            snapshot_error: None,
        }
    }
}

impl TerminalState {
    /// Returns the active [`TerminalSnapshot`] (imported `"T3VT"` snapshot if active, else live grid).
    pub fn active_snapshot(&self) -> TerminalSnapshot {
        self.imported_snapshot
            .clone()
            .unwrap_or_else(|| self.grid.snapshot())
    }

    /// Encodes the active snapshot into `"T3VT"` v2 binary bytes.
    pub fn export_t3vt_bytes(&self) -> Vec<u8> {
        encode_t3vt(&self.active_snapshot())
    }

    /// Returns trimmed viewport lines joined by `\n` for snapshot/JSON summaries.
    pub fn viewport_text(&self) -> String {
        if let Some(ref snap) = self.imported_snapshot {
            let cols = usize::from(snap.cols).max(1);
            let mut lines = Vec::with_capacity(usize::from(snap.rows));
            for row in snap.cells.chunks(cols) {
                let mut line = String::new();
                for cell in row {
                    if cell.text.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&cell.text);
                    }
                }
                lines.push(line.trim_end().to_owned());
            }
            return lines.join("\n");
        }
        self.grid.viewport_text()
    }
}

/// Complete Model-View-Update state owned by [`crate::update::WebAppEngine`].
#[derive(Debug, Clone)]
pub struct WebAppState {
    /// Active locale state across all 10 `t3_edge::i18n` locales (`BUG-27`, `BUG-24`).
    pub locale: LocaleState,
    /// Client connection, backoff, steady-shell (`BUG-21`), and thread-family state.
    pub client: ClientRuntimeState,
    /// Authentication and pairing state.
    pub auth: AuthState,
    /// Known workspace projects.
    pub projects: Vec<ProjectSummary>,
    /// Currently selected project ID.
    pub active_project_id: Option<String>,
    /// Create-project form draft workspace path.
    pub new_project_workspace_root: String,
    /// Create-project form draft title.
    pub new_project_title: String,
    /// Ordered thread summaries keyed by thread ID.
    pub threads: IndexMap<String, ThreadSummary>,
    /// Currently selected thread ID.
    pub active_thread_id: Option<String>,
    /// Per-thread projection timelines populated from `StreamItem`s.
    pub timelines: BTreeMap<String, ThreadTimeline>,
    /// Live Activity badge summary input (`BUG-24`).
    pub activity_input: AgentActivityInput,
    /// Prompt composer draft and context pills (`BUG-23`).
    pub composer: ComposerState,
    /// Parsed & decorated unified diff review state (`BUG-22`).
    pub diff: DiffState,
    /// Embedded `VT` terminal grid, sparse palette (`BUG-25`), and `"T3VT"` state.
    pub terminal: TerminalState,
    /// `Ctrl+K` command palette state (`t3_shared::search`).
    pub command_palette: CommandPaletteState,
    /// Currently visible right-pane tab.
    pub right_tab: RightPaneTab,
    /// Responsive sidebar drawer visibility.
    pub sidebar_open: bool,
    /// Wave 5 UI feature states (errors, Environments Map, linked pane, repos, secrets, MCP, updates).
    pub features: crate::state_features::WebFeatureState,
}

impl WebAppState {
    /// Initializes workspace state from a browser/OS `locale_hint` and explicit timestamp `now_ms`.
    pub fn new(locale_hint: &str, now_ms: u64) -> Self {
        let locale = if locale_hint.trim().is_empty() {
            LocaleState::resolve("en", &[])
        } else {
            LocaleState::resolve("system", &[locale_hint])
        };
        Self {
            locale,
            client: ClientRuntimeState::new(now_ms),
            auth: AuthState::default(),
            projects: Vec::new(),
            active_project_id: None,
            new_project_workspace_root: String::new(),
            new_project_title: String::new(),
            threads: IndexMap::new(),
            active_thread_id: None,
            timelines: BTreeMap::new(),
            activity_input: AgentActivityInput::default(),
            composer: ComposerState::default(),
            diff: DiffState::default(),
            terminal: TerminalState::default(),
            command_palette: CommandPaletteState::default(),
            right_tab: RightPaneTab::Chat,
            sidebar_open: true,
            features: crate::state_features::WebFeatureState::default(),
        }
    }

    /// Returns the localized Live Activity presentation derived from `self.activity_input`.
    pub fn localized_activity(&self) -> AgentLiveActivityPresentation {
        self.locale.present_activity(&self.activity_input)
    }

    /// Materializes a JSON-serializable snapshot of the workspace state.
    pub fn snapshot(&self) -> WebAppStateSnapshot {
        build_state_snapshot(self)
    }
}
