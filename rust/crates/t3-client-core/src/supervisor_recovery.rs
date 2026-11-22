//! Defect-tolerant connection supervisor recovery (`38bac7091a` / #39420, `003d53ace1` / #39431),
//! route switch cooldowns, and silent-route generation backoff.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backoff::{
    BETTER_ROUTE_CHECK_INTERVAL_MS, BETTER_ROUTE_COOLDOWN_MS, FailureReason, RETRY_BASE_DELAY_MS,
    RETRY_MAX_DELAY_MS, ReconnectBackoff, SILENT_ROUTE_BACKOFF_MAX_MS,
};

/// Delay before restarting a listener fiber whose stream unexpectedly died (`003d53ace1`).
pub const LISTENER_RESTART_DELAY_MS: u64 = RETRY_BASE_DELAY_MS;

/// Supervisor lifecycle state mirroring `EnvironmentSupervisor.state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorPhase {
    /// Target is not currently desired.
    Idle,
    /// Attempting connection or preparing transport.
    Connecting,
    /// Stable connected session live.
    Connected,
    /// Waiting for reconnect backoff before next attempt.
    Backoff,
    /// Host connectivity is offline; waiting for online signal.
    Offline,
    /// Blocked host refusal awaiting external retry signal.
    Blocked,
}

/// Which background listener fiber hit an unexpected defect (`003d53ace1` / #39431).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorListenerKind {
    /// Platform network online/offline stream.
    Connectivity,
    /// Foreground / power-wake / network-change wakeup stream.
    Wakeups,
    /// Periodic better-route probing loop.
    BetterRoute,
}

/// What happened inside a connection supervisor when a defect or disconnect occurred (`#39420`, `#39431`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorDefectKind {
    /// Attempt preparation or connectivity check threw a defect (`generation` untouched if never live).
    AttemptDefect,
    /// Attempt ended by self-interruption (`generation` untouched unless already connected).
    AttemptInterruption,
    /// Live session teardown/release defected; schedules fast reconnect keeping `generation`.
    SessionReleaseDefect,
    /// Outer loop defected outside an attempt; restarts loop with `RETRY_MAX_DELAY_MS` backoff.
    LoopDefect,
}

/// Tracked per-route better-route switch cooldowns (`BETTER_ROUTE_COOLDOWN_MS = 300_000` ms).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RouteCooldownTracker {
    cooldown_until_ms: BTreeMap<String, u64>,
}

impl RouteCooldownTracker {
    /// Creates an empty [`RouteCooldownTracker`].
    pub const fn new() -> Self {
        Self {
            cooldown_until_ms: BTreeMap::new(),
        }
    }

    /// Marks `route_id` as cooled down until `now_ms + BETTER_ROUTE_COOLDOWN_MS`.
    pub fn record_failed_switch(&mut self, route_id: impl Into<String>, now_ms: u64) {
        self.cooldown_until_ms.insert(
            route_id.into(),
            now_ms.saturating_add(BETTER_ROUTE_COOLDOWN_MS),
        );
    }

    /// Returns `true` when `route_id` is currently within its cooldown window at `now_ms`.
    pub fn is_cooled_down(&self, route_id: &str, now_ms: u64) -> bool {
        self.cooldown_until_ms
            .get(route_id)
            .is_some_and(|&until| until > now_ms)
    }

    /// Clears all recorded cooldowns.
    pub fn clear(&mut self) {
        self.cooldown_until_ms.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SilentRouteEntry {
    failures: u32,
    until_ms: u64,
}

/// Generation-scoped backoff for routes that stay silent during better-route preflight checks.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SilentRouteBackoff {
    generation: u64,
    routes: BTreeMap<String, SilentRouteEntry>,
}

impl SilentRouteBackoff {
    /// Creates an empty [`SilentRouteBackoff`] at generation `0`.
    pub const fn new() -> Self {
        Self {
            generation: 0,
            routes: BTreeMap::new(),
        }
    }

