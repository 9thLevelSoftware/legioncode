//! Per-session terminal VT grid, palette, streaming history, and child PTY/process attachment.

pub mod process;
pub mod shell;
pub mod stream;
pub mod stream_types;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use legion_protocol::provider_env::{
    TerminalResumeStatus, TerminalSessionKey, TerminalSessionKeys,
};
use legion_terminal_vt::paste::encode_paste;
use legion_terminal_vt::{Rgba, TerminalGrid, VtParser, sanitize_bracketed_paste};
use tokio::sync::Notify;

use crate::error::ServerRuntimeError;

pub use process::TerminalProcessMode;
pub(crate) use process::{TerminalChildHandle, TerminalSessionSinks};
pub use shell::{resolve_default_shell, resolve_default_shell_candidates};
use stream::TerminalStreamState;
pub use stream_types::{
    DEFAULT_MAX_TERMINAL_BUFFER_BYTES, DEFAULT_STREAM_CHANNEL_CAPACITY,
    TerminalMetadataStreamEvent, TerminalOutputSubscription, TerminalSessionLifecycleStatus,
    TerminalSessionStateSnapshot, TerminalSessionSummary, TerminalStreamChunk, retain_history_tail,
};

/// Result returned by [`crate::TerminalRuntime::terminal_open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOpenResult {
    /// Canonical session key.
    pub key: TerminalSessionKey,
    /// Whether an existing or LRU-evicted session was resumed.
    pub resumed: bool,
    /// Whether the session had previously been LRU-evicted prior to this open.
    pub was_evicted: bool,
    /// Effective terminal column count.
    pub cols: u16,
    /// Effective terminal row count.
    pub rows: u16,
    /// Current lifecycle status after open.
    pub status: TerminalResumeStatus,
}

/// Snapshot output produced by [`crate::TerminalRuntime::terminal_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshotResult {
    /// Binary `"LCVT"` wire snapshot frame encoded via `legion_terminal_vt::encode_lcvt`.
    pub lcvt_bytes: Vec<u8>,
    /// Current grid columns.
    pub cols: u16,
    /// Current grid rows.
    pub rows: u16,
    /// Current cursor X coordinate.
    pub cursor_x: u16,
    /// Current cursor Y coordinate.
    pub cursor_y: u16,
    /// Current visible viewport text.
    pub viewport_text: String,
}

/// Configuration for opening or resuming a live terminal session.
#[derive(Debug, Clone)]
pub struct TerminalOpenRequest {
    /// Owning thread ID.
    pub thread_id: String,
    /// Stable terminal ID within the thread.
    pub terminal_id: String,
    /// Initial column count (> 0).
    pub cols: u16,
    /// Initial row count (> 0).
    pub rows: u16,
    /// Optional working directory for a spawned child process.
    pub cwd: Option<PathBuf>,
    /// Optional git worktree root directory reported in session metadata.
    pub worktree_path: Option<PathBuf>,
    /// Whether this call is attempting to resume a prior session (`BUG-08`).
    pub resume: bool,
    /// Optional OS executable/shell command to attach to this session.
    pub command: Option<OsString>,
    /// Optional CLI arguments for `command`.
    pub args: Vec<OsString>,
    /// Explicit environment variable overrides layered onto `SanitizedChildEnv`.
    pub env_overrides: BTreeMap<String, String>,
    /// Child attachment mode (headless/piped compatibility vs interactive OS PTY).
    pub process_mode: TerminalProcessMode,
    /// Maximum UTF-8 history bytes retained for scrollback replay.
    pub max_history_bytes: usize,
    /// Optional initial terminal tab label.
    pub label: Option<String>,
}

