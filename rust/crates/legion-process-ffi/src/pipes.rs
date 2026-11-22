//! RAII stdio and extra-descriptor pipe management (`BUG-09` handle leak prevention).
//!
//! [`ScopedPipeSet`] and [`ChildPipeGuard`] track pre-spawn pipe endpoints so that any
//! early validation error, spawn failure, or panic closes all child-side descriptors
//! immediately without leaving dangling write ends open in the parent process.

use std::io::{self, PipeReader, PipeWriter};
use std::process::{Child, Command, Stdio};

/// Desired stdio wiring for one child stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StdioMode {
    /// Allocate a fresh OS pipe pair between parent and child.
    #[default]
    Piped,
    /// Inherit the parent's standard stream.
    Inherit,
    /// Redirect the child stream to the null device (`/dev/null` or `NUL`).
    Null,
}

/// Configuration for [`ScopedPipeSet::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeSetConfig {
    /// Wiring mode for standard input (`fd 0`).
    pub stdin: StdioMode,
    /// Wiring mode for standard output (`fd 1`).
    pub stdout: StdioMode,
    /// Wiring mode for standard error (`fd 2`).
    pub stderr: StdioMode,
    /// Number of auxiliary output pipes to pre-allocate alongside standard streams.
    pub extra_outputs: usize,
}

impl Default for PipeSetConfig {
    fn default() -> Self {
        Self::piped()
    }
}

impl PipeSetConfig {
    /// Configures piped `stdin`, `stdout`, and `stderr` with no auxiliary descriptors.
    pub const fn piped() -> Self {
        Self {
            stdin: StdioMode::Piped,
            stdout: StdioMode::Piped,
            stderr: StdioMode::Piped,
            extra_outputs: 0,
        }
    }

    /// Configures `Null` on all three standard streams (fire-and-forget / daemonized helper).
    pub const fn null() -> Self {
        Self {
            stdin: StdioMode::Null,
            stdout: StdioMode::Null,
            stderr: StdioMode::Null,
            extra_outputs: 0,
        }
    }
}

/// Parent-side pipe endpoints returned once child descriptors have been handed off or guarded.
#[derive(Debug, Default)]
pub struct ParentPipeSet {
    /// Writable parent end connected to the child's `stdin`.
    pub stdin: Option<PipeWriter>,
    /// Readable parent end connected to the child's `stdout`.
    pub stdout: Option<PipeReader>,
    /// Readable parent end connected to the child's `stderr`.
    pub stderr: Option<PipeReader>,
    /// Readable parent ends for auxiliary output pipes.
    pub extra_outputs: Vec<PipeReader>,
}

impl ParentPipeSet {
    /// Takes ownership of the parent `stdin` writer.
    pub fn take_stdin(&mut self) -> Option<PipeWriter> {
        self.stdin.take()
    }

    /// Takes ownership of the parent `stdout` reader.
    pub fn take_stdout(&mut self) -> Option<PipeReader> {
        self.stdout.take()
    }

    /// Takes ownership of the parent `stderr` reader.
    pub fn take_stderr(&mut self) -> Option<PipeReader> {
        self.stderr.take()
    }

    /// Closes all remaining parent-side pipe endpoints immediately.
    pub fn close_all(mut self) {
        self.stdin.take();
        self.stdout.take();
        self.stderr.take();
        self.extra_outputs.clear();
    }
}

/// RAII guard owning child-side pipe descriptors prior to `Command::spawn()`.
///
/// Dropping [`ChildPipeGuard`] closes any child-side endpoints that were not yet
/// transferred into a spawned child process, preventing the parent from holding
/// child write ends open on pre-spawn failures (`BUG-09`).
#[derive(Debug)]
pub struct ChildPipeGuard {
    stdin_mode: StdioMode,
    stdout_mode: StdioMode,
    stderr_mode: StdioMode,
    stdin: Option<PipeReader>,
    stdout: Option<PipeWriter>,
    stderr: Option<PipeWriter>,
    extra_outputs: Vec<Option<PipeWriter>>,
    committed: bool,
}

impl ChildPipeGuard {
    /// Returns the number of child-side pipe descriptors still held open by this guard.
    pub fn open_child_count(&self) -> usize {
        let base = usize::from(self.stdin.is_some())
            + usize::from(self.stdout.is_some())
            + usize::from(self.stderr.is_some());
        let extra = self
            .extra_outputs
            .iter()
            .filter(|slot| slot.is_some())
            .count();
        base + extra
    }

    /// Returns `true` once child descriptors have been transferred into a [`Command`].
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    /// Transfers child-side stdio endpoints onto `command`, clearing them from this guard.
    pub fn attach_to_command(&mut self, command: &mut Command) {
        command.stdin(match (self.stdin.take(), self.stdin_mode) {
            (Some(reader), _) => Stdio::from(reader),
            (None, StdioMode::Null) => Stdio::null(),
            (None, StdioMode::Inherit | StdioMode::Piped) => Stdio::inherit(),
        });
        command.stdout(match (self.stdout.take(), self.stdout_mode) {
            (Some(writer), _) => Stdio::from(writer),
            (None, StdioMode::Null) => Stdio::null(),
            (None, StdioMode::Inherit | StdioMode::Piped) => Stdio::inherit(),
        });
        command.stderr(match (self.stderr.take(), self.stderr_mode) {
            (Some(writer), _) => Stdio::from(writer),
            (None, StdioMode::Null) => Stdio::null(),
            (None, StdioMode::Inherit | StdioMode::Piped) => Stdio::inherit(),
        });
        self.extra_outputs.clear();
        self.committed = true;
    }

