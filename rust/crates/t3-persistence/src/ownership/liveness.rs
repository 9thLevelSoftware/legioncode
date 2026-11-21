//! Cross-platform OS PID liveness, legacy owner start-time checks, and UTC ISO-8601 helpers.

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::PersistedServerRuntimeState;

/// Probes OS process liveness safely without `unsafe` FFI (`isProcessAlive` in `serverRuntimeState.ts:132-140`).
pub fn is_process_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    if i64::from(std::process::id()) == pid {
        return true;
    }
    is_other_process_alive(pid)
}

#[cfg(unix)]
fn is_other_process_alive(pid: i64) -> bool {
    let Ok(output) = Command::new("kill").args(["-0", &pid.to_string()]).output() else {
        return false;
    };
    if output.status.success() {
        return true;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    stderr.contains("operation not permitted") || stderr.contains("not permitted")
}

#[cfg(windows)]
fn is_other_process_alive(pid: i64) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Ok(output) = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let marker = format!(",\"{pid}\",");
    String::from_utf8_lossy(&output.stdout).contains(&marker)
}

/// Checks whether a pre-lock (`owner_id == None`) runtime record belongs to a live legacy process.
pub fn legacy_owner_is_live(state: &PersistedServerRuntimeState) -> bool {
    if !is_process_alive(state.pid) {
        return false;
    }
    let Some(recorded_ms) = parse_iso8601_utc_millis(&state.started_at) else {
        return true;
    };
    let Some(started_ms) = probe_process_start_ms(state.pid) else {
        return true;
    };
    started_ms <= recorded_ms.saturating_add(1_000)
}

fn probe_process_start_ms(pid: i64) -> Option<i64> {
    #[cfg(windows)]
    let mut child = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToUniversalTime().ToString('o')"
                ),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?
    };
    #[cfg(unix)]
    let mut child = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let output = child.wait_with_output().ok()?;
                let text = String::from_utf8_lossy(&output.stdout);
                return parse_process_start_output(text.trim());
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn parse_process_start_output(text: &str) -> Option<i64> {
    #[cfg(windows)]
    {
        parse_iso8601_utc_millis(text)
    }
    #[cfg(unix)]
    {
        parse_ps_lstart_utc_millis(text)
    }
}

/// Formats Unix epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn format_epoch_millis_utc(epoch_ms: i64) -> String {
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

/// Formats a [`SystemTime`] as `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub fn format_iso8601_utc(time: SystemTime) -> String {
    let dur = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let epoch_ms = i64::try_from(dur.as_millis()).unwrap_or(i64::MAX);
    format_epoch_millis_utc(epoch_ms)
}

/// Parses an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff]Z`) into Unix epoch milliseconds.
pub fn parse_iso8601_utc_millis(input: &str) -> Option<i64> {
    let s = input.trim().strip_suffix('Z')?;
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
    if time_parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month)
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
            let d = |idx: usize| -> i64 { bytes.get(idx).map_or(0, |b| i64::from(b - b'0')) };
            d(0) * 100 + d(1) * 10 + d(2)
        }
        Some(_) => return None,
    };

    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000 + millis)
}

#[cfg(unix)]
fn parse_ps_lstart_utc_millis(input: &str) -> Option<i64> {
    let mut parts = input.split_whitespace();
    let _weekday = parts.next()?;
    let month_str = parts.next()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let hms = parts.next()?;
    let year: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let month = match month_str {
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
    let mut time_parts = hms.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000)
}

// Howard Hinnant's civil-from-days / days-from-civil algorithm.
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
