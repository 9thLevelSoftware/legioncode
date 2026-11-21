//! Per-host source-control concurrency caps, `Retry-After` parsing, and quota reserve enforcement.
//!
//! Implements [`SourceControlRateLimit`] to track active per-host request
//! permits, honor `429` / `Retry-After` backoff windows, and enforce a 10%
//! remaining-quota reserve (`remaining * 10 >= limit`) for non-priority calls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use http::{HeaderMap, StatusCode};

use crate::core::CoreError;
pub use crate::core::http_quota::{
    BlockReason, QuotaSnapshot, RateLimitDecision, RateLimitUpdate, parse_retry_after,
};
use crate::core::http_quota::{header_str, parse_quota_headers};

/// Default percentage of API quota preserved for critical requests.
pub const DEFAULT_QUOTA_RESERVE_PERCENT: u8 = 10;

/// Default maximum in-flight requests allowed per host when not explicitly tuned.
pub const DEFAULT_HOST_CONCURRENCY: usize = 6;

#[derive(Debug, Default)]
struct HostBucket {
    max_concurrency: Option<usize>,
    reserve_percent: Option<u8>,
    active: usize,
    blocked_until: Option<(u64, BlockReason)>,
    last_quota: Option<QuotaSnapshot>,
}

#[derive(Debug)]
struct InnerState {
    default_concurrency: usize,
    default_reserve_percent: u8,
    hosts: HashMap<String, HostBucket>,
}

/// RAII concurrency guard returned by [`SourceControlRateLimit::acquire`].
///
/// Decrements the host's active in-flight count automatically when dropped.
#[derive(Debug)]
pub struct RateLimitPermit {
    host: String,
    state: Arc<Mutex<InnerState>>,
}

impl Drop for RateLimitPermit {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.state.lock()
            && let Some(bucket) = guard.hosts.get_mut(&self.host)
        {
            bucket.active = bucket.active.saturating_sub(1);
        }
    }
}

/// Thread-safe per-host concurrency, `Retry-After`, and quota reserve coordinator.
#[derive(Debug, Clone)]
pub struct SourceControlRateLimit {
    state: Arc<Mutex<InnerState>>,
}

impl Default for SourceControlRateLimit {
    fn default() -> Self {
        Self::new(DEFAULT_HOST_CONCURRENCY, DEFAULT_QUOTA_RESERVE_PERCENT)
    }
}

impl SourceControlRateLimit {
    /// Creates a rate limiter with the given default per-host concurrency and quota reserve percentage.
    #[must_use]
    pub fn new(default_concurrency: usize, default_reserve_percent: u8) -> Self {
        Self {
            state: Arc::new(Mutex::new(InnerState {
                default_concurrency: default_concurrency.max(1),
                default_reserve_percent: default_reserve_percent.min(100),
                hosts: HashMap::new(),
            })),
        }
    }

    /// Configures a host-specific concurrency cap (clamped to `>= 1`) and reserve percentage.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::RateLimited`] if the internal state lock is poisoned.
    pub fn configure_host(
        &self,
        host: &str,
        max_concurrency: usize,
        reserve_percent: u8,
    ) -> Result<(), CoreError> {
        let key = normalize_host_key(host);
        let mut guard = self.lock_state(&key)?;
        let bucket = guard.hosts.entry(key).or_default();
        bucket.max_concurrency = Some(max_concurrency.max(1));
        bucket.reserve_percent = Some(reserve_percent.min(100));
        Ok(())
    }

    /// Evaluates whether `host` may start a request at `now_epoch_secs` without reserving a permit.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::RateLimited`] if the internal lock is poisoned.
    pub fn check_admission(
        &self,
        host: &str,
        allow_reserve: bool,
        now_epoch_secs: u64,
    ) -> Result<RateLimitDecision, CoreError> {
        let key = normalize_host_key(host);
        let guard = self.lock_state(&key)?;
        Ok(evaluate_bucket(
            guard.hosts.get(&key),
            guard.default_concurrency,
            guard.default_reserve_percent,
            allow_reserve,
            now_epoch_secs,
        ))
    }

    /// Attempts to acquire an RAII [`RateLimitPermit`] for `host`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::RateLimited`] if the host is blocked by a `Retry-After`
    /// window, has entered the protected 10% quota reserve (with `allow_reserve == false`),
    /// or reached its per-host concurrency ceiling.
    ///
    /// ```
    /// use t3_scm::core::rate_limit::SourceControlRateLimit;
    ///
    /// let limiter = SourceControlRateLimit::default();
    /// limiter.configure_host("api.github.com", 2, 10)?;
    /// let p1 = limiter.acquire("api.github.com", false, 1_700_000_000)?;
    /// let p2 = limiter.acquire("api.github.com", false, 1_700_000_000)?;
    /// assert!(limiter.acquire("api.github.com", false, 1_700_000_000).is_err());
    /// drop(p1);
    /// let _p3 = limiter.acquire("api.github.com", false, 1_700_000_000)?;
    /// drop(p2);
    /// # Ok::<(), t3_scm::core::CoreError>(())
    /// ```
    pub fn acquire(
        &self,
        host: &str,
        allow_reserve: bool,
        now_epoch_secs: u64,
    ) -> Result<RateLimitPermit, CoreError> {
        let key = normalize_host_key(host);
        let mut guard = self.lock_state(&key)?;
        let default_conc = guard.default_concurrency;
        let default_res = guard.default_reserve_percent;

        let decision = evaluate_bucket(
            guard.hosts.get(&key),
            default_conc,
            default_res,
            allow_reserve,
            now_epoch_secs,
        );
        match decision {
            RateLimitDecision::Admitted => {
                let bucket = guard.hosts.entry(key.clone()).or_default();
                bucket.active += 1;
                Ok(RateLimitPermit {
                    host: key,
                    state: Arc::clone(&self.state),
                })
            }
            RateLimitDecision::ConcurrencyFull {
                active,
                max_concurrency,
            } => Err(CoreError::RateLimited {
                host: key,
                reason: format!("concurrency limit reached ({active}/{max_concurrency})"),
            }),
            RateLimitDecision::BlockedUntil {
                until_epoch_secs,
                reason,
            } => Err(CoreError::RateLimited {
                host: key,
                reason: format!("blocked until {until_epoch_secs} ({reason:?})"),
            }),
            RateLimitDecision::RejectedQuotaReserve {
                remaining,
                limit,
                reserve_threshold,
            } => Err(CoreError::RateLimited {
                host: key,
                reason: format!(
                    "remaining quota {remaining}/{limit} is below reserve threshold {reserve_threshold}"
                ),
            }),
        }
    }

