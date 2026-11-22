//! Cross-platform interactive OS pseudoterminal (`PTY`) bindings and safe RAII wrapper.
//!
//! Uses direct Win32 `ConPTY` + kill-on-close [`crate::win32_job::JobHandle`] on Windows
//! and POSIX `posix_openpt`/`grantpt`/`unlockpt` + `TIOCSWINSZ`/`TIOCSCTTY` on Unix.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitStatus;

use thiserror::Error;

use crate::env::SanitizedChildEnv;
use crate::win32_job::Win32JobError;

#[cfg(unix)]
use crate::pty_unix as backend;
#[cfg(windows)]
use crate::pty_win32 as backend;
#[cfg(not(any(unix, windows)))]
mod backend {
    use std::io;
    use std::process::ExitStatus;

    use super::{PtyError, PtySpawnConfig};

    #[derive(Debug)]
    pub(crate) struct PlatformReader;

    impl io::Read for PlatformReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interactive PTYs are unsupported on this platform",
            ))
        }
    }

    #[derive(Debug)]
    pub(crate) struct PlatformPty;

    impl PlatformPty {
        pub(crate) fn spawn(_config: &PtySpawnConfig) -> Result<Self, PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }

        pub(crate) fn reader_clone(&self) -> Result<PlatformReader, PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }

        pub(crate) fn read(&self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "interactive PTYs are unsupported on this platform",
            ))
        }

        pub(crate) fn write_all(&self, _data: &[u8]) -> Result<(), PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }

        pub(crate) fn resize(&self, _cols: u16, _rows: u16) -> Result<(), PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }

        pub(crate) fn try_wait(&self) -> Result<Option<ExitStatus>, PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }

        pub(crate) fn wait(&self) -> Result<ExitStatus, PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }

        pub(crate) const fn pid(&self) -> u32 {
            0
        }

        pub(crate) fn terminate(&self) -> Result<(), PtyError> {
            Err(PtyError::UnsupportedPlatform)
        }
    }
}

/// Errors produced when allocating, interacting with, resizing, or tearing down a PTY.
#[derive(Debug, Error)]
pub enum PtyError {
    /// Interactive PTYs are unavailable on this compile target.
    #[error("interactive PTYs are not supported on this platform")]
    UnsupportedPlatform,
    /// Program path or binary name was empty.
    #[error("PTY program path must not be empty")]
    EmptyProgram,
    /// Terminal rows or columns were zero or exceeded platform bounds.
    #[error("invalid PTY dimensions {cols}x{rows}; both must be positive and fit i16")]
    InvalidDimensions {
        /// Requested column count.
        cols: u16,
        /// Requested row count.
        rows: u16,
    },
    /// PTY pipe, descriptor, or stream I/O failure.
    #[error("PTY I/O failed: {0}")]
    Io(#[from] io::Error),
    /// Win32 Job Object setup or termination failed.
    #[error(transparent)]
    Win32Job(#[from] Win32JobError),
    /// OS failed allocating the pseudoterminal or launching the child process.
    #[error("failed spawning PTY child process: {0}")]
    SpawnFailed(#[source] io::Error),
}

/// Spawn configuration for creating an interactive OS [`PtyProcess`].
#[derive(Debug, Clone)]
pub struct PtySpawnConfig {
    /// Executable path or binary name resolved on `PATH`.
    pub program: OsString,
    /// Literal arguments passed to the child process.
    pub args: Vec<OsString>,
    /// Optional working directory for the child process.
    pub cwd: Option<PathBuf>,
    /// Sanitized environment map applied cleanly to the child process.
    pub env: SanitizedChildEnv,
    /// Initial terminal width in character columns (`> 0`).
    pub cols: u16,
    /// Initial terminal height in character rows (`> 0`).
    pub rows: u16,
}

impl PtySpawnConfig {
    /// Creates a new [`PtySpawnConfig`] inheriting a sanitized snapshot of the current environment.
    pub fn new(program: impl Into<OsString>, cols: u16, rows: u16) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: SanitizedChildEnv::from_os_env(),
            cols,
            rows,
        }
    }

    /// Appends a single argument to the child command line.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends multiple arguments to the child command line.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets the child working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Replaces the sanitized environment snapshot.
    #[must_use]
    pub fn env(mut self, env: SanitizedChildEnv) -> Self {
        self.env = env;
        self
    }
}

/// Cloned read half of a [`PtyProcess`] suitable for a dedicated streaming reader thread.
#[derive(Debug)]
pub struct PtyReader {
    inner: backend::PlatformReader,
}

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// Safe RAII owner of a child process attached to an interactive OS pseudoterminal.
#[derive(Debug)]
pub struct PtyProcess {
    inner: backend::PlatformPty,
}

impl PtyProcess {
    /// Returns `true` when interactive OS PTYs are supported on this build target.
    pub const fn is_supported() -> bool {
        cfg!(any(unix, windows))
    }

    /// Validates `config` and spawns the child attached to a fresh OS PTY.
    pub fn spawn(config: &PtySpawnConfig) -> Result<Self, PtyError> {
        if config.program.is_empty() {
            return Err(PtyError::EmptyProgram);
        }
        if config.cols == 0 || config.rows == 0 {
            return Err(PtyError::InvalidDimensions {
                cols: config.cols,
                rows: config.rows,
            });
        }
        Ok(Self {
            inner: backend::PlatformPty::spawn(config)?,
        })
    }

    /// Clones a standalone [`PtyReader`] that can stream PTY output on another thread.
    pub fn reader_clone(&self) -> Result<PtyReader, PtyError> {
        Ok(PtyReader {
            inner: self.inner.reader_clone()?,
        })
    }

    /// Reads available bytes from the PTY output stream.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    /// Writes `data` cleanly to the PTY input stream and flushes it.
    pub fn write_all(&self, data: &[u8]) -> Result<(), PtyError> {
        self.inner.write_all(data)
    }

    /// Resizes the live OS PTY window to `cols x rows`.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        if cols == 0 || rows == 0 {
            return Err(PtyError::InvalidDimensions { cols, rows });
        }
        self.inner.resize(cols, rows)
    }

    /// Non-blocking check for whether the PTY child has exited.
    pub fn try_wait(&self) -> Result<Option<ExitStatus>, PtyError> {
        self.inner.try_wait()
    }

    /// Blocks until the PTY child process exits and returns its [`ExitStatus`].
    pub fn wait(&self) -> Result<ExitStatus, PtyError> {
        self.inner.wait()
    }

    /// Returns the OS process identifier of the direct child running inside the PTY.
    pub fn pid(&self) -> u32 {
        self.inner.pid()
    }

    /// Terminates the PTY session and its child process tree.
    pub fn terminate(&self) -> Result<(), PtyError> {
        self.inner.terminate()
    }
}

/// Extracts `(exit_code, signal)` from a finished [`ExitStatus`].
pub fn exit_parts(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        (status.code(), status.signal())
    }
    #[cfg(not(unix))]
    {
        (status.code(), None)
    }
}
