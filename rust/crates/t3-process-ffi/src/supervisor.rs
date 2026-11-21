//! Cross-platform OS process-tree supervisor (`BUG-09` closure).
//!
//! Combines [`SanitizedChildEnv`], [`ScopedPipeSet`], and (on Windows) [`JobHandle`]
//! so spawned child trees receive a sanitized environment, leak zero pipe descriptors
//! when pre-spawn steps fail, and terminate directly in the kernel without spawning
//! `taskkill` through a saturated worker pool.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};

use thiserror::Error;

use crate::env::SanitizedChildEnv;
use crate::pipes::{ParentPipeSet, PipeSetConfig, ScopedPipeSet};
#[cfg(windows)]
use crate::win32_job::JobHandle;
use crate::win32_job::Win32JobError;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Errors produced while configuring, spawning, or terminating supervised processes.
#[derive(Debug, Error)]
pub enum SupervisorError {
    /// Program executable path/name was empty.
    #[error("child program path must not be empty")]
    EmptyProgram,
    /// Pre-spawn stdio pipe creation failed.
    #[error("failed allocating child stdio pipes: {0}")]
    PipeSetup(#[source] io::Error),
    /// Child process launch failed (all pre-spawn pipes are closed automatically).
    #[error("failed spawning child process: {0}")]
    SpawnFailed(#[source] io::Error),
    /// Win32 Job Object creation, assignment, or termination failed.
    #[error(transparent)]
    Job(#[from] Win32JobError),
    /// Token did not refer to a known supervised child slot.
    #[error("unknown supervised child token {0}")]
    UnknownChild(usize),
}

/// Specification for launching a child process under [`JobProcessSupervisor`].
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// Executable path or command name.
    pub program: OsString,
    /// Literal command-line arguments passed without shell interpolation.
    pub args: Vec<OsString>,
    /// Optional working directory for the child process.
    pub cwd: Option<PathBuf>,
    /// Sanitized environment applied with `env_clear()` prior to launch.
    pub env: SanitizedChildEnv,
    /// Pre-spawn stdio wiring configuration.
    pub stdio: PipeSetConfig,
    /// Sets `CREATE_NO_WINDOW` when launching console helpers on Windows.
    pub windows_no_window: bool,
}

impl SpawnSpec {
    /// Creates a new [`SpawnSpec`] using a sanitized snapshot of the current host environment.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: SanitizedChildEnv::from_os_env(),
            stdio: PipeSetConfig::piped(),
            windows_no_window: true,
        }
    }

    /// Appends one literal argument to the child command line.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends multiple literal arguments to the child command line.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets the child's working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Replaces the child's sanitized environment map.
    #[must_use]
    pub fn env(mut self, env: SanitizedChildEnv) -> Self {
        self.env = env;
        self
    }

    /// Sets stdio pipe wiring for the child launch.
    #[must_use]
    pub fn stdio(mut self, stdio: PipeSetConfig) -> Self {
        self.stdio = stdio;
        self
    }

    /// Controls `CREATE_NO_WINDOW` on Windows spawns.
    #[must_use]
    pub fn windows_no_window(mut self, enabled: bool) -> Self {
        self.windows_no_window = enabled;
        self
    }
}

/// Opaque handle identifying a child spawned by a [`JobProcessSupervisor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessToken(usize);

impl ProcessToken {
    /// Returns the slot index of this token for diagnostic logging.
    pub const fn index(self) -> usize {
        self.0
    }
}

/// Cross-platform process-tree supervisor backed by a kernel Job Object on Windows.
///
/// # Examples
///
/// ```
/// use t3_process_ffi::supervisor::{JobProcessSupervisor, SpawnSpec, SupervisorError};
///
/// let mut sup = JobProcessSupervisor::new()?;
/// assert!(sup.kill_on_drop());
/// assert!(matches!(sup.spawn(&SpawnSpec::new("")), Err(SupervisorError::EmptyProgram)));
/// # Ok::<(), SupervisorError>(())
/// ```
#[derive(Debug)]
pub struct JobProcessSupervisor {
    #[cfg(windows)]
    job: JobHandle,
    children: Vec<Option<Child>>,
    kill_on_drop: bool,
}