    /// Current generation token; capture before an async check and pass when recording results.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Invalidates in-flight checks and forgets all silent route entries (e.g. on network change).
    pub fn forget_all(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.routes.clear();
    }

    /// Records a silent preflight result if `expected_generation` still matches `self.generation()`.
    pub fn record_silent(
        &mut self,
        route_id: impl Into<String>,
        expected_generation: u64,
        now_ms: u64,
    ) -> bool {
        if expected_generation != self.generation {
            return false;
        }
        let key = route_id.into();
        let prev_failures = self.routes.get(&key).map_or(0, |entry| entry.failures);
        let failures = prev_failures.saturating_add(1);
        let exp = failures.min(16);
        let delay_ms = BETTER_ROUTE_CHECK_INTERVAL_MS
            .saturating_mul(1_u64 << exp)
            .min(SILENT_ROUTE_BACKOFF_MAX_MS);
        self.routes.insert(
            key,
            SilentRouteEntry {
                failures,
                until_ms: now_ms.saturating_add(delay_ms),
            },
        );
        true
    }

    /// Clears a route's silent backoff once it responds, if `expected_generation` is current.
    pub fn record_responsive(&mut self, route_id: &str, expected_generation: u64) -> bool {
        if expected_generation != self.generation {
            return false;
        }
        self.routes.remove(route_id);
        true
    }

    /// Returns `true` if `route_id` is backed off past `now_ms`.
    pub fn is_backed_off(&self, route_id: &str, now_ms: u64) -> bool {
        self.routes
            .get(route_id)
            .is_some_and(|entry| entry.until_ms > now_ms)
    }
}

/// Pure connection-supervisor state tracking monotonic `generation`, attempt backoff,
/// defect recovery (`#39420`), and background listener restart scheduling (`#39431`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentSupervisorState {
    /// Current connection phase.
    pub phase: SupervisorPhase,
    /// Current attempt number in this cycle (`0` when fresh).
    pub attempt: u32,
    /// Monotonically increasing live-session generation across reconnects and defects.
    pub generation: u64,
    /// Epoch timestamp in milliseconds when the next reconnect attempt becomes due.
    pub retry_at_ms: Option<u64>,
    /// Reason recorded on the latest failure or defect.
    pub last_failure: Option<FailureReason>,
    /// True when host network status is offline.
    pub network_offline: bool,
    /// Scheduled restart deadlines in milliseconds for crashed background listener fibers.
    pub listener_restart_at_ms: BTreeMap<SupervisorListenerKind, u64>,
    /// Total restarts per listener kind for diagnostics/tests.
    pub listener_restarts: BTreeMap<SupervisorListenerKind, u32>,
    backoff: ReconnectBackoff,
}

impl Default for EnvironmentSupervisorState {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvironmentSupervisorState {
    /// Creates a fresh idle supervisor state at generation `0`.
    pub fn new() -> Self {
        Self {
            phase: SupervisorPhase::Idle,
            attempt: 0,
            generation: 0,
            retry_at_ms: None,
            last_failure: None,
            network_offline: false,
            listener_restart_at_ms: BTreeMap::new(),
            listener_restarts: BTreeMap::new(),
            backoff: ReconnectBackoff::new(),
        }
    }

    /// Transitions into `Connecting` for an upcoming attempt.
    pub fn start_connecting(&mut self) {
        self.phase = SupervisorPhase::Connecting;
        self.retry_at_ms = None;
    }

    /// Promotes the active attempt to `Connected` and increments `generation` monotonically.
    pub fn on_connected(&mut self) -> u64 {
        self.phase = SupervisorPhase::Connected;
        self.generation = self.generation.saturating_add(1);
        self.attempt = 0;
        self.retry_at_ms = None;
        self.last_failure = None;
        self.backoff.reset();
        self.generation
    }

    /// Transitions to [`SupervisorPhase::Blocked`] when a host refuses access until retried.
    pub fn on_blocked(&mut self) {
        self.phase = SupervisorPhase::Blocked;
        self.retry_at_ms = None;
        self.last_failure = Some(FailureReason::Transient);
    }

