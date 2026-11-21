//! Per-session terminal VT grid, palette, and supervised child process attachment.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{PipeReader, PipeWriter, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use t3_process_ffi::env::SanitizedChildEnv;
use t3_process_ffi::pipes::PipeSetConfig;
use t3_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken, SpawnSpec};
use t3_protocol::provider_env::{TerminalResumeStatus, TerminalSessionKey};
use t3_terminal_vt::paste::encode_paste;
use t3_terminal_vt::{Rgba, TerminalGrid, VtParser, sanitize_bracketed_paste};
use tokio::sync::Notify;

use crate::error::ServerRuntimeError;

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
    /// Binary `"T3VT"` wire snapshot frame encoded via `t3_terminal_vt::encode_t3vt`.
    pub t3vt_bytes: Vec<u8>,
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
    /// Whether this call is attempting to resume a prior session (`BUG-08`).
    pub resume: bool,
    /// Optional OS executable/shell command to attach to this session.
    pub command: Option<OsString>,
    /// Optional CLI arguments for `command`.
    pub args: Vec<OsString>,
    /// Explicit environment variable overrides layered onto `SanitizedChildEnv`.
    pub env_overrides: BTreeMap<String, String>,
}

impl TerminalOpenRequest {
    /// Construct a new headless terminal open request.
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
            resume: false,
            command: None,
            args: Vec::new(),
            env_overrides: BTreeMap::new(),
        }
    }
}

pub(crate) struct AttachedProcess {
    supervisor: JobProcessSupervisor,
    token: ProcessToken,
    stdin: Option<PipeWriter>,
}

impl AttachedProcess {
    pub fn spawn(
        command: &OsString,
        args: &[OsString],
        cwd: Option<&PathBuf>,
        env_overrides: &BTreeMap<String, String>,
        grid: &Arc<StdMutex<TerminalGrid>>,
        parser: &Arc<StdMutex<VtParser>>,
        output_notify: &Arc<Notify>,
    ) -> Result<Self, ServerRuntimeError> {
        let mut env = SanitizedChildEnv::from_os_env();
        for (k, v) in env_overrides {
            env.set(k.as_str(), v.as_str());
        }
        let mut spec = SpawnSpec::new(command)
            .args(args)
            .env(env)
            .stdio(PipeSetConfig::piped())
            .windows_no_window(true);
        if let Some(dir) = cwd {
            spec = spec.cwd(dir);
        }
        let mut supervisor =
            JobProcessSupervisor::new().map_err(|e| ServerRuntimeError::Terminal(e.to_string()))?;
        supervisor.set_kill_on_drop(true);
        let (token, mut pipes) = supervisor
            .spawn(&spec)
            .map_err(|e| ServerRuntimeError::Terminal(e.to_string()))?;

        if let Some(stdout) = pipes.take_stdout() {
            spawn_vt_pipe_pump(
                stdout,
                Arc::clone(grid),
                Arc::clone(parser),
                Arc::clone(output_notify),
            );
        }
        if let Some(stderr) = pipes.take_stderr() {
            spawn_vt_pipe_pump(
                stderr,
                Arc::clone(grid),
                Arc::clone(parser),
                Arc::clone(output_notify),
            );
        }
        Ok(Self {
            supervisor,
            token,
            stdin: pipes.take_stdin(),
        })
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), ServerRuntimeError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(ServerRuntimeError::Terminal(
                "attached terminal stdin is closed".to_owned(),
            ));
        };
        stdin.write_all(bytes)?;
        stdin.flush()?;
        Ok(())
    }

    pub fn terminate(&mut self) {
        let _ = self.stdin.take();
        let _ = self.supervisor.terminate_child(self.token);
        let _ = self.supervisor.terminate_tree(1);
    }
}

impl Drop for AttachedProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_vt_pipe_pump(
    mut reader: PipeReader,
    grid: Arc<StdMutex<TerminalGrid>>,
    parser: Arc<StdMutex<VtParser>>,
    notify: Arc<Notify>,
) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                notify.notify_waiters();
                break;
            }
            if let (Ok(mut g), Ok(mut p)) = (grid.lock(), parser.lock()) {
                p.feed(&buf[..n], &mut g);
            }
            notify.notify_waiters();
        }
    });
}

pub(crate) struct TerminalSessionEntry {
    pub grid: Arc<StdMutex<TerminalGrid>>,
    pub parser: Arc<StdMutex<VtParser>>,
    pub cwd: Option<PathBuf>,
    pub palette_overrides: Vec<(u8, Rgba)>,
    pub process: Option<AttachedProcess>,
    pub output_notify: Arc<Notify>,
}

#[derive(Debug, Clone)]
pub(crate) struct EvictedSessionMeta {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<PathBuf>,
    pub palette_overrides: Vec<(u8, Rgba)>,
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