    /// Updates quota snapshots and backoff windows from an HTTP response status and headers.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::RateLimited`] only if the mutex is poisoned.
    pub fn record_response(
        &self,
        host: &str,
        status: StatusCode,
        headers: &HeaderMap,
        now_epoch_secs: u64,
    ) -> Result<RateLimitUpdate, CoreError> {
        let key = normalize_host_key(host);
        let quota = parse_quota_headers(headers);
        let retry_after_secs =
            header_str(headers, "retry-after").and_then(|v| parse_retry_after(v, now_epoch_secs));

        let mut guard = self.lock_state(&key)?;
        let bucket = guard.hosts.entry(key).or_default();
        if let Some(q) = quota {
            bucket.last_quota = Some(q);
        }

        let mut blocked_until_epoch_secs = None;
        if let Some(delay) = retry_after_secs {
            let until = now_epoch_secs.saturating_add(delay.max(1));
            bucket.blocked_until = Some((until, BlockReason::RetryAfter));
            blocked_until_epoch_secs = Some(until);
        } else if status == StatusCode::TOO_MANY_REQUESTS {
            let until = now_epoch_secs.saturating_add(60);
            bucket.blocked_until = Some((until, BlockReason::RetryAfter));
            blocked_until_epoch_secs = Some(until);
        } else if let Some(q) = quota
            && q.remaining == 0
            && (status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS)
        {
            let until = q
                .reset_epoch_secs
                .filter(|&r| r > now_epoch_secs)
                .unwrap_or_else(|| now_epoch_secs.saturating_add(60));
            bucket.blocked_until = Some((until, BlockReason::QuotaExhausted));
            blocked_until_epoch_secs = Some(until);
        } else if status.is_success() {
            bucket.blocked_until = None;
        }

        Ok(RateLimitUpdate {
            retry_after_secs,
            quota,
            blocked_until_epoch_secs,
        })
    }

    /// Feeds upstream response status and `x-ratelimit-*` / `ratelimit-*` / `retry-after`
    /// headers into the per-host tracker.
    ///
    /// Alias for [`Self::record_response`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::RateLimited`] only if the internal mutex is poisoned.
    pub fn update_from_headers(
        &self,
        host: &str,
        status: StatusCode,
        headers: &HeaderMap,
        now_epoch_secs: u64,
    ) -> Result<RateLimitUpdate, CoreError> {
        self.record_response(host, status, headers, now_epoch_secs)
    }

    /// Returns the number of currently active in-flight permits for `host`.
    #[must_use]
    pub fn active_count(&self, host: &str) -> usize {
        let key = normalize_host_key(host);
        self.state
            .lock()
            .ok()
            .and_then(|g| g.hosts.get(&key).map(|b| b.active))
            .unwrap_or(0)
    }

    fn lock_state(&self, host: &str) -> Result<std::sync::MutexGuard<'_, InnerState>, CoreError> {
        self.state.lock().map_err(|_| CoreError::RateLimited {
            host: host.to_owned(),
            reason: "rate limiter state lock poisoned".to_owned(),
        })
    }
}

fn evaluate_bucket(
    bucket: Option<&HostBucket>,
    default_concurrency: usize,
    default_reserve_percent: u8,
    allow_reserve: bool,
    now_epoch_secs: u64,
) -> RateLimitDecision {
    let Some(b) = bucket else {
        return RateLimitDecision::Admitted;
    };
    if let Some((until, reason)) = b.blocked_until
        && now_epoch_secs < until
    {
        return RateLimitDecision::BlockedUntil {
            until_epoch_secs: until,
            reason,
        };
    }

    let reserve_percent = b.reserve_percent.unwrap_or(default_reserve_percent);
    if let Some(q) = b.last_quota
        && !q.has_headroom(reserve_percent, allow_reserve)
    {
        return RateLimitDecision::RejectedQuotaReserve {
            remaining: q.remaining,
            limit: q.limit,
            reserve_threshold: q.reserve_threshold(reserve_percent),
        };
    }

    let max_concurrency = b.max_concurrency.unwrap_or(default_concurrency);
    if b.active >= max_concurrency {
        return RateLimitDecision::ConcurrencyFull {
            active: b.active,
            max_concurrency,
        };
    }

    RateLimitDecision::Admitted
}

fn normalize_host_key(host: &str) -> String {
    host.trim().to_ascii_lowercase()
}
