//! UTC ISO-8601 timestamp helpers, duration parsing, and schedule calculations.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Minimum allowed interval schedule duration (1 minute).
pub const MIN_SCHEDULE_INTERVAL_MS: i64 = 60_000;
/// Grace window after which a missed fixed-time daily trigger is skipped (10 minutes).
pub const MISSED_FIXED_TIME_GRACE_MS: i64 = 10 * 60_000;

/// Schedule configuration for a workflow trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleSpec {
    /// Fires repeatedly every `every_ms` milliseconds.
    Interval {
        #[serde(rename = "everyMs")]
        every_ms: i64,
    },
    /// Fires at a 24-hour `HH:MM` time on all days or specific weekdays (0 = Sunday).
    FixedTime {
        #[serde(rename = "timeOfDay")]
        time_of_day: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekdays: Option<Vec<u8>>,
    },
}

/// Returns current Unix epoch milliseconds.
pub fn now_epoch_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

/// Formats the current UTC wall-clock time as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn now_iso_utc() -> String {
    format_millis_iso(now_epoch_ms())
}

/// Formats Unix epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn format_millis_iso(epoch_ms: i64) -> String {
    let total_secs = epoch_ms.div_euclid(1_000);
    let millis = epoch_ms.rem_euclid(1_000);
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Parses an ISO-8601 UTC timestamp (`...Z` or `...+00:00`) into Unix epoch milliseconds.
pub fn parse_iso_millis(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    let s = trimmed
        .strip_suffix('Z')
        .or_else(|| trimmed.strip_suffix("+00:00"))?;
    let (date_part, time_part) = s.split_once('T')?;
    let mut date_parts = date_part.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let (hms_part, frac_part) = match time_part.split_once('.') {
        Some((hms, frac)) => (hms, Some(frac)),
        None => (time_part, None),
    };
    let mut time_parts = hms_part.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let millis = match frac_part {
        None => 0_i64,
        Some(frac) if !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()) => {
            let bytes = frac.as_bytes();
            let digit = |idx: usize| bytes.get(idx).map_or(0_i64, |b| i64::from(b - b'0'));
            digit(0) * 100 + digit(1) * 10 + digit(2)
        }
        Some(_) => return None,
    };
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000 + millis)
}

/// Adds `delta_ms` milliseconds to an ISO-8601 UTC timestamp.
pub fn add_millis_iso(base_iso: &str, delta_ms: i64) -> Option<String> {
    let base_ms = parse_iso_millis(base_iso)?;
    Some(format_millis_iso(base_ms.saturating_add(delta_ms)))
}

/// Parses an Effect-compatible duration string (`"5 minutes"`, `"2 days"`, `"500 millis"`, etc.)
/// or numeric millisecond string into milliseconds.
pub fn parse_duration_ms(input: &str) -> Result<i64, String> {
    let trimmed = input.trim();
    if let Ok(ms) = trimmed.parse::<i64>()
        && ms >= 0
    {
        return Ok(ms);
    }
    let mut parts = trimmed.split_whitespace();
    let Some(num_raw) = parts.next() else {
        return Err(invalid_duration_message(input));
    };
    let Some(unit_raw) = parts.next() else {
        return Err(invalid_duration_message(input));
    };
    if parts.next().is_some() {
        return Err(invalid_duration_message(input));
    }
    let Ok(amount) = num_raw.parse::<f64>() else {
        return Err(invalid_duration_message(input));
    };
    if !amount.is_finite() || amount < 0.0 {
        return Err(invalid_duration_message(input));
    }
    let unit_ms = match unit_raw.to_ascii_lowercase().as_str() {
        "milli" | "millis" | "millisecond" | "milliseconds" | "ms" => 1.0,
        "second" | "seconds" | "sec" | "secs" | "s" => 1_000.0,
        "minute" | "minutes" | "min" | "mins" | "m" => 60_000.0,
        "hour" | "hours" | "hr" | "hrs" | "h" => 3_600_000.0,
        "day" | "days" | "d" => 86_400_000.0,
        "week" | "weeks" | "w" => 7.0 * 86_400_000.0,
        _ => return Err(invalid_duration_message(input)),
    };
    let total = amount * unit_ms;
    #[expect(
        clippy::cast_precision_loss,
        reason = "bounded finite duration check against i64::MAX"
    )]
    let max_ms = i64::MAX as f64;
    if !total.is_finite() || total > max_ms {
        return Err(invalid_duration_message(input));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "total is finite, non-negative, and bounded by i64::MAX"
    )]
    Ok(total.round() as i64)
}