impl TerminalOpenRequest {
    /// Construct a new terminal open request preserving headless/piped compatibility defaults.
    pub fn new(
        thread_id: impl Into<String>,
        terminal_id: impl Into<String>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            terminal_id: terminal_id.into(),
            cols,
            rows,
            cwd: None,
            worktree_path: None,
            resume: false,
            command: None,
            args: Vec::new(),
            env_overrides: BTreeMap::new(),
            process_mode: TerminalProcessMode::HeadlessOrPiped,
            max_history_bytes: DEFAULT_MAX_TERMINAL_BUFFER_BYTES,
            label: None,
        }
    }

    /// Construct a new interactive OS PTY terminal open request (spawning default shell when `command` is `None`).
    pub fn interactive(
        thread_id: impl Into<String>,
        terminal_id: impl Into<String>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self::new(thread_id, terminal_id, cols, rows).with_pty(true)
    }

    /// Alias for [`Self::interactive`] resolving the OS default shell via PTY.
    pub fn default_shell(
        thread_id: impl Into<String>,
        terminal_id: impl Into<String>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self::interactive(thread_id, terminal_id, cols, rows)
    }

    /// Toggles interactive OS PTY attachment mode.
    #[must_use]
    pub fn with_pty(mut self, interactive_pty: bool) -> Self {
        self.process_mode = if interactive_pty {
            TerminalProcessMode::InteractivePty
        } else {
            TerminalProcessMode::HeadlessOrPiped
        };
        self
    }

    /// Sets the terminal process attachment mode explicitly.
    #[must_use]
    pub const fn with_process_mode(mut self, mode: TerminalProcessMode) -> Self {
        self.process_mode = mode;
        self
    }

    /// Sets the child working directory.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets the associated git worktree path for metadata snapshots.
    #[must_use]
    pub fn with_worktree_path(mut self, worktree_path: impl Into<PathBuf>) -> Self {
        self.worktree_path = Some(worktree_path.into());
        self
    }

    /// Sets the maximum retained scrollback history size in bytes.
    #[must_use]
    pub const fn with_max_history_bytes(mut self, max_history_bytes: usize) -> Self {
        self.max_history_bytes = max_history_bytes;
        self
    }

    /// Sets an initial display label for the terminal session.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

pub(crate) struct TerminalSessionEntry {
    pub grid: Arc<StdMutex<TerminalGrid>>,
    pub parser: Arc<StdMutex<VtParser>>,
    pub cwd: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub palette_overrides: Vec<(u8, Rgba)>,
    pub process: Option<TerminalChildHandle>,
    pub output_notify: Arc<Notify>,
    pub stream: Arc<StdMutex<TerminalStreamState>>,
}

#[derive(Debug, Clone)]
pub(crate) struct EvictedSessionMeta {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub palette_overrides: Vec<(u8, Rgba)>,
}

pub(crate) fn build_opened_entry(
    req: &TerminalOpenRequest,
    meta: Option<EvictedSessionMeta>,
    metadata_tx: tokio::sync::broadcast::Sender<TerminalMetadataStreamEvent>,
) -> Result<(u16, u16, TerminalSessionEntry), ServerRuntimeError> {
    let cols = meta.as_ref().map_or(req.cols, |m| m.cols);
    let rows = meta.as_ref().map_or(req.rows, |m| m.rows);
    let cwd = req
        .cwd
        .clone()
        .or_else(|| meta.as_ref().and_then(|m| m.cwd.clone()));
    let worktree_path = req
        .worktree_path
        .clone()
        .or_else(|| meta.as_ref().and_then(|m| m.worktree_path.clone()));
    let palette_overrides = meta.map_or_else(Vec::new, |m| m.palette_overrides);

    let mut grid_val = TerminalGrid::try_new(cols, rows)?;
    if !palette_overrides.is_empty() {
        grid_val
            .palette_mut()
            .apply_theme_overrides(&palette_overrides);
    }
    let grid = Arc::new(StdMutex::new(grid_val));
    let parser = Arc::new(StdMutex::new(VtParser::new()));
    let output_notify = Arc::new(Notify::new());
    let cwd_str = cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .map_or_else(|| ".".to_owned(), |p| p.to_string_lossy().into_owned());
    let wt_str = worktree_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    let label = req
        .label
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Terminal".to_owned());
    let stream = Arc::new(StdMutex::new(TerminalStreamState::new(
        req.thread_id.clone(),
        req.terminal_id.clone(),
        cwd_str,
        wt_str,
        label,
        cols,
        rows,
        None,
        req.max_history_bytes,
        metadata_tx,
    )));
    let sinks = TerminalSessionSinks {
        grid: Arc::clone(&grid),
        parser: Arc::clone(&parser),
        stream: Arc::clone(&stream),
        output_notify: Arc::clone(&output_notify),
    };
    let process = TerminalChildHandle::spawn_for_request(req, cwd.as_ref(), cols, rows, &sinks)?;
    let pid = process.as_ref().and_then(TerminalChildHandle::pid);
    if let Ok(mut s) = stream.lock() {
        s.set_pid(pid);
        s.emit_started(req.resume);
    }
    Ok((
        cols,
        rows,
        TerminalSessionEntry {
            grid,
            parser,
            cwd,
            worktree_path,
            palette_overrides,
            process,
            output_notify,
            stream,
        },
    ))
}

