//! Byte-slice transcript usage scanning, hub-routed proxied usage-limit resolution,
//! and shared RFC-3339 timestamp parsing.

use serde::{Deserialize, Serialize};

use super::usage_limits::UsageWindow;

/// Usage-limit snapshot for one account served by an upstream hub (`c436409da0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSourceAccount {
    /// Canonical provider driver slug (`"codex"`, `"claudeAgent"`, `"antigravity"`, ...).
    pub driver: String,
    /// Optional email identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Optional workspace identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// `true` when latest hub poll failed for this account.
    pub unavailable: bool,
    /// Explicit model IDs routed to this account (`None` = readable model list unavailable).
    #[serde(default)]
    pub models: Option<Vec<String>>,
    /// Reported quota windows.
    #[serde(default)]
    pub windows: Vec<UsageWindow>,
}

/// Grouped accounts served through one hub origin (`c436409da0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageLimitSourceSnapshot {
    /// Hub endpoint URL / origin matching `provider.auth.endpoint`.
    pub origin: String,
    /// Accounts hosted by this hub.
    pub accounts: Vec<UsageSourceAccount>,
}

/// Checks whether a raw byte transcript line contains any ASCII marker byte slice without
/// allocating a temporary String (`abd47de64f`).
#[must_use]
pub fn transcript_line_contains_any_marker(line: &[u8], markers: &[&[u8]]) -> bool {
    markers.iter().any(|needle| {
        !needle.is_empty()
            && (line.len() >= needle.len() && line.windows(needle.len()).any(|win| win == *needle))
    })
}

/// Returns the hub snapshots whose `origin` equals `provider_endpoint` (`c436409da0`).
#[must_use]
pub fn instance_hubs<'a>(
    provider_endpoint: Option<&str>,
    sources: &'a [UsageLimitSourceSnapshot],
) -> Vec<&'a UsageLimitSourceSnapshot> {
    let Some(endpoint) = provider_endpoint else {
        return Vec::new();
    };
    sources
        .iter()
        .filter(|source| source.origin == endpoint)
        .collect()
}

fn antigravity_quota_family(id: &str) -> &'static str {
    if !id.starts_with("gemini") {
        "shared"
    } else if id.contains("argon") {
        "argon"
    } else if id.contains("carbon") {
        "carbon"
    } else {
        "gemini"
    }
}

/// Returns `true` when Antigravity quota window `window_id` meters `model` (`c436409da0`).
#[must_use]
pub fn antigravity_window_limits_model(window_id: &str, model: Option<&str>) -> bool {
    let base = model.map(|m| match m.rsplit_once('/') {
        Some((_, suffix)) => suffix,
        None => m,
    });
    let Some(base_slug) = base else {
        return true;
    };
    if !(base_slug.starts_with("gemini")
        || base_slug.starts_with("claude")
        || base_slug.starts_with("gpt"))
    {
        return true;
    }
    antigravity_quota_family(window_id) == antigravity_quota_family(base_slug)
}

/// Filters hub accounts down to those that route `model` for `driver` (`c436409da0`).
#[must_use]
pub fn hub_routed_accounts<'a>(
    sources: &[&'a UsageLimitSourceSnapshot],
    driver: &str,
    model: Option<&str>,
) -> Vec<&'a UsageSourceAccount> {
    let full = model.map(strip_1m_suffix);
    let base = full.map(strip_trailing_parens);
    let lists_slug = |target: Option<&str>| -> bool {
        let Some(id) = target else {
            return false;
        };
        sources.iter().any(|src| {
            src.accounts.iter().any(|acct| {
                acct.models
                    .as_ref()
                    .is_some_and(|list| list.iter().any(|m| m == id))
            })
        })
    };
    let resolved_slug = if lists_slug(base) { base } else { full };
    let account_lists_slug = |acct: &UsageSourceAccount| -> bool {
        let Some(target) = resolved_slug else {
            return false;
        };
        acct.models
            .as_ref()
            .is_some_and(|list| list.iter().any(|m| m == target))
    };

    let serving_drivers: Vec<&str> = sources
        .iter()
        .flat_map(|src| src.accounts.iter())
        .filter(|acct| account_lists_slug(acct))
        .map(|acct| acct.driver.as_str())
        .collect();

    if serving_drivers.is_empty() {
        return sources
            .iter()
            .flat_map(|src| src.accounts.iter())
            .filter(|acct| {
                acct.driver == driver && acct.models.as_ref().is_none_or(|list| !list.is_empty())
            })
            .collect();
    }

    sources
        .iter()
        .flat_map(|src| src.accounts.iter())
        .filter(|acct| {
            if acct.models.is_some() {
                account_lists_slug(acct)
            } else {
                serving_drivers.contains(&acct.driver.as_str()) || acct.driver == driver
            }
        })
        .collect()
}

