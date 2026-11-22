//! Safe Rust implementations of all six `__ssh-helper` operations.
//!
//! Mirrors `apps/server/src/cli/sshHelper.ts`, `apps/server/src/desktopUpdate/DesktopUpdateMarker.ts`,
//! `apps/server/src/cloud/serviceProtocol.ts`, and `apps/server/src/serviceLauncher.ts`.

#[path = "ssh_helper_net.rs"]
mod net;

use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use legion_persistence::ownership::{
    is_process_alive, legacy_owner_is_live, read_persisted_server_runtime_state,
};
use regex::Regex;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use net::is_loopback_http_origin;
pub use net::{
    ProbeOutcome, WaitReadyOutcome, answers_on_loopback, can_bind_loopback, format_picked_port,
    pick_port, probe_loopback_outcome_async, probe_loopback_ready_async, wait_ready,
    wait_ready_async, wait_ready_outcome_async,
};

/// TTL window in milliseconds enforced for `DesktopUpdateMarker` (`contracts/desktopBootstrap.ts:42`).
pub const DESKTOP_UPDATE_IN_PROGRESS_TTL_MS: u64 = 60_000;

fn exact_service_version_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:0|[1-9]\d*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
        );
        // INFALLIBLE: static SemVer pattern matching serviceProtocol.ts:166-168.
        compiled.expect("exact service version regex must compile")
    })
}

/// Returns `true` when `version` matches strict `SemVer` (`serviceProtocol.ts:164-171`).
///
/// ```
/// use legion_remote::ssh_helper::is_exact_service_version;
///
/// assert!(is_exact_service_version("0.1.7-nightly.20261102.4080"));
/// assert!(!is_exact_service_version("latest"));
/// ```
#[must_use]
pub fn is_exact_service_version(version: &str) -> bool {
    exact_service_version_regex().is_match(version)
}

/// Compares two strict `SemVer` strings matching `compareExactServiceVersions` (`serviceProtocol.ts:209-244`).
///
/// Build metadata (`+...`) is ignored; pre-release versions sort before their release counterpart.
#[must_use]
pub fn compare_exact_service_versions(left: &str, right: &str) -> Ordering {
    let (l_core, l_pre) = split_semver(left);
    let (r_core, r_pre) = split_semver(right);

    for (a, b) in l_core.iter().zip(r_core.iter()) {
        let ord = compare_decimal_str(a, b);
        if ord != Ordering::Equal {
            return ord;
        }
    }

    match (l_pre, r_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(lp), Some(rp)) => {
            let l_parts: Vec<&str> = lp.split('.').collect();
            let r_parts: Vec<&str> = rp.split('.').collect();
            let max_len = l_parts.len().max(r_parts.len());
            for idx in 0..max_len {
                match (l_parts.get(idx).copied(), r_parts.get(idx).copied()) {
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (None, None) => return Ordering::Equal,
                    (Some(x), Some(y)) => {
                        if x == y {
                            continue;
                        }
                        let x_num = x.bytes().all(|b| b.is_ascii_digit());
                        let y_num = y.bytes().all(|b| b.is_ascii_digit());
                        return match (x_num, y_num) {
                            (true, true) => compare_decimal_str(x, y),
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            (false, false) => x.cmp(y),
                        };
                    }
                }
            }
            Ordering::Equal
        }
    }
}

/// Evaluates serialized `server-runtime.json` text for `keep-server` (`sshHelper.ts:139-146`).
#[must_use]
pub fn keeps_running_server(runtime_json: &str, pid: u32, own_version: &str) -> bool {
    let Ok(val) = serde_json::from_str::<Value>(runtime_json) else {
        return false;
    };
    let Some(obj) = val.as_object() else {
        return false;
    };
    let Some(file_pid) = obj.get("pid").and_then(Value::as_u64) else {
        return false;
    };
    if file_pid != u64::from(pid) {
        return false;
    }
    let Some(server_version) = obj.get("serverVersion").and_then(Value::as_str) else {
        return false;
    };
    is_exact_service_version(server_version)
        && compare_exact_service_versions(server_version, own_version) != Ordering::Less
}

/// Returns `true` when `runtime_file` belongs to `pid` running `own_version` or newer (`sshHelper.ts:149-161`).
#[must_use]
pub fn keep_server(runtime_file: &Path, pid: u32, own_version: &str) -> bool {
    let Ok(raw) = fs::read_to_string(runtime_file) else {
        return false;
    };
    keeps_running_server(&raw, pid, own_version)
}

/// Reads `runtime_file`, returning `(pid, port)` when on loopback HTTP and alive (`sshHelper.ts:102-132`).
#[must_use]
pub fn runtime_port(runtime_file: &Path) -> Option<(u32, u16)> {
    let raw = fs::read_to_string(runtime_file).ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    let obj = val.as_object()?;
    let pid_u64 = obj.get("pid").and_then(Value::as_u64)?;
    let pid = u32::try_from(pid_u64).ok().filter(|&p| p > 0)?;
    let port_u64 = obj.get("port").and_then(Value::as_u64)?;
    let port = u16::try_from(port_u64).ok().filter(|&p| p > 0)?;
    let origin = obj.get("origin").and_then(Value::as_str)?;
    if !is_loopback_http_origin(origin) {
        return None;
    }
    if !is_process_alive(i64::from(pid)) {
        return None;
    }
    Some((pid, port))
}

