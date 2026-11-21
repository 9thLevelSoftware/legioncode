//! Quota snapshot evaluation, rate-limit decision models, and `Retry-After` HTTP header parsing.

use http::HeaderMap;

/// Reason a host is temporarily refusing new requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Upstream returned `429 Too Many Requests` or primary rate-limit backoff.
    RetryAfter,
    /// Upstream exhausted primary `X-RateLimit-Remaining`.
    QuotaExhausted,
}

/// Latest quota telemetry parsed from response headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaSnapshot {
    /// Total hourly/window quota limit (`X-RateLimit-Limit`).
    pub limit: u64,
    /// Remaining calls reported by upstream (`X-RateLimit-Remaining`).
    pub remaining: u64,
    /// Optional `Unix` epoch seconds when the quota window resets.
    pub reset_epoch_secs: Option<u64>,
}

impl QuotaSnapshot {
    /// Returns `true` when enough quota remains for a request.
    ///
    /// When `allow_reserve` is `false`, enforces `remaining >= limit * reserve_percent / 100`
    /// using exact integer arithmetic (`remaining * 100 >= limit * reserve_percent`).
    /// When `allow_reserve` is `true`, admits requests while `remaining > 0`.
    ///
    /// ```
    /// use t3_scm::core::rate_limit::QuotaSnapshot;
    ///
    /// let snap = QuotaSnapshot { limit: 5000, remaining: 499, reset_epoch_secs: None };
    /// assert!(!snap.has_headroom(10, false));
    /// assert!(snap.has_headroom(10, true));
    ///
    /// let edge = QuotaSnapshot { limit: 5000, remaining: 500, reset_epoch_secs: None };
    /// assert!(edge.has_headroom(10, false));
    /// ```
    #[must_use]
    pub const fn has_headroom(&self, reserve_percent: u8, allow_reserve: bool) -> bool {
        if self.remaining == 0 {
            return false;
        }
        if allow_reserve || self.limit == 0 || reserve_percent == 0 {
            return true;
        }
        self.remaining.saturating_mul(100) >= self.limit.saturating_mul(reserve_percent as u64)
    }

    /// Computes the minimum `remaining` threshold needed when the reserve is enforced.
    #[must_use]
    pub const fn reserve_threshold(&self, reserve_percent: u8) -> u64 {
        let num = self.limit.saturating_mul(reserve_percent as u64);
        num.div_ceil(100)
    }
}

/// Decision returned when probing or acquiring a host request slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitDecision {
    /// Request can proceed immediately.
    Admitted,
    /// Host already reached its configured active concurrency ceiling.
    ConcurrencyFull {
        /// Current number of active in-flight permits.
        active: usize,
        /// Maximum permitted concurrent calls for this host.
        max_concurrency: usize,
    },
    /// Host is blocked until a `Retry-After` or reset timestamp passes.
    BlockedUntil {
        /// Epoch seconds when requests may resume.
        until_epoch_secs: u64,
        /// Triggering rate-limit condition.
        reason: BlockReason,
    },
    /// Remaining quota entered the protected 10% reserve and `allow_reserve` was false.
    RejectedQuotaReserve {
        /// Remaining calls in current quota window.
        remaining: u64,
        /// Total quota limit in current window.
        limit: u64,
        /// Minimum remaining calls required to exit reserve mode.
        reserve_threshold: u64,
    },
}

/// Summary returned after updating state from an HTTP response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitUpdate {
    /// Parsed `Retry-After` delay in seconds, if present.
    pub retry_after_secs: Option<u64>,
    /// Parsed quota snapshot when `X-RateLimit-Limit` and `X-RateLimit-Remaining` exist.
    pub quota: Option<QuotaSnapshot>,
    /// Epoch seconds until which requests are paused, if any.
    pub blocked_until_epoch_secs: Option<u64>,
}

/// Parses an HTTP `Retry-After` header (delta seconds or `RFC 7231` HTTP-date) into delay seconds.
#[must_use]
pub fn parse_retry_after(value: &str, now_epoch_secs: u64) -> Option<u64> {
    let trimmed = value.trim();
    if let Ok(seconds) = trimmed.parse::<u64>() {
        return Some(seconds);
    }
    let target_epoch = parse_http_date_epoch_secs(trimmed)?;
    Some(target_epoch.saturating_sub(now_epoch_secs))
}

pub(crate) fn parse_quota_headers(headers: &HeaderMap) -> Option<QuotaSnapshot> {
    let limit = header_u64(headers, "x-ratelimit-limit")
        .or_else(|| header_u64(headers, "ratelimit-limit"))?;
    let remaining = header_u64(headers, "x-ratelimit-remaining")
        .or_else(|| header_u64(headers, "ratelimit-remaining"))?;
    let reset_epoch_secs = header_u64(headers, "x-ratelimit-reset")
        .or_else(|| header_u64(headers, "ratelimit-reset"))
        .or_else(|| header_u64(headers, "x-ratelimit-reset-after"));
    Some(QuotaSnapshot {
        limit,
        remaining,
        reset_epoch_secs,
    })
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    header_str(headers, name)?.trim().parse::<u64>().ok()
}

pub(crate) fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// Parses an IMF-fixdate string (`Wed, 21 Oct 2015 07:28:00 GMT`) into Unix epoch seconds.
fn parse_http_date_epoch_secs(raw: &str) -> Option<u64> {
    let (_, rest) = raw.split_once(',')?;
    let mut parts = rest.split_whitespace();
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<i64>().ok()?;
    let time_part = parts.next()?;
    if parts.next()? != "GMT" {
        return None;
    }
    let mut t = time_part.split(':');
    let hour = t.next()?.parse::<u64>().ok()?;
    let min = t.next()?.parse::<u64>().ok()?;
    let sec = t.next()?.parse::<u64>().ok()?;
    if hour > 23 || min > 59 || sec > 59 || day == 0 || day > 31 {
        return None;
    }
    let days = days_from_civil(year, month, i64::from(day));
    if days < 0 {
        return None;
    }
    let epoch_days = u64::try_from(days).ok()?;
    Some(epoch_days * 86_400 + hour * 3600 + min * 60 + sec)
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