/// Returns the earliest ISO-8601 reset timestamp when **every** routed account for `(driver, model)`
/// has an exhausted (`>= 100%`) window covering `model` with a reset time strictly after `now_ms` (`c436409da0`).
#[must_use]
pub fn hub_pool_reset_at(
    sources: &[&UsageLimitSourceSnapshot],
    driver: &str,
    model: Option<&str>,
    now_ms: i64,
) -> Option<String> {
    let routed = hub_routed_accounts(sources, driver, model);
    if routed.is_empty() {
        return None;
    }
    let mut earliest: Option<(i64, String)> = None;
    for acct in routed {
        if acct.unavailable {
            return None;
        }
        let spent: Vec<&UsageWindow> = acct
            .windows
            .iter()
            .filter(|win| {
                win.used_percent >= 100.0
                    && (acct.driver != "antigravity"
                        || antigravity_window_limits_model(&win.id, model))
            })
            .collect();
        if spent.is_empty() {
            return None;
        }
        let mut account_max: Option<(i64, String)> = None;
        for win in spent {
            let iso = win.resets_at.as_deref()?;
            let ms = parse_rfc3339_millis(iso)?;
            if account_max.as_ref().is_none_or(|(best, _)| ms > *best) {
                account_max = Some((ms, iso.to_owned()));
            }
        }
        let (recovers_at, iso) = account_max?;
        if recovers_at <= now_ms {
            return None;
        }
        if earliest
            .as_ref()
            .is_none_or(|(best, _)| recovers_at < *best)
        {
            earliest = Some((recovers_at, iso));
        }
    }
    earliest.map(|(_, iso)| iso)
}

fn strip_1m_suffix(raw: &str) -> &str {
    raw.strip_suffix("[1m]")
        .or_else(|| raw.strip_suffix("[1M]"))
        .unwrap_or(raw)
}

fn strip_trailing_parens(raw: &str) -> &str {
    if let Some(open) = raw.rfind('(')
        && raw.ends_with(')')
    {
        return &raw[..open];
    }
    raw
}

/// Parses an RFC-3339 UTC or offset timestamp into Unix epoch milliseconds.
#[must_use]
pub fn parse_rfc3339_millis(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let (date_str, time_str) = trimmed.split_once(['T', 't'])?;
    let mut date_parts = date_str.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (hms_frac, offset_minutes) = if let Some(s) = time_str.strip_suffix(['Z', 'z']) {
        (s, 0_i64)
    } else {
        let pos = time_str.rfind(['+', '-'])?;
        let (lhs, rhs) = time_str.split_at(pos);
        let sign = if rhs.starts_with('-') { -1_i64 } else { 1_i64 };
        let (oh, om) = rhs[1..].split_once(':')?;
        let off_h: i64 = oh.parse().ok()?;
        let off_m: i64 = om.parse().ok()?;
        (lhs, sign * (off_h * 60 + off_m))
    };

    let (hms, frac_ms) = match hms_frac.split_once('.') {
        Some((l, r)) => {
            let mut digits = r.bytes().take(3).collect::<Vec<_>>();
            while digits.len() < 3 {
                digits.push(b'0');
            }
            let ms_str = std::str::from_utf8(&digits).ok()?;
            (l, ms_str.parse::<i64>().ok()?)
        }
        None => (hms_frac, 0),
    };
    let mut t_parts = hms.split(':');
    let hour: i64 = t_parts.next()?.parse().ok()?;
    let min: i64 = t_parts.next()?.parse().ok()?;
    let sec: i64 = t_parts.next()?.parse().ok()?;
    if t_parts.next().is_some() || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let total_secs = days * 86_400 + hour * 3_600 + min * 60 + sec - offset_minutes * 60;
    Some(total_secs * 1_000 + frac_ms)
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m_adj = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * m_adj + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
