//! Typed Model-View-Update actions dispatched to [`crate::update::WebAppEngine`].

use legion_client_core::diff_markdown::Utf16Span;
use legion_contracts::orchestration::StreamItem;
use legion_surfaces::mobile_diff::SyntaxTokenInput;
use serde::{Deserialize, Serialize};

pub use crate::actions_types::{
    PaletteOverrideInput, ProjectInput, RightPaneTab, SteadyShellBatchItem, ThreadFamilyOp,
    ThreadSummaryInput,
};

/// Every action accepted by [`crate::update::WebAppEngine::dispatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WebAppAction {
    /// Switch active locale across all 10 `legion_edge::i18n` locales (`BUG-27`, `BUG-24`).
    #[serde(rename_all = "camelCase")]
    SetLocale {
        language: String,
        #[serde(default)]
        system_locales: Vec<String>,
    },
    /// Toggle or explicitly set sidebar visibility.
    #[serde(rename_all = "camelCase")]
    ToggleSidebar {
        #[serde(default)]
        open: Option<bool>,
    },
    /// Switch active right-pane tab (`chat`, `diff`, `terminal`).
    #[serde(rename_all = "camelCase")]
    SetRightTab { tab: RightPaneTab },
    /// Update the pairing token input field.
    #[serde(rename_all = "camelCase")]
    SetPairingInput { value: String },
    /// Submit `/api/auth/pair` using the current pairing input.
    #[serde(rename_all = "camelCase")]
    SubmitPairing {
        #[serde(default)]
        client_label: Option<String>,
    },
    /// Apply successful `/api/auth/pair` bearer session response.
    #[serde(rename_all = "camelCase")]
    ApplyPairingSuccess {
        #[serde(alias = "access_token")]
        access_token: String,
        #[serde(default, alias = "session_id")]
        session_id: Option<String>,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        scopes: Vec<String>,
    },
    /// Record an authentication or HTTP error banner.
    #[serde(rename_all = "camelCase")]
    ApplyAuthError { message: String },
    /// Restore a previously persisted bearer session token from `sessionStorage`.
    #[serde(rename_all = "camelCase")]
    RestoreSessionToken { token: String },
    /// Clear session credentials and emit empty `PersistSessionToken`.
    SignOut,
    /// Request `GET /api/projects`.
    FetchProjects,
    /// Apply `/api/projects` or `/api/projects/mutate` snapshot payload.
    #[serde(rename_all = "camelCase")]
    ApplyProjectsSnapshot { projects: Vec<ProjectInput> },
    /// Select a workspace project by ID.
    #[serde(rename_all = "camelCase")]
    SelectProject { project_id: String },
    /// Update the create-project form draft fields.
    #[serde(rename_all = "camelCase")]
    SetProjectDraft {
        #[serde(default)]
        workspace_root: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    /// Emit `POST /api/projects/mutate` (`project.create`).
    #[serde(rename_all = "camelCase")]
    CreateProject {
        workspace_root: String,
        #[serde(default)]
        title: Option<String>,
    },
    /// Emit `POST /api/projects/mutate` (`project.update`) renaming a project.
    #[serde(rename_all = "camelCase")]
    RenameProject { project_id: String, title: String },
    /// Emit `POST /api/projects/mutate` (`project.delete`).
    #[serde(rename_all = "camelCase")]
    DeleteProject {
        project_id: String,
        #[serde(default)]
        force: bool,
    },
    /// Mark WebSocket connection established at `now_ms`.
    #[serde(rename_all = "camelCase")]
    WsConnected { now_ms: u64 },
    /// Record WebSocket disconnect and compute reconnect backoff via `legion_client_core`.
    #[serde(rename_all = "camelCase")]
    WsDisconnected {
        now_ms: u64,
        #[serde(default)]
        approval_pending: bool,
        #[serde(default = "default_midpoint")]
        random: f64,
    },
    /// Advance `SteadyShellCoordinator` (`BUG-21`) with environment shell snapshots.
    #[serde(rename_all = "camelCase")]
    StepSteadyShells {
        now_ms: u64,
        inputs: Vec<SteadyShellBatchItem>,
    },
    /// Poll or fire the steady-shell release timer at `now_ms`.
    #[serde(rename_all = "camelCase")]
    PollSteadyShells {
        now_ms: u64,
        #[serde(default)]
        timer_token: Option<u64>,
    },
    /// Exercise `ThreadFamilyCache` for a thread family.
    #[serde(rename_all = "camelCase")]
    TouchThreadFamily {
        environment_id: String,
        thread_id: String,
        op: ThreadFamilyOp,
        #[serde(default)]
        now_ms: Option<u64>,
    },
    /// Feed a raw incoming WebSocket text frame through `FramePartAssembler` (`BUG-21`).
    #[serde(rename_all = "camelCase")]
    ReceiveWsTextFrame {
        raw_frame: String,
        #[serde(default)]
        now_ms: Option<u64>,
    },
    /// Apply a typed `legion_contracts::orchestration::StreamItem`.
    #[serde(rename_all = "camelCase")]
    ReceiveStreamItem {
        #[serde(default)]
        thread_id: Option<String>,
        item: StreamItem,
        #[serde(default)]
        now_ms: Option<u64>,
    },
    /// Select an active thread in the sidebar.
    #[serde(rename_all = "camelCase")]
    SelectThread { thread_id: String },
    /// Create a thread and emit `thread.create` RPC frame parts.
    #[serde(rename_all = "camelCase")]
    CreateThread {
        project_id: String,
        title: String,
        #[serde(default)]
        model: Option<String>,
    },
    /// Update composer prompt draft text.
    #[serde(rename_all = "camelCase")]
    UpdateComposerDraft { text: String },
    /// Sanitize pasted text and filter clipboard context records (`BUG-23`).
    #[serde(rename_all = "camelCase")]
    PasteIntoComposer {
        raw_text: String,
        #[serde(default)]
        clipboard_fragment_json: Option<String>,
    },
    /// Add a context pill normalized via `ComposerClipboardContext` (`BUG-23`).
    #[serde(rename_all = "camelCase")]
    AddContextPill {
        #[serde(default)]
        context_id: Option<String>,
        #[serde(default)]
        screenshot_context_id: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    /// Remove a context pill by index.
    #[serde(rename_all = "camelCase")]
    RemoveContextPill { index: usize },
    /// Send the current composer draft as `message.dispatch` split via `?frameParts` (`BUG-21`).
    SendMessage,
    /// Parse a unified git diff and decorate `UTF-16` word-diff runs (`BUG-22`).
    #[serde(rename_all = "camelCase")]
    LoadUnifiedDiff {
        patch: String,
        #[serde(default = "default_true")]
        switch_tab: bool,
    },
    /// Decorate pre-tokenized syntax runs against `UTF-16` word-diff spans (`BUG-22`).
    #[serde(rename_all = "camelCase")]
    DecorateDiffTokens {
        file_name: String,
        tokens: Vec<SyntaxTokenInput>,
        word_highlights: Vec<Utf16Span>,
        #[serde(default)]
        highlight_background: Option<String>,
    },
    /// Select a parsed diff file tab by index.
    #[serde(rename_all = "camelCase")]
    SelectDiffFile { index: usize },
    /// Feed `UTF-8` / ANSI / OSC text into `VtParser` + `TerminalGrid`.
    #[serde(rename_all = "camelCase")]
    TerminalFeed { text: String },
    /// Feed raw PTY bytes into `VtParser` + `TerminalGrid`.
    #[serde(rename_all = "camelCase")]
    TerminalFeedBytes { bytes: Vec<u8> },
    /// Resize the `VT` terminal grid (`1..=1024 x 1..=512`).
    #[serde(rename_all = "camelCase")]
    TerminalResize { cols: u16, rows: u16 },
    /// Apply Ghostty theme text or sparse 256-color palette overrides (`BUG-25`).
    #[serde(rename_all = "camelCase")]
    TerminalApplyTheme {
        #[serde(default)]
        config_text: Option<String>,
        #[serde(default)]
        overrides: Vec<PaletteOverrideInput>,
        #[serde(default)]
        foreground: Option<String>,
        #[serde(default)]
        background: Option<String>,
        #[serde(default)]
        cursor_color: Option<String>,
    },
    /// Sanitize clipboard text via `legion_terminal_vt::paste` and emit `terminal.write` RPC frames.
    #[serde(rename_all = "camelCase")]
    TerminalPaste { raw_text: String },
    /// Export the current `VT` viewport as a CRC-32 checked `"LCVT"` v2 binary snapshot.
    TerminalExportSnapshot,
    /// Import a `"LCVT"` v1/v2 binary snapshot into the terminal panel.
    #[serde(rename_all = "camelCase")]
    TerminalImportSnapshot { bytes: Vec<u8> },
    /// Reset the `VT` terminal parser, grid, and imported snapshot.
    TerminalReset,
    /// Open the `Ctrl+K` command palette modal and focus its search input.
    OpenCommandPalette,
    /// Close the `Ctrl+K` command palette modal.
    CloseCommandPalette,
    /// Toggle the `Ctrl+K` command palette modal.
    ToggleCommandPalette,
    /// Update the command palette search query using `legion_shared::search::FuzzyPaletteIndex`.
    #[serde(rename_all = "camelCase")]
    SetCommandPaletteQuery { query: String },
    /// Execute the highlighted or indexed command palette match.
    #[serde(rename_all = "camelCase")]
    ExecuteCommandPaletteItem {
        #[serde(default)]
        index: Option<usize>,
    },
    /// Records a repeatable thread error occurrence banner (`5872b0a488`, `aa6fe390db`).
    RecordThreadError(crate::actions_features::RecordThreadErrorPayload),
    /// Dismisses a thread error banner by monotonic occurrence ID.
    DismissThreadError(crate::actions_features::DismissThreadErrorPayload),
    /// Updates Environments Map topology (routes + P2P links, NO cloud relay).
    SetEnvironmentsFleet(crate::actions_features::SetEnvironmentsFleetPayload),
    /// Opens or closes the Environments Map panel.
    #[serde(rename_all = "camelCase")]
    SetEnvironmentsMapOpen { open: bool },
    /// Starts concurrency-capped batch P2P linking (`8e8ff9b243`).
    #[serde(rename_all = "camelCase")]
    StartBatchLink {
        #[serde(default)]
        candidate_ids: Vec<String>,
    },
    /// Advances batch link status for one environment without halting siblings.
    AdvanceBatchLink(crate::actions_features::AdvanceBatchLinkPayload),
    /// Opens or switches the unified read-only right-hand thread pane (`1cb3a8c753`).
    OpenLinkedThreadPane(crate::actions_features::OpenLinkedThreadPanePayload),
    /// Synchronizes working/history/error status on the open linked-thread pane.
    SyncLinkedThreadPane(crate::actions_features::SyncLinkedThreadPanePayload),
    /// Closes the unified read-only linked-thread pane.
    CloseLinkedThreadPane,
    /// Reopens the previously closed linked-thread pane.
    ReopenLinkedThreadPane,
    /// Opens or closes the multi-repo project picker (`60863edc41`).
    #[serde(rename_all = "camelCase")]
    SetRepoPickerOpen { open: bool },
    /// Sets tracked/scanned folders and worktree repository defaults for a project.
    SetProjectRepositories(crate::actions_features::SetProjectRepositoriesPayload),
    /// Toggles tracking a repository folder on a project (`60863edc41`).
    ToggleProjectRepository(crate::actions_features::ProjectRepoFolderPayload),
    /// Pins or unpins a repository in a thread's repo switcher.
    TogglePinThreadRepository(crate::actions_features::ThreadRepoPinPayload),
    /// Registers or updates a multi-value secret request card (`0bc9b57b93`).
    UpsertSecretCard(crate::actions_features::UpsertSecretCardPayload),
    /// Updates input value on a secret request card field.
    SetSecretFieldValue(crate::actions_features::SecretFieldValuePayload),
    /// Toggles show/hide mask state on a revealable secret field.
    ToggleSecretFieldReveal(crate::actions_features::SecretFieldKeyPayload),
    /// Saves or declines a secret request card (`secrets.answerRequest`).
    AnswerSecretCard(crate::actions_features::AnswerSecretCardPayload),
    /// Registers an MCP OAuth sign-in notice (`669d8941e2`).
    UpsertMcpSignIn(crate::actions_features::UpsertMcpSignInPayload),
    /// Starts MCP OAuth sign-in (`provider.mcpSignIn.start`).
    StartMcpSignIn(crate::actions_features::StartMcpSignInPayload),
    /// Completes MCP OAuth sign-in callback (`provider.mcpSignIn.complete`).
    CompleteMcpSignIn(crate::actions_features::CompleteMcpSignInPayload),
    /// Applies MCP OAuth sign-in completion, cancellation, or error status.
    ResolveMcpSignIn(crate::actions_features::ResolveMcpSignInPayload),
    /// Locks or unlocks composer text input during dictation while keeping chip removal active (`b19ca97827`).
    #[serde(rename_all = "camelCase")]
    SetDictationActive { active: bool },
    /// Replaces keybindings list evaluated against `when` conditions (`342b58a79f`).
    SetKeybindings(crate::actions_features::KeybindingListPayload),
    /// Filters keybinding entries in settings/palette (`3d31ffcd9a`).
    #[serde(rename_all = "camelCase")]
    SearchKeybindings { query: String },
    /// Sets compact sidebar update capsule state and computes countdown seconds.
    SetUpdateCapsule(crate::actions_features::SetUpdateCapsulePayload),
    /// Advances update countdown timer against `now_ms`.
    #[serde(rename_all = "camelCase")]
    TickUpdateCountdown { now_ms: i64 },
    /// Postpones/dismisses the update countdown (`Later`).
    PostponeUpdateCapsule,
}

const fn default_true() -> bool {
    true
}

const fn default_midpoint() -> f64 {
    0.5
}
