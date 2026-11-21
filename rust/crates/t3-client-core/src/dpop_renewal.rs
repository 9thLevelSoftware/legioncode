//! `DPoP` access-token socket renewal scheduling before expiry.
//!
//! Mirrors `renewDpopBeforeExpiry` in `packages/client-runtime/src/authorization/service.ts`:
//! - Schedules renewal `60_000` ms (`DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS`) prior to expiry.
//! - Enforces a `5_000` ms retry floor while still before token expiry (`now < expires_at`).
//! - Terminates (`None`) once a relay-backed token expires (`now >= expires_at`), because the
//!   relay/host closes expired `T3 Connect` WebSocket connections.
//! - Continues retrying on a `60_000` ms floor past expiry for direct connections, because direct
//!   hosts keep an open socket alive across its session renewal window.
//!
//! # Examples
//!
//! ```
//! use t3_client_core::dpop_renewal::{
//!     DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS, DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS, next_renewal_delay_ms,
//! };
//!
//! // One hour token checked at t = 0 waits 59 minutes (expires_at - 60_000).
//! assert_eq!(next_renewal_delay_ms(3_600_000, 0, false), Some(3_540_000));
//!
//! // Inside the 60s skew window but pre-expiry, retry floor is 5_000 ms.
//! assert_eq!(
//!     next_renewal_delay_ms(3_600_000, 3_540_000, false),
//!     Some(DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS)
//! );
//!
//! // Past expiry: relay stops (None), direct keeps retrying every 60_000 ms.
//! assert_eq!(next_renewal_delay_ms(1_000, 5_000, false), None);
//! assert_eq!(
//!     next_renewal_delay_ms(1_000, 5_000, true),
//!     Some(DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS)
//! );
//! ```

use serde::{Deserialize, Serialize};

/// Renewal skew window in milliseconds before token expiry (`60_000` ms).
pub const DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS: u64 = 60_000;

/// Minimum renewal sleep/retry delay in milliseconds while `now < expires_at` (`5_000` ms).
pub const DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS: u64 = 5_000;

/// Minimum renewal retry delay in milliseconds once `now >= expires_at` on direct links (`60_000` ms).
pub const DPOP_RENEWAL_POST_EXPIRY_DIRECT_FLOOR_MS: u64 = DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS;

/// Computes the numeric renewal delay in milliseconds using the pre/post-expiry floor rules.
///
/// Prefer [`next_renewal_delay_ms`] when enforcing relay termination past `expires_at_epoch_ms`.
pub fn renewal_delay_ms(expires_at_epoch_ms: u64, now_epoch_ms: u64) -> u64 {
    let floor = if now_epoch_ms < expires_at_epoch_ms {
        DPOP_RENEWAL_PRE_EXPIRY_FLOOR_MS
    } else {
        DPOP_RENEWAL_POST_EXPIRY_DIRECT_FLOOR_MS
    };
    let target = expires_at_epoch_ms.saturating_sub(DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS);
    target.saturating_sub(now_epoch_ms).max(floor)
}

/// Computes the next `DPoP` socket token renewal delay in milliseconds, or `None` to stop.
///
/// Returns `None` when `!is_direct && now_epoch_ms >= expires_at_epoch_ms`, matching the loop
/// guard `while (direct !== undefined || now < expiresAtEpochMs)` in `authorization/service.ts`.
pub fn next_renewal_delay_ms(
    expires_at_epoch_ms: u64,
    now_epoch_ms: u64,
    is_direct: bool,
) -> Option<u64> {
    if !is_direct && now_epoch_ms >= expires_at_epoch_ms {
        return None;
    }
    Some(renewal_delay_ms(expires_at_epoch_ms, now_epoch_ms))
}

/// Stateful helper tracking `DPoP` socket token renewal across transient and blocked failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DpopRenewalTracker {
    expires_at_epoch_ms: u64,
    is_direct: bool,
    stopped: bool,
}

impl DpopRenewalTracker {
    /// Starts tracking a socket token expiring at `expires_at_epoch_ms`.
    pub const fn new(expires_at_epoch_ms: u64, is_direct: bool) -> Self {
        Self {
            expires_at_epoch_ms,
            is_direct,
            stopped: false,
        }
    }

    /// Current tracked token expiry timestamp in epoch milliseconds.
    pub const fn expires_at_epoch_ms(&self) -> u64 {
        self.expires_at_epoch_ms
    }

    /// Whether the connection uses a direct host credential (`true`) or T3 Connect relay (`false`).
    pub const fn is_direct(&self) -> bool {
        self.is_direct
    }

    /// Returns `true` if a blocked credential error or relay expiry ended renewal.
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Computes the next sleep duration from `now_epoch_ms`, returning `None` once stopped/expired.
    pub fn next_delay_ms(&mut self, now_epoch_ms: u64) -> Option<u64> {
        if self.stopped {
            return None;
        }
        let delay = next_renewal_delay_ms(self.expires_at_epoch_ms, now_epoch_ms, self.is_direct);
        if delay.is_none() {
            self.stopped = true;
        }
        delay
    }

    /// Advances the tracked expiry monotonically (`max(current, renewed_expires_at_epoch_ms)`).
    pub fn record_renewed(&mut self, renewed_expires_at_epoch_ms: u64) {
        self.expires_at_epoch_ms = self.expires_at_epoch_ms.max(renewed_expires_at_epoch_ms);
    }

    /// Keeps the current expiry on a transient error so the loop retries on schedule.
    pub const fn record_transient_failure(&mut self) {}

    /// Permanently stops renewal on a blocked credential error (`ConnectionBlockedError`).
    pub const fn record_blocked_failure(&mut self) {
        self.stopped = true;
    }
}
