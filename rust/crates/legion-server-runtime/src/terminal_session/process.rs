//! Child OS process attachment backends for [`crate::TerminalRuntime`]: interactive [`PtyProcess`] and piped `AttachedProcess` fallback.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{ErrorKind, PipeReader, PipeWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, mpsc};

use legion_process_ffi::pipes::PipeSetConfig;
use legion_process_ffi::pty::{PtyError, PtyProcess, PtyReader, PtySpawnConfig, exit_parts};
use legion_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken, SpawnSpec};
use legion_terminal_vt::{TerminalGrid, VtParser};
use tokio::sync::Notify;

use crate::error::ServerRuntimeError;
use crate::terminal_session::TerminalOpenRequest;
use crate::terminal_session::shell::{build_terminal_child_env, resolve_default_shell_candidates};
use crate::terminal_session::stream::TerminalStreamState;

/// Controls whether opening a terminal uses a headless VT grid, a piped child, or an interactive OS PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalProcessMode {
    /// Existing compatibility mode: headless VT when `command: None`, piped child when `command: Some`.
    #[default]
    HeadlessOrPiped,
    /// Interactive OS PTY (`PtyProcess`) spawning explicit `command` or default OS shell, falling back to pipes only if PTYs are unsupported.
    InteractivePty,
    /// Explicitly spawn child stdio through `JobProcessSupervisor` pipes without a PTY.
    PipedProcess,
}

/// Shared VT, stream broadcast, and readiness sinks wired into spawned child pumps.
#[derive(Clone)]
pub(crate) struct TerminalSessionSinks {
    pub grid: Arc<StdMutex<TerminalGrid>>,
    pub parser: Arc<StdMutex<VtParser>>,
    pub stream: Arc<StdMutex<TerminalStreamState>>,
    pub output_notify: Arc<Notify>,
}