/// Formats `runtime-port` stdout (`"{pid} {port}"`, NO trailing newline, `sshHelper.ts:126`).
#[must_use]
pub fn format_runtime_port(pid: u32, port: u16) -> String {
    format!("{pid} {port}")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMarkerJson {
    started_at: Value,
    expires_at: Value,
}

/// Computes remaining desktop update wait ms capped at [`DESKTOP_UPDATE_IN_PROGRESS_TTL_MS`].
///
/// Mirrors `DesktopUpdateMarker.remainingWait` (`DesktopUpdateMarker.ts:46-68`).
///
/// ```
/// use std::fs;
/// use legion_remote::ssh_helper::{format_remaining_update_wait, remaining_update_wait_ms};
///
/// let dir = tempfile::tempdir()?;
/// let marker = dir.path().join("update-in-progress.json");
/// fs::write(&marker, r#"{"startedAt":1000,"expiresAt":45000}"#)?;
/// let remaining = remaining_update_wait_ms(&marker, 20_000);
/// assert_eq!(remaining, Some(25_000));
/// assert_eq!(format_remaining_update_wait(remaining), "25000");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use]
pub fn remaining_update_wait_ms(marker_file: &Path, now_ms: i64) -> Option<u64> {
    let raw = fs::read_to_string(marker_file).ok()?;
    let marker: UpdateMarkerJson = serde_json::from_str(&raw).ok()?;
    let started_at = marker.started_at.as_f64()?;
    let expires_at = marker.expires_at.as_f64()?;
    let now_f64 = i64_to_f64_lossy(now_ms);
    if !started_at.is_finite()
        || !expires_at.is_finite()
        || started_at > now_f64
        || expires_at <= now_f64
        || expires_at <= started_at
    {
        return None;
    }
    let ttl_f64 = f64::from(u32::try_from(DESKTOP_UPDATE_IN_PROGRESS_TTL_MS).unwrap_or(u32::MAX));
    let clamped = (expires_at - now_f64).min(ttl_f64).max(0.0).trunc();
    let text = format!("{clamped:.0}");
    text.parse::<u64>().ok()
}

/// Formats `update-remaining` stdout (`""` or `"{ms}"`, NO trailing newline, `sshHelper.ts:170`).
#[must_use]
pub fn format_remaining_update_wait(remaining: Option<u64>) -> String {
    remaining.map_or_else(String::new, |ms| ms.to_string())
}

/// Returns `true` when a launcher or live server holds `base_dir` (`sshHelper.ts:184`, `serviceLauncher.ts:791-847`).
#[must_use]
pub fn is_home_owned(base_dir: &Path) -> bool {
    let serving_lock = base_dir.join("runtime").join("service-serving.lock");
    if serving_lock.exists() && is_sqlite_lock_held(&serving_lock) {
        return true;
    }

    let state_dir = base_dir.join("userdata");
    let owner_lock = state_dir.join("server-owner.sqlite");
    if owner_lock.exists() && is_sqlite_lock_held(&owner_lock) {
        return true;
    }

    let runtime_path = state_dir.join("server-runtime.json");
    if let Ok(Some(state)) = read_persisted_server_runtime_state(&runtime_path)
        && state.owner_id.is_none()
        && legacy_owner_is_live(&state)
    {
        return true;
    }

    false
}

fn is_sqlite_lock_held(lock_path: &Path) -> bool {
    let Ok(conn) = Connection::open_with_flags(lock_path, OpenFlags::SQLITE_OPEN_READ_WRITE) else {
        return false;
    };
    match conn.execute_batch("PRAGMA busy_timeout = 0; BEGIN EXCLUSIVE;") {
        Ok(()) => false,
        Err(err) => is_sqlite_busy(&err),
    }
}

fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(ffi_err, _) => {
            ffi_err.code == ErrorCode::DatabaseBusy || (ffi_err.extended_code & 0xff) == 5
        }
        _ => false,
    }
}

fn split_semver(version: &str) -> ([&str; 3], Option<&str>) {
    let without_build = version.split('+').next().unwrap_or(version);
    let (core, pre) = match without_build.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    let patch = parts.next().unwrap_or("0");
    ([major, minor, patch], pre)
}

fn compare_decimal_str(left: &str, right: &str) -> Ordering {
    let l_trim = left.trim_start_matches('0');
    let r_trim = right.trim_start_matches('0');
    l_trim
        .len()
        .cmp(&r_trim.len())
        .then_with(|| l_trim.cmp(r_trim))
}

fn i64_to_f64_lossy(value: i64) -> f64 {
    let neg = value < 0;
    let abs = value.unsigned_abs();
    let hi = u32::try_from(abs >> 32).unwrap_or(u32::MAX);
    let lo = u32::try_from(abs & 0xFFFF_FFFF).unwrap_or(0);
    let mag = f64::from(hi) * 4_294_967_296.0 + f64::from(lo);
    if neg { -mag } else { mag }
}