    /// Handles a normal transient or approval-pending connection failure.
    pub fn on_failure(
        &mut self,
        reason: FailureReason,
        was_stable: bool,
        now_ms: u64,
        random: f64,
    ) {
        let decision = self.backoff.record_failure(reason, was_stable, random);
        self.attempt = decision.attempt;
        self.last_failure = Some(reason);
        if self.network_offline {
            self.phase = SupervisorPhase::Offline;
            self.retry_at_ms = None;
        } else {
            self.phase = SupervisorPhase::Backoff;
            self.retry_at_ms = Some(now_ms.saturating_add(decision.delay_ms));
        }
    }

    /// Recovers from an unexpected defect inside an attempt, session release, or outer loop
    /// without permanently halting the supervisor (`38bac7091a` / #39420).
    pub fn on_defect(&mut self, kind: SupervisorDefectKind, now_ms: u64, random: f64) {
        match kind {
            SupervisorDefectKind::AttemptDefect => {
                self.on_failure(FailureReason::Transient, false, now_ms, random);
            }
            SupervisorDefectKind::AttemptInterruption => {
                let decision = self
                    .backoff
                    .record_failure(FailureReason::Transient, false, random);
                self.attempt = decision.attempt;
                self.last_failure = None;
                self.phase = if self.network_offline {
                    SupervisorPhase::Offline
                } else {
                    SupervisorPhase::Backoff
                };
                self.retry_at_ms =
                    (!self.network_offline).then(|| now_ms.saturating_add(decision.delay_ms));
            }
            SupervisorDefectKind::SessionReleaseDefect => {
                self.on_failure(FailureReason::Transient, true, now_ms, random);
            }
            SupervisorDefectKind::LoopDefect => {
                self.attempt = self.attempt.saturating_add(1).max(1);
                self.last_failure = Some(FailureReason::Transient);
                if self.network_offline {
                    self.phase = SupervisorPhase::Offline;
                    self.retry_at_ms = None;
                } else {
                    self.phase = SupervisorPhase::Backoff;
                    self.retry_at_ms = Some(now_ms.saturating_add(RETRY_MAX_DELAY_MS));
                }
            }
        }
    }

    /// Updates connectivity status (`#39420` / `#39431`); returning online moves `Offline -> Connecting`.
    pub fn set_network_offline(&mut self, offline: bool) {
        self.network_offline = offline;
        if !offline && self.phase == SupervisorPhase::Offline {
            self.phase = SupervisorPhase::Connecting;
            self.retry_at_ms = None;
        } else if offline
            && matches!(
                self.phase,
                SupervisorPhase::Backoff | SupervisorPhase::Connecting
            )
        {
            self.phase = SupervisorPhase::Offline;
            self.retry_at_ms = None;
        }
    }

    /// Records a background listener stream defect and schedules restart after `LISTENER_RESTART_DELAY_MS`
    /// (`003d53ace1` / #39431) rather than abandoning the listener permanently.
    pub fn on_listener_defect(&mut self, kind: SupervisorListenerKind, now_ms: u64) -> u64 {
        let count = self.listener_restarts.entry(kind).or_insert(0);
        *count = count.saturating_add(1);
        let delay = match kind {
            SupervisorListenerKind::BetterRoute => BETTER_ROUTE_CHECK_INTERVAL_MS,
            SupervisorListenerKind::Connectivity | SupervisorListenerKind::Wakeups => {
                LISTENER_RESTART_DELAY_MS
            }
        };
        let restart_at = now_ms.saturating_add(delay);
        self.listener_restart_at_ms.insert(kind, restart_at);
        restart_at
    }

    /// Marks `kind` listener fiber restarted once `now_ms >= restart_at`.
    pub fn consume_due_listener_restart(
        &mut self,
        kind: SupervisorListenerKind,
        now_ms: u64,
    ) -> bool {
        let due = self
            .listener_restart_at_ms
            .get(&kind)
            .is_some_and(|&at| now_ms >= at);
        if due {
            self.listener_restart_at_ms.remove(&kind);
        }
        due
    }
}