/// Format input text for terminal injection, sanitizing bracketed paste sequences (`BUG-25`/paste safety).
pub(crate) fn prepare_write_bytes(
    data: &str,
    as_paste: bool,
    bracketed_paste_enabled: bool,
) -> Vec<u8> {
    if as_paste {
        let safe = sanitize_bracketed_paste(data);
        encode_paste(&safe, bracketed_paste_enabled).into_bytes()
    } else {
        data.as_bytes().to_vec()
    }
}

pub(crate) fn validate_resume_eligibility(
    keys: &TerminalSessionKeys,
    key: &TerminalSessionKey,
    resume: bool,
) -> Result<(), ServerRuntimeError> {
    if !resume {
        return Ok(());
    }
    if keys.is_closed(key) {
        return Err(ServerRuntimeError::Conflict(format!(
            "terminal `{}` was explicitly closed and cannot be resumed",
            key.to_wire_key()
        )));
    }
    if !keys.can_resume(key) {
        return Err(ServerRuntimeError::NotFound(format!(
            "terminal `{}` has no resumable session state",
            key.to_wire_key()
        )));
    }
    Ok(())
}

pub(crate) fn active_entry<'a>(
    active: &'a HashMap<TerminalSessionKey, TerminalSessionEntry>,
    key: &TerminalSessionKey,
) -> Result<&'a TerminalSessionEntry, ServerRuntimeError> {
    active.get(key).ok_or_else(|| {
        ServerRuntimeError::NotFound(format!("terminal `{}` is not active", key.to_wire_key()))
    })
}

pub(crate) fn active_entry_mut<'a>(
    active: &'a mut HashMap<TerminalSessionKey, TerminalSessionEntry>,
    key: &TerminalSessionKey,
) -> Result<&'a mut TerminalSessionEntry, ServerRuntimeError> {
    active.get_mut(key).ok_or_else(|| {
        ServerRuntimeError::NotFound(format!("terminal `{}` is not active", key.to_wire_key()))
    })
}

pub(crate) fn collect_summaries(
    active: &HashMap<TerminalSessionKey, TerminalSessionEntry>,
    thread_id: Option<&str>,
) -> Vec<TerminalSessionSummary> {
    let mut out: Vec<TerminalSessionSummary> = active
        .iter()
        .filter(|(k, _)| thread_id.is_none_or(|t| k.thread_id == t))
        .filter_map(|(_, entry)| entry.stream.lock().ok().map(|s| s.summary()))
        .collect();
    out.sort_by(|a, b| {
        a.thread_id
            .cmp(&b.thread_id)
            .then_with(|| a.terminal_id.cmp(&b.terminal_id))
    });
    out
}
