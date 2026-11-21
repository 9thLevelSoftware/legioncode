//! Connection liveness probe timeouts and dead/silent-socket detection state machine.
//!
//! Mirrors `probeTimeoutFor`, `awaitSilentSocket`, and `monitorConnectedLease` in
//! `packages/client-runtime/src/connection/supervisor.ts` and `wakeups.ts`.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::probe::{
//!     ConnectionRouteKind, ConnectionWakeup, ProbeAction, ProbeEndReason, ProbeSignal,
//!     SilentSocketStateMachine, probe_timeout_for,
//! };
//!
//! assert_eq!(
//!     probe_timeout_for(ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveLong), None),
//!     Some(1_500)
//! );
//! assert_eq!(
//!     probe_timeout_for(
//!         ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
//!         Some(ConnectionRouteKind::Lan),
//!     ),
//!     Some(3_000)
//! );
//! assert_eq!(
//!     probe_timeout_for(
//!         ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe),
//!         Some(ConnectionRouteKind::Relay),
//!     ),
//!     Some(15_000)
//! );
//!
//! let mut machine = SilentSocketStateMachine::new(Some(ConnectionRouteKind::Relay));
//! let _ = machine.on_signal(ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveProbe), 0);
//! assert!(machine.on_tick(499, 0).is_empty());
//! assert_eq!(machine.on_tick(500, 0), vec![ProbeAction::LoadInitialConfig]);
//! assert_eq!(machine.on_config_loaded(true, 0, 500), vec![ProbeAction::CheckHost]);
//! assert!(machine.on_host_check_result(true, 0, 500).is_empty());
//! assert_eq!(machine.on_tick(2_499, 0), vec![ProbeAction::PollInboundFrames]);
//! assert_eq!(
//!     machine.on_tick(2_500, 0),
//!     vec![ProbeAction::ReplaceSocket {
//!         reason: ProbeEndReason::SilentSocket,
//!     }]
//! );
//! ```

use serde::{Deserialize, Serialize};

pub use crate::probe_machine::{SilentSocketMachine, SilentSocketStateMachine};

/// Probe timeout in milliseconds for remote routes and desktop/web foreground wakeups (`15_000` ms).
pub const CONNECTION_PROBE_TIMEOUT_MS: u64 = 15_000;

/// Probe timeout in milliseconds for loopback/LAN routes, explicit retry, and offline reports (`3_000` ms).
pub const QUICK_CONNECTION_PROBE_TIMEOUT_MS: u64 = 3_000;

/// Probe timeout in milliseconds after a long application suspension (`1_500` ms).
pub const LONG_RESUME_PROBE_TIMEOUT_MS: u64 = 1_500;

/// Delay in milliseconds before a probe begins checking whether the socket is silent (`500` ms).
pub const SILENT_SOCKET_CHECK_AFTER_MS: u64 = 500;

/// Interval in milliseconds between inbound frame count polls while checking silence (`100` ms).
pub const FRAME_CHECK_EVERY_MS: u64 = 100;

/// Retry interval in milliseconds between `checkHost` probes when the host fails to answer (`1_000` ms).
pub const SILENT_SOCKET_RECHECK_EVERY_MS: u64 = 1_000;

/// Minimum socket silence duration in milliseconds before replacing a remote route (`2_500` ms).
pub const REMOTE_SILENT_SOCKET_AFTER_MS: u64 = 2_500;

/// Transport route kind for the active connection lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionRouteKind {
    /// T3 Connect cloud relay route.
    Relay,
    /// Local loopback address (`127.0.0.1`, `localhost`, `::1`).
    Loopback,
    /// Private RFC 1918 local network address.
    Lan,
    /// Tailscale / tailnet address.
    Tailnet,
    /// Shared address space / VPN (`100.64.0.0/10`).
    Vpn,
    /// Public internet direct address.
    Public,
    /// SSH-tunneled route.
    Ssh,
}

impl ConnectionRouteKind {
    /// Returns `true` for routes that answer reliably within 3 seconds (`Loopback` and `Lan`).
    pub const fn answers_quickly(self) -> bool {
        matches!(self, Self::Loopback | Self::Lan)
    }
}

/// Device network reachability state reported by the host platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkStatus {
    /// Reachability has not been determined.
    Unknown,
    /// Host platform reports no network connectivity.
    Offline,
    /// Host platform reports network connectivity.
    Online,
}

/// Platform wakeup reason emitted by `ConnectionWakeups`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionWakeup {
    /// Web/desktop window became active.
    ApplicationActive,
    /// Mobile short foreground probe.
    ApplicationActiveProbe,
    /// Mobile/desktop return after a long suspension.
    ApplicationActiveLong,
    /// Window focus gained (does not reach supervisor probe loop).
    ApplicationFocused,
    /// Account or token credentials changed.
    CredentialsChanged,
    /// Platform network interface changed.
    NetworkChanged,
}

impl ConnectionWakeup {
    /// Returns `true` for foreground/resume activation wakeups.
    pub const fn is_application_active(self) -> bool {
        matches!(
            self,
            Self::ApplicationActive | Self::ApplicationActiveProbe | Self::ApplicationActiveLong
        )
    }

