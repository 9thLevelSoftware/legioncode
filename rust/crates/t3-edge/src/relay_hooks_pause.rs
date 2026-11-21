//! Isolate-local and shared Cloudflare API rate-limit pause gate (`BUG-32`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Shared DB pause re-read interval per isolate (`ManagedEndpointProvider.ts:488`).
pub const CLOUDFLARE_PAUSE_REFRESH_MS: u64 = 5_000;

/// Default pause duration after a `429` without an explicit `Retry-After` header.
pub const CLOUDFLARE_DEFAULT_RATE_LIMIT_PAUSE_MS: u64 = 30_000;

/// Maximum pause duration applied after a Cloudflare `429` response (`2 minutes`).
pub const CLOUDFLARE_MAX_RATE_LIMIT_PAUSE_MS: u64 = 120_000;

/// Error returned when an upstream Cloudflare API call is rejected during an active pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("Cloudflare API calls are paused for {retry_after_ms}ms after a rate limit")]
pub struct CloudflareApiPaused {
    /// Remaining milliseconds before calls are permitted again.
    pub retry_after_ms: u64,
}

/// Outcome of permitting a Cloudflare API call through [`CloudflareApiPauseGate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PauseGatePass<E> {
    /// `true` when the shared pause store was successfully queried during this check.
    pub refreshed_from_shared: bool,
    /// Present when a shared pause read failed and the gate failed open without advancing
    /// `checked_at_ms`.
    pub shared_read_error: Option<E>,
}

/// Per-isolate cache for the shared Cloudflare account-wide 429 pause (`BUG-32`).
///
/// Mirrors `makeCloudflareApiGate` in `infra/relay/src/environments/ManagedEndpointProvider.ts`
/// while fixing `BUG-32`: `checked_at_ms` advances **only** when reading the shared pause store
/// succeeds (`Ok(pause_until_ms)`), never on transient read errors (`Err`), so subsequent calls
/// immediately retry reading shared pause state instead of suppressing reads for `5_000 ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareApiPauseGate {
    paused_until: u64,
    last_checked: Option<u64>,
    refresh_window_ms: u64,
}

impl Default for CloudflareApiPauseGate {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudflareApiPauseGate {
    /// Creates a gate using the default `5_000 ms` shared-pause refresh interval.
    pub fn new() -> Self {
        Self::with_refresh_window_ms(CLOUDFLARE_PAUSE_REFRESH_MS)
    }

    /// Creates a gate with a custom shared-pause refresh interval in milliseconds.
    pub fn with_refresh_window_ms(refresh_window_ms: u64) -> Self {
        Self {
            paused_until: 0,
            last_checked: None,
            refresh_window_ms,
        }
    }

    /// Epoch milliseconds until which calls are locally paused (`0` when unpaused).
    pub fn paused_until_ms(&self) -> u64 {
        self.paused_until
    }

    /// Epoch milliseconds of the last **successful** shared pause read, if any.
    pub fn checked_at_ms(&self) -> Option<u64> {
        self.last_checked
    }

    /// Configured cache window for shared pause re-reads in milliseconds.
    pub fn refresh_window_ms(&self) -> u64 {
        self.refresh_window_ms
    }

    /// Returns `Some(remaining_ms)` when `now_ms` falls inside an active local pause.
    pub fn is_paused(&self, now_ms: u64) -> Option<u64> {
        (now_ms < self.paused_until).then(|| self.paused_until - now_ms)
    }

    /// Returns `true` when the gate should query the shared pause store at `now_ms`.
    pub fn should_refresh(&self, now_ms: u64) -> bool {
        if now_ms < self.paused_until {
            return false;
        }
        match self.last_checked {
            None => true,
            Some(checked_at) => now_ms.saturating_sub(checked_at) >= self.refresh_window_ms,
        }
    }

    /// Applies the result of reading the shared pause store at `now_ms`.
    ///
    /// Advances `checked_at_ms` **only** when `read_result` is `Ok(shared_until_ms)`. On `Err`,
    /// `checked_at_ms` remains unchanged so the next call does not suppress shared reads for the
    /// `5_000 ms` window (`BUG-32`).
    pub fn apply_shared_pause_read<E>(
        &mut self,
        now_ms: u64,
        read_result: Result<u64, E>,
    ) -> Result<u64, E> {
        let shared_until_ms = read_result?;
        self.last_checked = Some(now_ms);
        self.paused_until = self.paused_until.max(shared_until_ms);
        Ok(self.paused_until)
    }

    /// Evaluates whether an upstream call at `now_ms` may proceed, refreshing from the shared
    /// store when due and failing open on shared read error without advancing `checked_at_ms`.
    pub fn evaluate<E, F>(
        &mut self,
        now_ms: u64,
        read_shared_pause: F,
    ) -> Result<PauseGatePass<E>, CloudflareApiPaused>
    where
        F: FnOnce() -> Result<u64, E>,
    {
        let mut refreshed_from_shared = false;
        let mut shared_read_error = None;
        if self.should_refresh(now_ms) {
            match self.apply_shared_pause_read(now_ms, read_shared_pause()) {
                Ok(_) => refreshed_from_shared = true,
                Err(err) => shared_read_error = Some(err),
            }
        }
        if let Some(retry_after_ms) = self.is_paused(now_ms) {
            return Err(CloudflareApiPaused { retry_after_ms });
        }
        Ok(PauseGatePass {
            refreshed_from_shared,
            shared_read_error,
        })
    }

    /// Records a local `429` / rate-limit pause clamped to `CLOUDFLARE_MAX_RATE_LIMIT_PAUSE_MS`,
    /// returning the resulting `paused_until_ms` timestamp to persist in the shared store.
    pub fn record_rate_limit(&mut self, now_ms: u64, retry_after_ms: Option<u64>) -> u64 {
        let pause_ms = retry_after_ms
            .unwrap_or(CLOUDFLARE_DEFAULT_RATE_LIMIT_PAUSE_MS)
            .min(CLOUDFLARE_MAX_RATE_LIMIT_PAUSE_MS);
        let until = now_ms.saturating_add(pause_ms);
        self.paused_until = self.paused_until.max(until);
        self.paused_until
    }
}
