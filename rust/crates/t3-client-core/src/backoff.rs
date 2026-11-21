//! Jittered reconnect backoff, session-loss retry, approval-pending pacing, and route cooldowns.
//!
//! Mirrors `packages/client-runtime/src/connection/supervisor.ts` (`RETRY_BASE_DELAY_MS`,
//! `RETRY_MAX_DELAY_MS`, `BACKOFF_RESET_AFTER_MS`, `SESSION_LOSS_RETRY_DELAY_MS`,
//! `APPROVAL_PENDING_*`, `BETTER_ROUTE_COOLDOWN_MS`, and `SILENT_ROUTE_BACKOFF_MAX_MS`) in pure
//! target-agnostic Rust without timers or platform threads.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::backoff::{
//!     APPROVAL_PENDING_RETRY_DELAY_MS, FailureReason, ReconnectBackoff,
//! };
//!
//! let mut backoff = ReconnectBackoff::new();
//! let first_loss = backoff.record_failure(FailureReason::Transient, true, 0.0);
//! assert_eq!(first_loss.delay_ms, 125);
//!
//! backoff.reset();
//! let pending = backoff.record_failure(FailureReason::ApprovalPending, false, 0.9);
//! assert_eq!(pending.delay_ms, APPROVAL_PENDING_RETRY_DELAY_MS);
//! assert!(pending.awaiting_approval);
//! assert_eq!(backoff.backoff_failures(), 0);
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Base delay in milliseconds used for exponential reconnect backoff (`1_000` ms).
pub const RETRY_BASE_DELAY_MS: u64 = 1_000;

/// Maximum exponential reconnect backoff ceiling in milliseconds (`300_000` ms / 5 minutes).
pub const RETRY_MAX_DELAY_MS: u64 = 300_000;

/// Minimum connected duration in milliseconds required before resetting backoff (`30_000` ms).
pub const BACKOFF_RESET_AFTER_MS: u64 = 30_000;

/// Maximum delay in milliseconds for the first loss of an established session (`250` ms, `[125..=250]`).
pub const SESSION_LOSS_RETRY_DELAY_MS: u64 = 250;

/// Fixed retry delay in milliseconds while awaiting host device approval (`4_000` ms).
pub const APPROVAL_PENDING_RETRY_DELAY_MS: u64 = 4_000;

/// Number of fast `approval-pending` retries before falling back to exponential backoff (`150`).
pub const APPROVAL_PENDING_FAST_RETRIES: u32 = 150;

/// Cooldown duration in milliseconds after switching to a preferred route fails (`300_000` ms).
pub const BETTER_ROUTE_COOLDOWN_MS: u64 = 300_000;

/// Periodic interval in milliseconds for checking whether a higher-ranked route answers (`60_000` ms).
pub const BETTER_ROUTE_CHECK_INTERVAL_MS: u64 = 60_000;

/// Maximum backoff in milliseconds for a route that stays silent across checks (`1_800_000` ms).
pub const SILENT_ROUTE_BACKOFF_MAX_MS: u64 = 1_800_000;

/// Per-route establishment timeout budget in milliseconds (`15_000` ms).
pub const CONNECTION_ESTABLISHMENT_TIMEOUT_MS: u64 = 15_000;

/// Extra timeout budget in milliseconds added per route reaching `synchronizing` (`75_000` ms).
pub const SYNCHRONIZATION_TIMEOUT_MS: u64 = 75_000;

/// Failure classification driving reconnect backoff pacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureReason {
    /// Device approval is pending on the host (`reason === "approval-pending"`).
    ApprovalPending,
    /// Any other transient connection failure.
    Transient,
}

/// Output produced when recording a transient failure into [`ReconnectBackoff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackoffDecision {
    /// Delay in milliseconds before the next reconnect attempt.
    pub delay_ms: u64,
    /// 1-based failure attempt counter since the last reset.
    pub attempt: u32,
    /// Whether this failure fell inside the `approval-pending` fast-poll window.
    pub awaiting_approval: bool,
    /// Whether this failure used the immediate first-session-loss fast retry (`[125..=250]` ms).
    pub first_loss: bool,
}

