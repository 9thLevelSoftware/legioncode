//! UTC ISO-8601 timestamp formatting and parsing helpers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use legion_persistence::ownership::{format_iso8601_utc, parse_iso8601_utc_millis};

/// Formats the current wall-clock UTC time as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn now_iso_utc() -> String {
    format_iso8601_utc(SystemTime::now())
}

/// Parses an ISO-8601 UTC timestamp (`...Z` or `...+00:00`) into Unix epoch milliseconds.
pub fn parse_iso_millis(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if let Some(stem) = trimmed.strip_suffix("+00:00") {
        let mut normalized = String::with_capacity(stem.len() + 1);
        normalized.push_str(stem);
        normalized.push('Z');
        return parse_iso8601_utc_millis(&normalized);
    }
    parse_iso8601_utc_millis(trimmed)
}

/// Formats Unix epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn format_millis_iso(epoch_ms: i64) -> String {
    if let Ok(millis) = u64::try_from(epoch_ms) {
        return format_iso8601_utc(UNIX_EPOCH + Duration::from_millis(millis));
    }
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

/// Adds `delta_ms` milliseconds to an ISO-8601 UTC timestamp.
pub fn add_millis_iso(base_iso: &str, delta_ms: i64) -> Option<String> {
    let base_ms = parse_iso_millis(base_iso)?;
    Some(format_millis_iso(base_ms.saturating_add(delta_ms)))
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
