//! Live async SSH `-N -L` tunnel subprocess manager and `tailscale` CLI controller.
//!
//! Integrates [`crate::tunnel::ssh`] and [`crate::tunnel::tailscale`] with real `tokio::process`
//! child lifecycle management and HTTP `/.well-known/legioncode/environment` readiness probing.

#[path = "live_ssh_tailscale_proc.rs"]
mod proc_io;
#[path = "live_ssh_tailscale_ts.rs"]
mod ts_ctl;
#[path = "live_ssh_tailscale_types.rs"]
mod types;

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use reqwest::Client;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tunnel::ssh::{
    BatchMode, SshTarget, SshTunnelStateMachine, TunnelEndpoints, TunnelEvent,
    build_direct_tunnel_args, remote_state_key,
};
use proc_io::{
    WellKnownProbe, build_no_redirect_http_client, probe_well_known_environment,
    push_redacted_stderr_tail,
};
pub use ts_ctl::LiveTailscaleController;
use types::into_early_exit_error;
pub use types::{LiveSshError, LiveSshForwardHandle};

const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(2_000);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Live subprocess manager spawning `ssh -N -L` forwards and probing loopback environments.
///
/// # Examples
///
/// ```
/// use legion_remote::live_network::LiveSshTunnelManager;
///
/// let mgr = LiveSshTunnelManager::new()?.with_ssh_binary("ssh");
/// assert_eq!(mgr.ssh_binary(), "ssh");
/// # Ok::<(), legion_remote::live_network::LiveSshError>(())
/// ```
#[derive(Debug, Clone)]
pub struct LiveSshTunnelManager {
    ssh_binary: String,
    extra_env: Vec<(String, String)>,
    http: Client,
    probe_timeout: Duration,
}

impl LiveSshTunnelManager {
    /// Creates a [`LiveSshTunnelManager`] using `"ssh"` and a non-redirecting HTTP client.
    pub fn new() -> Result<Self, LiveSshError> {
        let http = build_no_redirect_http_client().map_err(|err| LiveSshError::HttpClient {
            detail: err.to_string(),
        })?;
        Ok(Self {
            ssh_binary: "ssh".to_owned(),
            extra_env: Vec::new(),
            http,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
        })
    }

    /// Overrides the `ssh` executable path (useful for wrappers or integration test scripts).
    #[must_use]
    pub fn with_ssh_binary(mut self, binary: impl Into<String>) -> Self {
        self.ssh_binary = binary.into();
        self
    }

    /// Adds an environment variable passed to spawned `ssh` processes.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// Sets the timeout for individual loopback `/.well-known/legioncode/environment` HTTP probes.
    #[must_use]
    pub fn with_probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = timeout;
        self
    }

    /// Returns the configured `ssh` executable name or path.
    pub fn ssh_binary(&self) -> &str {
        &self.ssh_binary
    }

    /// Probes `http://127.0.0.1:<local_port>/.well-known/legioncode/environment`.
    pub async fn probe_environment(
        &self,
        local_port: u16,
    ) -> Result<ExecutionEnvironmentDescriptor, LiveSshError> {
        let base = format!("http://127.0.0.1:{local_port}");
        match probe_well_known_environment(&self.http, &base, self.probe_timeout).await {
            WellKnownProbe::LegionEnvironment(desc) => Ok(*desc),
            WellKnownProbe::NotLegion(status) => Err(LiveSshError::ProbeFailed {
                url: base,
                detail: format!("HTTP {status} did not return a valid LegionCode descriptor"),
            }),
            WellKnownProbe::Unreachable(detail) => {
                Err(LiveSshError::ProbeFailed { url: base, detail })
            }
        }
    }

    /// Spawns `ssh -N -L <local_port>:127.0.0.1:<remote_port>` without waiting for readiness.
    pub fn spawn_forward(
        &self,
        target: &SshTarget,
        local_port: u16,
        remote_port: u16,
        batch_mode: BatchMode,
    ) -> Result<LiveSshForwardHandle, LiveSshError> {
        let args = build_direct_tunnel_args(target, local_port, remote_port, batch_mode)?;
        let mut machine = SshTunnelStateMachine::new(batch_mode);
        let _ = machine.handle(TunnelEvent::BeginResolve(target.clone()));
        let _ = machine.handle(TunnelEvent::ResolveSucceeded(target.clone()));
        let _ = machine.handle(TunnelEvent::RemoteLaunched {
            remote_port,
            local_port,
            remote_server_kind: None,
        });

        let mut cmd = Command::new(&self.ssh_binary);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let mut drain_tasks = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            drain_tasks.push(tokio::spawn(async move {
                let mut sink = [0_u8; 4096];
                while matches!(stdout.read(&mut sink).await, Ok(n) if n > 0) {}
            }));
        }
        if let Some(mut stderr) = child.stderr.take() {
            let tail_ref = Arc::clone(&stderr_tail);
            drain_tasks.push(tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut guard) = tail_ref.lock() {
                        push_redacted_stderr_tail(&mut guard, &buf[..n]);
                    }
                }
            }));
        }

        let endpoints = TunnelEndpoints {
            key: remote_state_key(target),
            target: target.clone(),
            local_port,
            remote_port,
            http_base_url: format!("http://127.0.0.1:{local_port}/"),
            ws_base_url: format!("ws://127.0.0.1:{local_port}/"),
            remote_server_kind: None,
        };
        Ok(LiveSshForwardHandle {
            endpoints,
            machine,
            child,
            stderr_tail,
            drain_tasks,
        })
    }

    /// Spawns `ssh -N -L` and polls `/.well-known/legioncode/environment` until ready or early exit.
    pub async fn start_forward_and_wait(
        &self,
        target: &SshTarget,
        local_port: u16,
        remote_port: u16,
        batch_mode: BatchMode,
        readiness_timeout: Duration,
    ) -> Result<(LiveSshForwardHandle, ExecutionEnvironmentDescriptor), LiveSshError> {
        let mut handle = self.spawn_forward(target, local_port, remote_port, batch_mode)?;
        let deadline = Instant::now() + readiness_timeout;

        loop {
            let probe_or_exit = tokio::select! {
                status = handle.child.wait() => {
                    let exit_code = status.ok().and_then(|s| s.code());
                    return Err(into_early_exit_error(handle, exit_code).await);
                }
                probe_res = self.probe_environment(local_port) => probe_res,
            };

            if let Ok(descriptor) = probe_or_exit {
                let _ = handle.machine.handle(TunnelEvent::ForwardListening);
                let _ = handle.machine.handle(TunnelEvent::HttpReady);
                return Ok((handle, descriptor));
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let tail = handle.stderr_tail();
                let _ = handle.stop().await;
                return Err(LiveSshError::ReadinessTimeout {
                    local_port,
                    stderr_tail: tail,
                });
            }

            tokio::select! {
                status = handle.child.wait() => {
                    let exit_code = status.ok().and_then(|s| s.code());
                    return Err(into_early_exit_error(handle, exit_code).await);
                }
                () = tokio::time::sleep(remaining.min(POLL_INTERVAL)) => {}
            }
        }
    }
}