/// Piped child process handle (`JobProcessSupervisor`) for non-PTY / headless fallback sessions.
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
        sinks: &TerminalSessionSinks,
    ) -> Result<Self, ServerRuntimeError> {
        let env = build_terminal_child_env(env_overrides);
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
            spawn_vt_pipe_pump(stdout, sinks.clone());
        }
        if let Some(stderr) = pipes.take_stderr() {
            spawn_vt_pipe_pump(stderr, sinks.clone());
        }
        Ok(Self {
            supervisor,
            token,
            stdin: pipes.take_stdin(),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.supervisor.pid(self.token)
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

/// Active child handle attached to a [`crate::TerminalRuntime`] session.
pub(crate) enum TerminalChildHandle {
    Pty {
        pty: Arc<PtyProcess>,
        shutting_down: Arc<AtomicBool>,
    },
    Piped(AttachedProcess),
}

impl TerminalChildHandle {
    pub(crate) fn spawn_for_request(
        req: &TerminalOpenRequest,
        cwd: Option<&PathBuf>,
        cols: u16,
        rows: u16,
        sinks: &TerminalSessionSinks,
    ) -> Result<Option<Self>, ServerRuntimeError> {
        match req.process_mode {
            TerminalProcessMode::InteractivePty => Self::spawn_interactive_pty(
                req.command.as_ref(),
                &req.args,
                cwd,
                &req.env_overrides,
                cols,
                rows,
                sinks,
            )
            .map(Some),
            TerminalProcessMode::HeadlessOrPiped | TerminalProcessMode::PipedProcess => {
                match &req.command {
                    Some(cmd) => {
                        AttachedProcess::spawn(cmd, &req.args, cwd, &req.env_overrides, sinks)
                            .map(|p| Some(Self::Piped(p)))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    fn spawn_interactive_pty(
        command: Option<&OsString>,
        args: &[OsString],
        cwd: Option<&PathBuf>,
        env_overrides: &BTreeMap<String, String>,
        cols: u16,
        rows: u16,
        sinks: &TerminalSessionSinks,
    ) -> Result<Self, ServerRuntimeError> {
        let candidates: Vec<(OsString, &[OsString])> = match command {
            Some(explicit) => vec![(explicit.clone(), args)],
            None => resolve_default_shell_candidates(env_overrides)
                .into_iter()
                .map(|shell| (shell, &[][..]))
                .collect(),
        };
        let child_env = build_terminal_child_env(env_overrides);
        let mut last_err: Option<PtyError> = None;

        for (program, cand_args) in candidates {
            let mut cfg = PtySpawnConfig::new(&program, cols, rows)
                .args(cand_args)
                .env(child_env.clone());
            if let Some(dir) = cwd {
                cfg = cfg.cwd(dir);
            }
            match PtyProcess::spawn(&cfg) {
                Ok(pty_raw) => {
                    let pty = Arc::new(pty_raw);
                    let shutting_down = Arc::new(AtomicBool::new(false));
                    let reader = pty
                        .reader_clone()
                        .map_err(|e| ServerRuntimeError::Terminal(e.to_string()))?;
                    spawn_pty_pumps(
                        Arc::clone(&pty),
                        reader,
                        sinks.clone(),
                        Arc::clone(&shutting_down),
                    );
                    return Ok(Self::Pty { pty, shutting_down });
                }
                Err(PtyError::UnsupportedPlatform) => {
                    let piped =
                        AttachedProcess::spawn(&program, cand_args, cwd, env_overrides, sinks)?;
                    return Ok(Self::Piped(piped));
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }

        Err(ServerRuntimeError::Terminal(last_err.map_or_else(
            || "no interactive shell executable found".to_owned(),
            |e| e.to_string(),
        )))
    }

    pub(crate) fn pid(&self) -> Option<u32> {
        match self {
            Self::Pty { pty, .. } => Some(pty.pid()),
            Self::Piped(piped) => piped.pid(),
        }
    }

    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), ServerRuntimeError> {
        match self {
            Self::Pty { pty, .. } => pty
                .write_all(bytes)
                .map_err(|e| ServerRuntimeError::Terminal(e.to_string())),
            Self::Piped(piped) => piped.write_bytes(bytes),
        }
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<(), ServerRuntimeError> {
        match self {
            Self::Pty { pty, .. } => match pty.resize(cols, rows) {
                Ok(()) => Ok(()),
                Err(PtyError::Io(err)) if err.kind() == ErrorKind::BrokenPipe => Ok(()),
                Err(err) => Err(ServerRuntimeError::Terminal(err.to_string())),
            },
            Self::Piped(_) => Ok(()),
        }
    }

    pub(crate) fn terminate(&mut self) {
        match self {
            Self::Pty { pty, shutting_down } => {
                shutting_down.store(true, Ordering::Release);
                let _ = pty.terminate();
            }
            Self::Piped(piped) => piped.terminate(),
        }
    }
}

fn spawn_vt_pipe_pump(mut reader: PipeReader, sinks: TerminalSessionSinks) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                if let Ok(mut s) = sinks.stream.lock() {
                    s.flush_eof_utf8();
                }
                sinks.output_notify.notify_waiters();
                break;
            }
            feed_chunk_to_grid_and_stream(&buf[..n], &sinks);
        }
    });
}

type PtyExitParts = (Option<i32>, Option<i32>);

fn spawn_pty_pumps(
    pty: Arc<PtyProcess>,
    mut reader: PtyReader,
    sinks: TerminalSessionSinks,
    shutting_down: Arc<AtomicBool>,
) {
    // On Windows ConPTY, `conhost.exe` keeps `out_read` open until `ClosePseudoConsole` runs inside `wait()`.
    #[cfg(windows)]
    let exit_rx = {
        let (tx, rx) = mpsc::sync_channel::<Option<PtyExitParts>>(1);
        let pty_wait = Arc::clone(&pty);
        std::thread::spawn(move || {
            let parts = pty_wait.wait().ok().map(exit_parts);
            let _ = tx.send(parts);
        });
        Some(rx)
    };
    #[cfg(not(windows))]
    let exit_rx: Option<mpsc::Receiver<Option<PtyExitParts>>> = None;

    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            feed_chunk_to_grid_and_stream(&buf[..n], &sinks);
        }
        let exit_info = match exit_rx {
            Some(rx) => rx.recv().ok().flatten(),
            None => {
                if shutting_down.load(Ordering::Acquire) {
                    pty.try_wait().ok().flatten().map(exit_parts)
                } else {
                    pty.wait().ok().map(exit_parts)
                }
            }
        };
        if let Ok(mut s) = sinks.stream.lock() {
            s.flush_eof_utf8();
            if !shutting_down.load(Ordering::Acquire) {
                let (code, signal) = exit_info.unwrap_or((None, None));
                s.mark_exited(code, signal);
            }
        }
        sinks.output_notify.notify_waiters();
    });
}

fn feed_chunk_to_grid_and_stream(chunk: &[u8], sinks: &TerminalSessionSinks) {
    let title = if let (Ok(mut g), Ok(mut p)) = (sinks.grid.lock(), sinks.parser.lock()) {
        p.feed(chunk, &mut g);
        let current_title = g.title();
        (!current_title.is_empty()).then(|| current_title.to_owned())
    } else {
        None
    };
    if let Ok(mut s) = sinks.stream.lock() {
        s.push_raw_output(chunk, title.as_deref());
    }
    sinks.output_notify.notify_waiters();
}
