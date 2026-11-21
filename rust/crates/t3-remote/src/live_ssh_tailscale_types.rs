//! Types, errors, and live child handle for [`super::LiveSshTunnelManager`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;
use tokio::process::Child;
use tokio::task::JoinHandle;

use crate::tunnel::ssh::{
    SshError, SshTunnelStateMachine, TunnelAction, TunnelEndpoints, TunnelEvent,
};
use crate::tunnel::tailscale::{TailscaleError, TailscaleStderrDiagnostic};

/// Errors produced by [`super::LiveSshTunnelManager`] and [`LiveSshForwardHandle`].
#[derive(Debug, Error)]
pub enum LiveSshError {
    /// Failed to construct the internal HTTP readiness client.
    #[error("failed to initialize HTTP readiness client: {detail}")]
    HttpClient {
        /// Underlying client setup error.
        detail: String,
    },
    /// Target validation or SSH argument construction failed.
    #[error(transparent)]
    Ssh(#[from] SshError),
    /// Failed to spawn or manage the `ssh` subprocess.
    #[error("failed to run ssh subprocess: {source}")]
    Process {
        /// Underlying OS process error.
        #[from]
        source: std::io::Error,
    },
    /// The `ssh` subprocess exited before forwarding became ready.
    #[error("ssh tunnel exited early (code {exit_code:?}): {stderr_tail}")]
    ExitedEarly {
        /// Process exit code, if available.
        exit_code: Option<i32>,
        /// Secret-redacted tail of `ssh` stderr.
        stderr_tail: String,
    },
    /// Loopback `/.well-known/t3/environment` did not answer within the startup deadline.
    #[error("ssh tunnel on 127.0.0.1:{local_port} did not become ready in time: {stderr_tail}")]
    ReadinessTimeout {
        /// Local forwarded TCP port.
        local_port: u16,
        /// Secret-redacted tail of `ssh` stderr.
        stderr_tail: String,
    },
    /// Loopback probe failed or answered with non-T3 content.
    #[error("loopback environment probe failed on {url}: {detail}")]
    ProbeFailed {
        /// Probed loopback URL.
        url: String,
        /// Failure summary.
        detail: String,
    },
}

/// Active `ssh -N -L` child process handle with bounded stderr tail capture and clean teardown.
#[derive(Debug)]
pub struct LiveSshForwardHandle {
    pub(super) endpoints: TunnelEndpoints,
    pub(super) machine: SshTunnelStateMachine,
    pub(super) child: Child,
    pub(super) stderr_tail: Arc<Mutex<String>>,
    pub(super) drain_tasks: Vec<JoinHandle<()>>,
}

impl LiveSshForwardHandle {
    /// Returns the local/remote endpoint metadata for this tunnel.
    pub fn endpoints(&self) -> &TunnelEndpoints {
        &self.endpoints
    }

    /// Returns a snapshot of the driving [`SshTunnelStateMachine`].
    pub fn state_machine(&self) -> &SshTunnelStateMachine {
        &self.machine
    }

    /// Returns the child OS PID while running, or `None` after exit.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Returns the secret-redacted `stderr` tail captured so far.
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Checks non-blockingly whether the `ssh` child process has exited (`Some(exit_code)`).
    pub fn try_exit_status(&mut self) -> Result<Option<Option<i32>>, LiveSshError> {
        let status = self.child.try_wait()?;
        Ok(status.map(|s| s.code()))
    }

    /// Terminates the `ssh` subprocess cleanly and drains its output tasks.
    pub async fn stop(mut self) -> Result<Vec<TunnelAction>, LiveSshError> {
        let actions = self
            .machine
            .handle(TunnelEvent::Disconnect)
            .unwrap_or_default();
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        self.drain_stderr_tasks().await;
        let _ = self.machine.handle(TunnelEvent::ShutdownCompleted);
        Ok(actions)
    }

    pub(super) async fn drain_stderr_tasks(&mut self) {
        for mut task in self.drain_tasks.drain(..) {
            if tokio::time::timeout(Duration::from_millis(200), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

pub(super) async fn into_early_exit_error(
    mut handle: LiveSshForwardHandle,
    exit_code: Option<i32>,
) -> LiveSshError {
    handle.drain_stderr_tasks().await;
    LiveSshError::ExitedEarly {
        exit_code,
        stderr_tail: handle.stderr_tail(),
    }
}

pub(super) fn map_serve_exit_error(
    diagnostic: Option<TailscaleStderrDiagnostic>,
    serve_port: u16,
    local_host: &str,
) -> TailscaleError {
    match diagnostic {
        Some(TailscaleStderrDiagnostic::NotRunning | TailscaleStderrDiagnostic::NotLoggedIn) => {
            TailscaleError::TailscaleUnavailable
        }
        Some(TailscaleStderrDiagnostic::PermissionDenied) => {
            TailscaleError::TailscaleOperatorRequired
        }
        Some(TailscaleStderrDiagnostic::UnsupportedTarget) => {
            TailscaleError::TailscaleServeTargetRefused {
                local_host: local_host.to_owned(),
            }
        }
        _ => TailscaleError::TailscaleServeFailed { serve_port },
    }
}
