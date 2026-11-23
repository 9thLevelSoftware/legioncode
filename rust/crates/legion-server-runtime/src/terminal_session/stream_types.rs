//! Wire-compatible terminal stream/snapshot DTOs and UTF-8 history retention helpers.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Default maximum sanitized scrollback/history retained per terminal session (`512 KiB`).
pub const DEFAULT_MAX_TERMINAL_BUFFER_BYTES: usize = 512 * 1024;

/// Default broadcast channel capacity per terminal output stream.
pub const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 256;

/// Lifecycle status matching `TerminalSessionStatus` in `packages/contracts/src/terminal.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalSessionLifecycleStatus {
    /// Session process is starting.
    Starting,
    /// Session is active (live PTY/process or open headless VT session).
    Running,
    /// Attached child process exited.
    Exited,
    /// Spawning or stream execution failed.
    Error,
}

/// Full terminal session state snapshot matching `TerminalSessionSnapshot` in `terminal.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionStateSnapshot {
    /// Owning thread identifier.
    pub thread_id: String,
    /// Session terminal identifier.
    pub terminal_id: String,
    /// Effective working directory string.
    pub cwd: String,
    /// Optional git worktree root path.
    pub worktree_path: Option<String>,
    /// Lifecycle state (`starting` | `running` | `exited` | `error`).
    pub status: TerminalSessionLifecycleStatus,
    /// Live child OS process ID when running under a subprocess/PTY.
    pub pid: Option<u32>,
    /// Bounded sanitized terminal output history ready for initial xterm replay.
    pub history: String,
    /// Process exit code once exited.
    pub exit_code: Option<i32>,
    /// Process terminating signal number on Unix once exited.
    pub exit_signal: Option<i32>,
    /// Display label / OSC window title.
    pub label: String,
    /// RFC-3339 timestamp of the last session mutation.
    pub updated_at: String,
    /// Monotonic event sequence included in this snapshot for gap-free broadcast catchup.
    pub sequence: u64,
    /// Current terminal width in columns.
    pub cols: u16,
    /// Current terminal height in rows.
    pub rows: u16,
    /// Startup or runtime error description when `status == Error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Lightweight terminal metadata summary matching `TerminalSummary` in `terminal.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionSummary {
    /// Owning thread identifier.
    pub thread_id: String,
    /// Session terminal identifier.
    pub terminal_id: String,
    /// Effective working directory string.
    pub cwd: String,
    /// Optional git worktree root path.
    pub worktree_path: Option<String>,
    /// Lifecycle status.
    pub status: TerminalSessionLifecycleStatus,
    /// Active subprocess PID if running.
    pub pid: Option<u32>,
    /// Recorded process exit code.
    pub exit_code: Option<i32>,
    /// Recorded Unix exit signal.
    pub exit_signal: Option<i32>,
    /// Whether a live child process is currently running.
    pub has_running_subprocess: bool,
    /// Current display label / window title.
    pub label: String,
    /// RFC-3339 last update timestamp.
    pub updated_at: String,
}

/// Real-time per-session chunk matching `TerminalEvent` in `packages/contracts/src/terminal.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TerminalStreamChunk {
    /// Session opened cleanly.
    #[serde(rename_all = "camelCase")]
    Started {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
        /// Initial session snapshot.
        snapshot: TerminalSessionStateSnapshot,
    },
    /// Raw terminal output chunk (decoded incrementally at valid UTF-8 boundaries, plus raw bytes).
    #[serde(rename_all = "camelCase")]
    Output {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
        /// UTF-8 decoded text chunk for wire `TerminalOutputEvent`.
        data: String,
        /// Original byte slice for binary consumers.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        bytes: Vec<u8>,
    },
    /// Attached child process exited.
    #[serde(rename_all = "camelCase")]
    Exited {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
        /// Exit code when terminated normally.
        exit_code: Option<i32>,
        /// Terminating signal on Unix.
        exit_signal: Option<i32>,
    },
    /// Session was explicitly closed.
    #[serde(rename_all = "camelCase")]
    Closed {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
    },
    /// Fatal spawn or I/O error occurred.
    #[serde(rename_all = "camelCase")]
    Error {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
        /// Human-readable diagnostic error message.
        message: String,
    },
    /// Viewport and scrollback history cleared.
    #[serde(rename_all = "camelCase")]
    Cleared {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
    },
    /// Session was restarted or resumed onto a new shell process.
    #[serde(rename_all = "camelCase")]
    Restarted {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
        /// Fresh session snapshot.
        snapshot: TerminalSessionStateSnapshot,
    },
    /// Title, subprocess activity, or dimensions changed.
    #[serde(rename_all = "camelCase")]
    Activity {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
        /// Monotonic per-session sequence.
        sequence: u64,
        /// Whether child process is still running.
        has_running_subprocess: bool,
        /// Current label / OSC title.
        label: String,
        /// Updated terminal columns when resized.
        #[serde(skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        /// Updated terminal rows when resized.
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
    },
}

impl TerminalStreamChunk {
    /// Returns the monotonic sequence number assigned to this chunk.
    pub const fn sequence(&self) -> u64 {
        match self {
            Self::Started { sequence, .. }
            | Self::Output { sequence, .. }
            | Self::Exited { sequence, .. }
            | Self::Closed { sequence, .. }
            | Self::Error { sequence, .. }
            | Self::Cleared { sequence, .. }
            | Self::Restarted { sequence, .. }
            | Self::Activity { sequence, .. } => *sequence,
        }
    }
}

/// Real-time fleet metadata stream event matching `TerminalMetadataStreamEvent` in `terminal.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TerminalMetadataStreamEvent {
    /// Initial snapshot of all active terminal summaries when subscribing.
    #[serde(rename_all = "camelCase")]
    Snapshot {
        /// Active terminal session summaries.
        terminals: Vec<TerminalSessionSummary>,
    },
    /// Created or updated terminal summary.
    #[serde(rename_all = "camelCase")]
    Upsert {
        /// Updated summary record.
        terminal: TerminalSessionSummary,
    },
    /// Session removed or closed.
    #[serde(rename_all = "camelCase")]
    Remove {
        /// Owning thread ID.
        thread_id: String,
        /// Terminal ID.
        terminal_id: String,
    },
}

/// Atomic subscription handle combining initial scrollback snapshot with live stream receiver.
pub struct TerminalOutputSubscription {
    /// Snapshot captured atomically under the same lock as `receiver` subscription.
    pub snapshot: TerminalSessionStateSnapshot,
    /// Broadcast receiver yielding subsequent [`TerminalStreamChunk`]s with `sequence > snapshot.sequence`.
    pub receiver: broadcast::Receiver<TerminalStreamChunk>,
}

/// Retains at most the trailing `max_bytes` of UTF-8 history aligned on code-point & ESC boundaries.
pub fn retain_history_tail(history: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if history.len() <= max_bytes {
        return history.to_owned();
    }
    let mut start = history.len().saturating_sub(max_bytes);
    while start < history.len() && !history.is_char_boundary(start) {
        start += 1;
    }
    let slice = &history[start..];
    if let Some(first_esc) = slice.find('\x1b')
        && let Some(c) = slice[..first_esc].chars().next()
        && matches!(c, '[' | ']' | 'P' | 'X' | '^' | '_' | '0'..='9' | '?' | ';')
        && first_esc < 64
    {
        return slice[first_esc..].to_owned();
    }
    slice.to_owned()
}
