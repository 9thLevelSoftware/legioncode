//! Minimal base-36 epoch formatting and ISO-8601 epoch-ms timestamp parsing for milestones.

use std::time::{SystemTime, UNIX_EPOCH};

const BASE36_DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Formats a non-negative integer as lowercase base-36 (matching JS `Number.prototype.toString(36)`).
pub fn format_base36(mut value: u64) -> String {
    if value == 0 {
        return "0".to_owned();
    }
    let mut buf = Vec::new();
    while value > 0 {
        let rem = usize::try_from(value % 36).unwrap_or(0);
        buf.push(BASE36_DIGITS[rem]);
        value /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_else(|_| "0".to_owned())
}

/// Parses an ISO-8601 UTC (`Z`) or numeric offset (`+HH:MM`/`-HH:MM`) timestamp to epoch milliseconds.
pub fn parse_iso_epoch_ms(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    let (date_part, rest) = trimmed.split_once('T')?;
    let mut date_iter = date_part.split('-');
    let year: i64 = date_iter.next()?.parse().ok()?;
    let month: i64 = date_iter.next()?.parse().ok()?;
    let day: i64 = date_iter.next()?.parse().ok()?;
    if date_iter.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (time_part, offset_ms) = if let Some(stem) = rest.strip_suffix('Z') {
        (stem, 0_i64)
    } else {
        let idx = rest.rfind(['+', '-'])?;
        let (stem, tz) = rest.split_at(idx);
        let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
        let tz_digits = &tz[1..];
        let (hh_raw, mm_raw) = tz_digits.split_once(':')?;
        let hh: i64 = hh_raw.parse().ok()?;
        let mm: i64 = mm_raw.parse().ok()?;
        if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) {
            return None;
        }
        (stem, sign * ((hh * 60 + mm) * 60 * 1_000))
    };

    let (hms_part, frac_ms) = match time_part.split_once('.') {
        Some((hms, frac)) => {
            let digits: String = frac.chars().take(3).collect();
            let mut ms: i64 = digits.parse().ok()?;
            if digits.len() == 1 {
                ms *= 100;
            } else if digits.len() == 2 {
                ms *= 10;
            }
            (hms, ms)
        }
        None => (time_part, 0_i64),
    };

    let mut time_iter = hms_part.split(':');
    let hour: i64 = time_iter.next()?.parse().ok()?;
    let minute: i64 = time_iter.next()?.parse().ok()?;
    let second: i64 = time_iter.next()?.parse().ok()?;
    if time_iter.next().is_some()
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    let total_ms = secs.checked_mul(1_000)?.checked_add(frac_ms)?;
    total_ms.checked_sub(offset_ms)
}

const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(super) fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|dur| i64::try_from(dur.as_millis()).ok())
        .unwrap_or(0)
}