    /// Attaches child descriptors to `command` and invokes `command.spawn()`.
    ///
    /// On error, every child-side endpoint attached to `command` is dropped by `Command`
    /// and any remaining endpoints in this guard are closed immediately.
    pub fn spawn(&mut self, command: &mut Command) -> io::Result<Child> {
        self.attach_to_command(command);
        let result = command.spawn();
        if result.is_err() {
            command.stdin(Stdio::null());
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }
        result
    }

    /// Explicitly closes any child-side descriptors still held by this guard.
    pub fn close_remaining(&mut self) {
        self.stdin.take();
        self.stdout.take();
        self.stderr.take();
        self.extra_outputs.clear();
    }
}

impl Drop for ChildPipeGuard {
    fn drop(&mut self) {
        self.close_remaining();
    }
}

/// Combined RAII guard tracking both parent and child pre-spawn pipe endpoints.
///
/// # Examples
///
/// ```
/// use std::io::Read;
/// use legion_process_ffi::pipes::{PipeSetConfig, ScopedPipeSet};
///
/// let pipes = ScopedPipeSet::new(PipeSetConfig::piped())?;
/// let (child_guard, mut parent) = pipes.split();
/// drop(child_guard);
/// let mut buf = [0u8; 8];
/// if let Some(mut stdout) = parent.take_stdout() {
///     assert_eq!(stdout.read(&mut buf)?, 0);
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct ScopedPipeSet {
    child: ChildPipeGuard,
    parent: Option<ParentPipeSet>,
}

impl ScopedPipeSet {
    /// Allocates OS pipes according to `config` with zero leaked descriptors on partial failure.
    pub fn new(config: PipeSetConfig) -> io::Result<Self> {
        let (parent_stdin, child_stdin) = match config.stdin {
            StdioMode::Piped => {
                let (reader, writer) = io::pipe()?;
                (Some(writer), Some(reader))
            }
            StdioMode::Inherit | StdioMode::Null => (None, None),
        };
        let (parent_stdout, child_stdout) = match config.stdout {
            StdioMode::Piped => {
                let (reader, writer) = io::pipe()?;
                (Some(reader), Some(writer))
            }
            StdioMode::Inherit | StdioMode::Null => (None, None),
        };
        let (parent_stderr, child_stderr) = match config.stderr {
            StdioMode::Piped => {
                let (reader, writer) = io::pipe()?;
                (Some(reader), Some(writer))
            }
            StdioMode::Inherit | StdioMode::Null => (None, None),
        };

        let mut parent_extras = Vec::with_capacity(config.extra_outputs);
        let mut child_extras = Vec::with_capacity(config.extra_outputs);
        for _ in 0..config.extra_outputs {
            let (reader, writer) = io::pipe()?;
            parent_extras.push(reader);
            child_extras.push(Some(writer));
        }

        Ok(Self {
            child: ChildPipeGuard {
                stdin_mode: config.stdin,
                stdout_mode: config.stdout,
                stderr_mode: config.stderr,
                stdin: child_stdin,
                stdout: child_stdout,
                stderr: child_stderr,
                extra_outputs: child_extras,
                committed: false,
            },
            parent: Some(ParentPipeSet {
                stdin: parent_stdin,
                stdout: parent_stdout,
                stderr: parent_stderr,
                extra_outputs: parent_extras,
            }),
        })
    }

    /// Returns the number of child-side pipe descriptors still held by this guard.
    pub fn child_open_count(&self) -> usize {
        self.child.open_child_count()
    }

    /// Splits this set into an independent [`ChildPipeGuard`] and [`ParentPipeSet`].
    pub fn split(mut self) -> (ChildPipeGuard, ParentPipeSet) {
        let parent = self.parent.take().unwrap_or_default();
        let child = ChildPipeGuard {
            stdin_mode: self.child.stdin_mode,
            stdout_mode: self.child.stdout_mode,
            stderr_mode: self.child.stderr_mode,
            stdin: self.child.stdin.take(),
            stdout: self.child.stdout.take(),
            stderr: self.child.stderr.take(),
            extra_outputs: std::mem::take(&mut self.child.extra_outputs),
            committed: self.child.committed,
        };
        (child, parent)
    }

    /// Attaches child descriptors to `command`, spawns it, and returns `(Child, ParentPipeSet)`.
    ///
    /// If `spawn` fails, both child and parent endpoints are closed immediately on drop.
    pub fn spawn_command(self, command: &mut Command) -> io::Result<(Child, ParentPipeSet)> {
        let (mut child_guard, parent_pipes) = self.split();
        let child = child_guard.spawn(command)?;
        Ok((child, parent_pipes))
    }
}

impl Drop for ScopedPipeSet {
    fn drop(&mut self) {
        self.child.close_remaining();
        if let Some(parent) = self.parent.take() {
            parent.close_all();
        }
    }
}