fn invalid_duration_message(input: &str) -> String {
    format!("cannot read {input:?} as a duration. Write one such as \"5 minutes\".")
}

/// Formats milliseconds concisely for trigger summaries and timeout messages.
pub fn format_duration_human(ms: i64) -> String {
    if ms <= 0 {
        return "0ms".to_owned();
    }
    if ms % 86_400_000 == 0 {
        let d = ms / 86_400_000;
        return format!("{d} {}", if d == 1 { "day" } else { "days" });
    }
    if ms % 3_600_000 == 0 {
        let h = ms / 3_600_000;
        return format!("{h} {}", if h == 1 { "hour" } else { "hours" });
    }
    if ms % 60_000 == 0 {
        let m = ms / 60_000;
        return format!("{m} {}", if m == 1 { "minute" } else { "minutes" });
    }
    if ms % 1_000 == 0 {
        let s = ms / 1_000;
        return format!("{s} {}", if s == 1 { "second" } else { "seconds" });
    }
    format!("{ms}ms")
}

/// Parses 24-hour `HH:MM` time-of-day.
pub fn parse_time_of_day(value: &str) -> Option<(u8, u8)> {
    let trimmed = value.trim();
    let (h_raw, m_raw) = trimmed.split_once(':')?;
    if !(1..=2).contains(&h_raw.len()) || m_raw.len() != 2 {
        return None;
    }
    if !h_raw.bytes().all(|b| b.is_ascii_digit()) || !m_raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hour: u8 = h_raw.parse().ok()?;
    let minute: u8 = m_raw.parse().ok()?;
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// Computes the next due timestamp (epoch ms) strictly after `from_ms` for `schedule`.
pub fn next_scheduled_run_ms(schedule: &ScheduleSpec, from_ms: i64) -> Option<i64> {
    match schedule {
        ScheduleSpec::Interval { every_ms } => {
            Some(from_ms.saturating_add((*every_ms).max(MIN_SCHEDULE_INTERVAL_MS)))
        }
        ScheduleSpec::FixedTime {
            time_of_day,
            weekdays,
        } => {
            let (hour, minute) = parse_time_of_day(time_of_day)?;
            let allowed: Option<BTreeSet<u8>> = weekdays
                .as_ref()
                .filter(|days| !days.is_empty())
                .map(|days| days.iter().copied().collect());
            let base_days = from_ms.div_euclid(86_400_000);
            for offset in 0..=7_i64 {
                let day_index = base_days + offset;
                let candidate = day_index * 86_400_000
                    + i64::from(hour) * 3_600_000
                    + i64::from(minute) * 60_000;
                if candidate <= from_ms {
                    continue;
                }
                // 1970-01-01 was Thursday (weekday = 4; 0 = Sunday).
                let weekday = u8::try_from((day_index + 4).rem_euclid(7)).ok()?;
                if let Some(ref set) = allowed
                    && !set.contains(&weekday)
                {
                    continue;
                }
                return Some(candidate);
            }
            None
        }
    }
}

/// Returns `true` when a fixed-time run is past the 10-minute grace period and should be skipped.
pub fn is_missed_fixed_time_run(schedule: &ScheduleSpec, due_ms: i64, now_ms: i64) -> bool {
    matches!(schedule, ScheduleSpec::FixedTime { .. })
        && now_ms.saturating_sub(due_ms) > MISSED_FIXED_TIME_GRACE_MS
}

const fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
