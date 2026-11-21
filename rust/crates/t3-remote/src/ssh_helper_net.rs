//! Loopback port picking, HTTP readiness probing, and origin validation for [`super`].

use std::fs;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::Client;

/// Validates that `origin` is `http://127.0.0.1[:port]` or `http://localhost[:port]` (`sshHelper.ts:120-124`).
pub(super) fn is_loopback_http_origin(origin: &str) -> bool {
    let trimmed = origin.trim();
    let Some(rest) = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("HTTP://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.starts_with('[') {
        return false;
    }
    let (host, port_part) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(port_str) = port_part
        && port_str.parse::<u16>().is_err()
    {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    lower == "127.0.0.1" || lower == "localhost"
}

/// Checks whether `127.0.0.1:<port>` can be bound cleanly (`sshHelper.ts:18-26`).
#[must_use]
pub fn can_bind_loopback(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Checks whether anything currently answers or times out on `127.0.0.1:<port>` (`sshHelper.ts:29-42`).
///
/// Matches Node `Net.connect` behavior: `connect` or `timeout` counts as occupied (`true`);
/// immediate connection failure (`ConnectionRefused`, etc.) returns `false`.
#[must_use]
pub fn answers_on_loopback(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    // Windows Winsock delays WSAECONNREFUSED ~2.08s on closed loopback ports; 2.5s avoids false TimedOut.
    let timeout = if cfg!(windows) {
        Duration::from_millis(2_500)
    } else {
        Duration::from_secs(1)
    };
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => true,
        Err(err) => err.kind() == ErrorKind::TimedOut,
    }
}

/// Picks the first free loopback port starting from `port_file` (or `default_port`) within `scan_window`.
///
/// Mirrors `__ssh-helper pick-port` (`sshHelper.ts:44-63`).
#[must_use]
pub fn pick_port(port_file: &Path, default_port: u16, scan_window: u16) -> Option<u16> {
    let preferred = fs::read_to_string(port_file)
        .ok()
        .and_then(|raw| parse_js_decimal_prefix(&raw))
        .and_then(|n| u16::try_from(n).ok())
        .filter(|&p| p >= 1);
    let start = preferred.unwrap_or(default_port);

    for offset in 0..scan_window {
        let Some(port) = start.checked_add(offset) else {
            break;
        };
        if port == 0 {
            continue;
        }
        if can_bind_loopback(port) && !answers_on_loopback(port) {
            return Some(port);
        }
    }
    None
}

/// Formats `pick-port` stdout (`"{port}"`, NO trailing newline, `sshHelper.ts:56`).
#[must_use]
pub fn format_picked_port(port: u16) -> String {
    port.to_string()
}

/// Performs a single HTTP `GET http://127.0.0.1:<port>/` probe returning `true` for status `200..300`.
pub async fn probe_loopback_ready_async(port: u16, probe_timeout_ms: u64) -> bool {
    let Ok(client) = Client::builder()
        .timeout(Duration::from_millis(probe_timeout_ms.max(1)))
        .build()
    else {
        return false;
    };
    let url = format!("http://127.0.0.1:{port}/");
    match client.get(&url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Polls `http://127.0.0.1:<port>/` every 100ms up to `timeout_ms` until HTTP 2xx (`sshHelper.ts:84-99`).
pub async fn wait_ready_async(port: u16, timeout_ms: u64, probe_timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if probe_loopback_ready_async(port, probe_timeout_ms).await {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(100))).await;
    }
    false
}

/// Blocking wrapper around [`wait_ready_async`] for synchronous CLI `__ssh-helper wait-ready` callers.
///
/// Spawns a dedicated helper thread with a current-thread Tokio runtime so it never panics
/// even when invoked from inside an existing async runtime context.
#[must_use]
pub fn wait_ready(port: u16, timeout_ms: u64, probe_timeout_ms: u64) -> bool {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .is_ok_and(|rt| rt.block_on(wait_ready_async(port, timeout_ms, probe_timeout_ms)))
    })
    .join()
    .unwrap_or(false)
}

fn parse_js_decimal_prefix(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let digits_end = trimmed
        .char_indices()
        .find(|&(idx, ch)| !(ch.is_ascii_digit() || (idx == 0 && matches!(ch, '+' | '-'))))
        .map_or(trimmed.len(), |(idx, _)| idx);
    let prefix = &trimmed[..digits_end];
    if prefix.is_empty() || prefix == "+" || prefix == "-" {
        return None;
    }
    prefix.parse::<i64>().ok()
}
