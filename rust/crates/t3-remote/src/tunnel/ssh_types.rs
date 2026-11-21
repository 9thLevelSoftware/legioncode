//! State, event, and action types for [`super::SshTunnelStateMachine`].

use serde::{Deserialize, Serialize};

use super::SshTarget;

/// Maximum characters retained in the SSH tunnel stderr diagnostic ring tail (`tunnel.ts:63`).
pub const TUNNEL_STDERR_TAIL_CHARS: usize = 16_384;

/// Remote server ownership mode recorded during SSH launch (`tunnel.ts:127`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteServerKind {
    /// Pre-existing external `t3` server adopted on loopback.
    External,
    /// Server spawned and owned by this SSH launcher.
    Managed,
}

/// Transport used for the SSH `-L` local port forward (`tunnel.ts:1536-1557`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelTransport {
    /// Direct `ssh -N -L` connection.
    Direct,
    /// Multiplexed forward through an existing `ControlMaster` socket (`ControlPath`).
    ControlMaster {
        /// Path to the live `ControlMaster` UNIX socket.
        control_path: String,
    },
}

/// Live local port-forward tunnel endpoints (`DesktopSshEnvironmentBootstrap`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelEndpoints {
    /// Canonical target connection key (`targetConnectionKey`).
    pub key: String,
    /// Resolved SSH target.
    pub target: SshTarget,
    /// Reserved loopback port forwarding to `remote_port`.
    pub local_port: u16,
    /// Remote `t3` server loopback port.
    pub remote_port: u16,
    /// Local forwarded HTTP base URL (`http://127.0.0.1:<local_port>/`).
    pub http_base_url: String,
    /// Local forwarded WebSocket base URL (`ws://127.0.0.1:<local_port>/`).
    pub ws_base_url: String,
    /// Ownership mode of the remote server.
    pub remote_server_kind: Option<RemoteServerKind>,
}

/// Explicit state of an SSH local port-forward tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshTunnelState {
    /// Initial state before resolving the target alias.
    Idle,
    /// Resolving SSH config (`ssh -G <alias>`).
    Resolving {
        /// Requested SSH target.
        target: SshTarget,
    },
    /// Launching or reusing the remote `t3` server over SSH.
    LaunchingRemote {
        /// Canonical target connection key.
        key: String,
        /// Resolved SSH target.
        target: SshTarget,
    },
    /// Spawning the local `-L` port forward and waiting for the loopback socket to bind.
    StartingForward {
        /// Canonical target connection key.
        key: String,
        /// Resolved SSH target.
        target: SshTarget,
        /// Local loopback port reserved for forwarding.
        local_port: u16,
        /// Remote server loopback port.
        remote_port: u16,
        /// Ownership mode of the remote server.
        remote_server_kind: Option<RemoteServerKind>,
        /// Active tunnel transport (Direct or `ControlMaster`).
        transport: TunnelTransport,
        /// Number of password prompts already attempted.
        auth_prompts: u8,
        /// Whether `ControlMaster` fallback was already tried.
        control_fallback_used: bool,
        /// Bounded tail of child `stderr` bytes decoded as UTF-8.
        stderr_tail: String,
    },
    /// Loopback forward bound; probing HTTP readiness on `http_base_url`.
    ProbingReadiness {
        /// Forwarded endpoints awaiting HTTP 200 readiness.
        endpoints: TunnelEndpoints,
        /// Active tunnel transport.
        transport: TunnelTransport,
    },
    /// Tunnel is live and ready for RPC/HTTP traffic.
    Ready {
        /// Active forwarded endpoints.
        endpoints: TunnelEndpoints,
        /// Active tunnel transport.
        transport: TunnelTransport,
    },
    /// Tunnel is shutting down.
    Closing {
        /// Canonical target connection key.
        key: String,
        /// Whether explicit disconnect should also stop the remote managed server.
        stop_remote: bool,
    },
    /// Tunnel reached terminal closed state.
    Closed,
    /// Tunnel failed with a typed diagnostic reason and bounded stderr tail.
    Failed {
        /// Diagnostic failure message.
        message: String,
        /// Last [`TUNNEL_STDERR_TAIL_CHARS`] characters of child stderr.
        stderr_tail: String,
    },
}

/// Inputs driving transitions on [`super::SshTunnelStateMachine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// Start resolving `target`.
    BeginResolve(SshTarget),
    /// `ssh -G` resolved `target`.
    ResolveSucceeded(SshTarget),
    /// Remote server started/reused on `remote_port` and local loopback `local_port` reserved.
    RemoteLaunched {
        /// Remote `t3` server port.
        remote_port: u16,
        /// Local reserved loopback port.
        local_port: u16,
        /// Ownership mode of the remote server.
        remote_server_kind: Option<RemoteServerKind>,
    },
    /// Tunnel process emitted stderr diagnostics before exit/readiness.
    StderrChunk(String),
    /// Tunnel startup hit an authentication failure (`tunnel.ts:1851-1982`).
    AuthFailed {
        /// Error message from `ssh`.
        error_message: String,
        /// Whether an interactive in-app password prompt is available.
        prompt_available: bool,
        /// Live `ControlMaster` socket path discovered via `findLiveControlPath`, if any.
        live_control_path: Option<String>,
    },
    /// Loopback `-L` forward socket is accepting connections.
    ForwardListening,
    /// Forwarded HTTP endpoint passed readiness probe.
    HttpReady,
    /// Existing tunnel failed readiness check and is stale (`tunnel.ts:2081-2090`).
    StaleDetected,
    /// Explicit caller disconnect (`tunnel.ts:2174-2212`).
    Disconnect,
    /// Tunnel child exited unexpectedly.
    ProcessExited {
        /// Process exit code, if available.
        exit_code: Option<i32>,
    },
    /// Shutdown completed.
    ShutdownCompleted,
}

/// Side-effect actions emitted by [`super::SshTunnelStateMachine::handle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelAction {
    /// Run `ssh -G` to resolve alias config.
    ResolveTarget(SshTarget),
    /// Launch or adopt the remote `t3` server over SSH.
    LaunchRemoteServer {
        /// Target to run remote bootstrap on.
        target: SshTarget,
    },
    /// Spawn the SSH `-L` forward process with the computed argv.
    SpawnForward {
        /// `ssh` CLI arguments (excluding executable).
        args: Vec<String>,
        /// Active transport mode.
        transport: TunnelTransport,
    },
    /// Prompt the user for an SSH password/OTP (`attempt` = `1` or `2`).
    PromptPassword {
        /// 1-based prompt attempt counter (`1..=2`).
        attempt: u8,
    },
    /// Poll loopback port listener until `ForwardListening`.
    WaitForForward {
        /// Local loopback port to probe.
        local_port: u16,
    },
    /// Poll HTTP readiness at `http_base_url`.
    ProbeHttpReady {
        /// Forwarded HTTP base URL.
        http_base_url: String,
    },
    /// Cancel `ControlMaster` forward (`-O cancel`) and terminate forward child; stop remote only if `stop_remote` is true.
    CloseForward {
        /// Optional `-O cancel` args when `transport` was [`TunnelTransport::ControlMaster`].
        control_cancel_args: Option<Vec<String>>,
        /// `true` on explicit disconnect; `false` on stale tunnel teardown (`tunnel.ts:2088`).
        stop_remote: bool,
    },
}