    /// Returns `true` when this wakeup resets the reconnect backoff ladder.
    pub const fn resets_retry_backoff(self) -> bool {
        self.is_application_active() || matches!(self, Self::NetworkChanged)
    }

    /// Returns `true` when this wakeup should trigger a shell resubscription.
    pub const fn should_resubscribe_after_wakeup(self) -> bool {
        self.is_application_active()
    }

    /// Returns `true` when this wakeup reaches `EnvironmentSupervisor` (`!= ApplicationFocused`).
    pub const fn reaches_connection_supervisor(self) -> bool {
        !matches!(self, Self::ApplicationFocused)
    }
}

/// Signal delivered to a connected lease monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProbeSignal {
    /// Explicit connect requested.
    ConnectRequested,
    /// Explicit disconnect requested.
    DisconnectRequested,
    /// User/UI requested an immediate retry.
    RetryRequested,
    /// Platform network status transitioned.
    NetworkChanged(NetworkStatus),
    /// Platform wakeup event.
    Wakeup(ConnectionWakeup),
    /// A higher-ranked route answered preflight.
    BetterRouteAvailable,
}

/// Computes the liveness probe timeout in milliseconds for `signal` and `route_kind`, if any.
pub const fn probe_timeout_for(
    signal: ProbeSignal,
    route_kind: Option<ConnectionRouteKind>,
) -> Option<u64> {
    match signal {
        ProbeSignal::RetryRequested => Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS),
        ProbeSignal::NetworkChanged(NetworkStatus::Offline) => {
            Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS)
        }
        ProbeSignal::NetworkChanged(NetworkStatus::Online | NetworkStatus::Unknown)
        | ProbeSignal::ConnectRequested
        | ProbeSignal::DisconnectRequested
        | ProbeSignal::BetterRouteAvailable => None,
        ProbeSignal::Wakeup(reason) => match reason {
            ConnectionWakeup::ApplicationActive => Some(CONNECTION_PROBE_TIMEOUT_MS),
            ConnectionWakeup::ApplicationActiveLong => Some(LONG_RESUME_PROBE_TIMEOUT_MS),
            ConnectionWakeup::ApplicationActiveProbe | ConnectionWakeup::NetworkChanged => {
                if matches!(route_kind, Some(kind) if kind.answers_quickly()) {
                    Some(QUICK_CONNECTION_PROBE_TIMEOUT_MS)
                } else {
                    Some(CONNECTION_PROBE_TIMEOUT_MS)
                }
            }
            ConnectionWakeup::ApplicationFocused | ConnectionWakeup::CredentialsChanged => None,
        },
    }
}

/// Why [`SilentSocketStateMachine`] instructed the runtime to replace the live socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProbeEndReason {
    /// Host answered a fresh HTTP check while the WebSocket delivered zero inbound frames.
    SilentSocket,
    /// A long-resume probe expired before the socket responded.
    LongResumeTimeout,
}

/// Imperative side-effect emitted by [`SilentSocketStateMachine`] for the host adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProbeAction {
    /// Start (or restart for follow-up) the RPC probe request on the active session.
    StartProbe,
    /// Check whether `session.initialConfig` has finished loading before polling frames.
    LoadInitialConfig,
    /// Issue a fresh out-of-band `driver.checkHost` HTTP request.
    CheckHost,
    /// Poll the session's inbound frame counter (`100` ms cadence).
    PollInboundFrames,
    /// Close and replace the live socket immediately without blaming the retry ladder.
    ReplaceSocket {
        /// Cause of socket replacement.
        reason: ProbeEndReason,
    },
    /// Probe timed out; emit a transient timeout error and enter reconnect backoff.
    ProbeTimedOut,
    /// Probe completed cleanly; lease remains healthy.
    ProbeHealthy,
    /// Probe request itself returned an error; reconnect immediately.
    ProbeFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MonitorState {
    pub(crate) started_at_ms: u64,
    pub(crate) frames_before: u64,
    pub(crate) next_frame_poll_ms: u64,
    pub(crate) next_host_check_ms: Option<u64>,
    pub(crate) host_check_in_flight: bool,
    pub(crate) remote_floor_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SilentCheckPhase {
    Idle,
    WaitingInitialCheck { started_at_ms: u64, due_at_ms: u64 },
    AwaitingConfig { started_at_ms: u64 },
    ConfigUnavailable,
    Monitoring(MonitorState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeMode {
    Standard,
    Long { started_at_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveProbe {
    Idle,
    Running {
        probe_timeout_ms: u64,
        deadline_ms: u64,
        tolerates_miss: bool,
        resume: ResumeMode,
        recheck_queued: bool,
    },
}

pub(crate) const fn is_foreground_probe(signal: ProbeSignal) -> bool {
    matches!(
        signal,
        ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActive)
    )
}

pub(crate) const fn is_long_resume(signal: ProbeSignal) -> bool {
    matches!(
        signal,
        ProbeSignal::Wakeup(ConnectionWakeup::ApplicationActiveLong)
    )
}