/// Rounds `value` using JS `Math.round` semantics (`floor(value + 0.5)`) inside `[min_val, max_val]`.
fn round_bounded_f64_to_u32(value: f64, min_val: u32, max_val: u32) -> u32 {
    let mut lo = min_val;
    let mut hi = max_val;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if f64::from(mid) - 0.5 <= value {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

fn clamp_unit_random(random: f64) -> f64 {
    if random.is_nan() {
        0.0
    } else {
        random.clamp(0.0, 1.0)
    }
}

/// Computes the exponential backoff ceiling for 0-based `backoff_failure_index`.
///
/// Matches `Math.min(300_000, 1_000 * 2 ** (failureCount + 1))`.
///
/// # Examples
///
/// ```
/// use t3_client_core::backoff::{RETRY_MAX_DELAY_MS, retry_delay_ceiling_ms};
///
/// assert_eq!(retry_delay_ceiling_ms(0), 2_000);
/// assert_eq!(retry_delay_ceiling_ms(1), 4_000);
/// assert_eq!(retry_delay_ceiling_ms(8), RETRY_MAX_DELAY_MS);
/// ```
pub fn retry_delay_ceiling_ms(backoff_failure_index: u32) -> u64 {
    let exp = backoff_failure_index.saturating_add(1).min(20);
    RETRY_BASE_DELAY_MS
        .saturating_mul(1_u64 << exp)
        .min(RETRY_MAX_DELAY_MS)
}

/// Computes jittered reconnect delay in `[ceiling / 2, ceiling]` for `backoff_failure_index`.
///
/// `random` is clamped to `[0.0, 1.0]` (`NaN` maps to `0.0`).
///
/// # Examples
///
/// ```
/// use t3_client_core::backoff::jittered_retry_delay_ms;
///
/// assert_eq!(jittered_retry_delay_ms(0, 0.0), 1_000);
/// assert_eq!(jittered_retry_delay_ms(0, 1.0), 2_000);
/// assert_eq!(jittered_retry_delay_ms(20, 0.0), 150_000);
/// assert_eq!(jittered_retry_delay_ms(20, 1.0), 300_000);
/// ```
pub fn jittered_retry_delay_ms(backoff_failure_index: u32, random: f64) -> u64 {
    let ceiling = retry_delay_ceiling_ms(backoff_failure_index);
    let half = u32::try_from(ceiling / 2).unwrap_or(u32::MAX);
    let max_val = u32::try_from(ceiling).unwrap_or(u32::MAX);
    let r = clamp_unit_random(random);
    let raw = f64::from(half) + f64::from(half) * r;
    u64::from(round_bounded_f64_to_u32(raw, half, max_val))
}

/// Computes the first-loss retry delay for an established session in `[125..=250]` ms.
///
/// Matches `Math.round(SESSION_LOSS_RETRY_DELAY_MS * (0.5 + random / 2))`.
///
/// # Examples
///
/// ```
/// use t3_client_core::backoff::session_loss_retry_delay_ms;
///
/// assert_eq!(session_loss_retry_delay_ms(0.0), 125);
/// assert_eq!(session_loss_retry_delay_ms(0.5), 188);
/// assert_eq!(session_loss_retry_delay_ms(1.0), 250);
/// ```
pub fn session_loss_retry_delay_ms(random: f64) -> u64 {
    let min_val = u32::try_from(SESSION_LOSS_RETRY_DELAY_MS / 2).unwrap_or(125);
    let max_val = u32::try_from(SESSION_LOSS_RETRY_DELAY_MS).unwrap_or(250);
    let r = clamp_unit_random(random);
    let raw = f64::from(max_val) * (0.5 + r * 0.5);
    u64::from(round_bounded_f64_to_u32(raw, min_val, max_val))
}

/// Returns `true` when an established connection stayed up long enough to reset retry backoff.
pub const fn is_stable_connection_ms(connected_for_ms: u64) -> bool {
    connected_for_ms >= BACKOFF_RESET_AFTER_MS
}

/// Stateful reconnect backoff ladder matching `EnvironmentSupervisor`'s retry loop.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectBackoff {
    failure_count: u32,
    backoff_failures: u32,
    approval_waits: u32,
}

impl ReconnectBackoff {
    /// Creates a fresh [`ReconnectBackoff`] at attempt `0`.
    pub const fn new() -> Self {
        Self {
            failure_count: 0,
            backoff_failures: 0,
            approval_waits: 0,
        }
    }
    /// Total transient failures observed since the last reset.
    pub const fn failure_count(&self) -> u32 {
        self.failure_count
    }
    /// Exponential backoff steps consumed (excludes fast `approval-pending` waits).
    pub const fn backoff_failures(&self) -> u32 {
        self.backoff_failures
    }
    /// Count of `approval-pending` failures observed since the last reset.
    pub const fn approval_waits(&self) -> u32 {
        self.approval_waits
    }
    /// 1-based attempt number for the next connection attempt (`failure_count + 1`).
    pub const fn next_attempt(&self) -> u32 {
        self.failure_count.saturating_add(1)
    }
    /// Resets failure, backoff, and approval counters back to `0`.
    pub const fn reset(&mut self) {
        self.failure_count = 0;
        self.backoff_failures = 0;
        self.approval_waits = 0;
    }
    /// Resets the ladder when an established session lasted at least [`BACKOFF_RESET_AFTER_MS`].
    pub const fn on_session_ended(&mut self, connected_for_ms: u64) {
        if is_stable_connection_ms(connected_for_ms) {
            self.reset();
        }
    }

    /// Records one retryable failure and computes the delay before the next attempt.
    pub fn record_failure(
        &mut self,
        reason: FailureReason,
        established_session: bool,
        random: f64,
    ) -> BackoffDecision {
        let first_loss = established_session && self.failure_count == 0;
        self.failure_count = self.failure_count.saturating_add(1);
        if reason == FailureReason::ApprovalPending {
            self.approval_waits = self.approval_waits.saturating_add(1);
        }
        let awaiting_approval = reason == FailureReason::ApprovalPending
            && self.approval_waits <= APPROVAL_PENDING_FAST_RETRIES;
        if !awaiting_approval {
            self.backoff_failures = self.backoff_failures.saturating_add(1);
        }
        let delay_ms = if awaiting_approval {
            APPROVAL_PENDING_RETRY_DELAY_MS
        } else if first_loss {
            session_loss_retry_delay_ms(random)
        } else {
            let delay = jittered_retry_delay_ms(self.backoff_failures.saturating_sub(1), random);
            if reason == FailureReason::ApprovalPending {
                delay.max(APPROVAL_PENDING_RETRY_DELAY_MS)
            } else {
                delay
            }
        };
        BackoffDecision {
            delay_ms,
            attempt: self.failure_count,
            awaiting_approval,
            first_loss,
        }
    }
}

/// Tracks per-route better-route switch cooldowns (`BETTER_ROUTE_COOLDOWN_MS = 300_000` ms).
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