impl JobProcessSupervisor {
    /// Creates a new supervisor (allocating a Win32 kill-on-close Job Object on Windows).
    pub fn new() -> Result<Self, SupervisorError> {
        Ok(Self {
            #[cfg(windows)]
            job: JobHandle::new_kill_on_close()?,
            children: Vec::new(),
            kill_on_drop: true,
        })
    }

    /// Controls whether dropping the supervisor terminates remaining tracked processes.
    pub fn set_kill_on_drop(&mut self, enabled: bool) {
        self.kill_on_drop = enabled;
    }

    /// Returns whether `kill_on_drop` is currently enabled.
    pub const fn kill_on_drop(&self) -> bool {
        self.kill_on_drop
    }

    /// Spawns a child process with sanitized env and RAII pipes, assigning it to the supervisor.
    ///
    /// On any pre-spawn or assignment failure, all allocated pipes are closed immediately
    /// and any half-launched child is terminated and reaped before returning `Err`.
    pub fn spawn(
        &mut self,
        spec: &SpawnSpec,
    ) -> Result<(ProcessToken, ParentPipeSet), SupervisorError> {
        let pipes = ScopedPipeSet::new(spec.stdio).map_err(SupervisorError::PipeSetup)?;
        if spec.program.is_empty() {
            return Err(SupervisorError::EmptyProgram);
        }

        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        spec.env.apply_to_command(&mut command);

        #[cfg(windows)]
        if spec.windows_no_window {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let (mut child, parent_pipes) = pipes
            .spawn_command(&mut command)
            .map_err(SupervisorError::SpawnFailed)?;

        #[cfg(windows)]
        if let Err(err) = self.job.assign_child(&child) {
            let _ = child.kill();
            let _ = child.wait();
            drop(parent_pipes);
            return Err(SupervisorError::Job(err));
        }

        let token = ProcessToken(self.children.len());
        self.children.push(Some(child));
        Ok((token, parent_pipes))
    }

    /// Returns the OS process ID for `token` while the handle is retained.
    pub fn pid(&self, token: ProcessToken) -> Option<u32> {
        self.children
            .get(token.0)
            .and_then(|slot| slot.as_ref().map(Child::id))
    }

    /// Polls whether the supervised child identified by `token` has exited.
    pub fn try_wait(&mut self, token: ProcessToken) -> Result<Option<ExitStatus>, SupervisorError> {
        let Some(slot) = self.children.get_mut(token.0) else {
            return Err(SupervisorError::UnknownChild(token.0));
        };
        let Some(child) = slot.as_mut() else {
            return Ok(None);
        };
        child.try_wait().map_err(SupervisorError::SpawnFailed)
    }

    /// Waits for the supervised child identified by `token` to exit and releases its handle.
    pub fn wait(&mut self, token: ProcessToken) -> Result<ExitStatus, SupervisorError> {
        let Some(slot) = self.children.get_mut(token.0) else {
            return Err(SupervisorError::UnknownChild(token.0));
        };
        let Some(mut child) = slot.take() else {
            return Err(SupervisorError::UnknownChild(token.0));
        };
        child.wait().map_err(SupervisorError::SpawnFailed)
    }

    /// Kills and reaps a single tracked direct child identified by `token`.
    pub fn terminate_child(&mut self, token: ProcessToken) -> Result<(), SupervisorError> {
        let Some(slot) = self.children.get_mut(token.0) else {
            return Err(SupervisorError::UnknownChild(token.0));
        };
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    /// Terminates all processes owned by this supervisor.
    ///
    /// On Windows this executes `TerminateJobObject` directly in the kernel, ending
    /// the entire tree without spawning `taskkill`.
    pub fn terminate_tree(&mut self, exit_code: u32) -> Result<(), SupervisorError> {
        #[cfg(windows)]
        {
            self.job.terminate(exit_code)?;
        }
        #[cfg(not(windows))]
        let _ = exit_code;

        for slot in &mut self.children {
            if let Some(mut child) = slot.take() {
                #[cfg(not(windows))]
                {
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
        }
        Ok(())
    }
}

impl Drop for JobProcessSupervisor {
    fn drop(&mut self) {
        if self.kill_on_drop {
            let _ = self.terminate_tree(1);
        }
    }
}
