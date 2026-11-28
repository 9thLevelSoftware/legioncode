//! Loopback/origin guards, cookie scanning, and URL query helpers for `/ws` upgrade auth.

use axum::http::HeaderMap;
use axum::http::header::HOST;

pub(super) fn find_prefixed_cookie(cookie_header: &str, prefix: &str) -> Option<String> {
    for segment in cookie_header.split(';') {
        let trimmed = segment.trim();
        let Some((name, val)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim().starts_with(prefix) {
            let value = val.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

pub(super) fn is_direct_loopback_request(headers: &HeaderMap) -> bool {
    const PROXY_HEADERS: [&str; 5] = [
        "x-forwarded-for",
        "x-forwarded-host",
        "forwarded",
        "cf-connecting-ip",
        "true-client-ip",
    ];
    if PROXY_HEADERS.iter().any(|name| headers.contains_key(*name)) {
        return false;
    }

    let Some(host_val) = headers.get(HOST).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    is_loopback_authority(host_val.trim())
}

pub(super) fn is_allowed_loopback_origin(origin: &str) -> bool {
    let trimmed = origin.trim();
    let lower = trimmed.to_ascii_lowercase();
    if [
        "app://",
        "legioncode://",
        "capacitor://",
        "ionic://",
        "file://",
        "tauri://",
    ]
    .iter()
    .any(|scheme| lower.starts_with(scheme))
    {
        return true;
    }
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"));
    let Some(authority) = rest else {
        return false;
    };
    let host_part = authority.split('/').next().unwrap_or("");
    is_loopback_authority(host_part)
}

fn is_loopback_authority(authority: &str) -> bool {
    let host = if let Some(after_bracket) = authority.strip_prefix('[') {
        after_bracket.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

pub(super) fn constant_time_eq(lhs: &[u8], rhs: &[u8]) -> bool {
    if lhs.len() != rhs.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (a, b) in lhs.iter().zip(rhs.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub(super) fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let q = query?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(v.to_owned());
        }
    }
    None
}

pub(super) fn has_query_flag(query: Option<&str>, key: &str) -> bool {
    let Some(q) = query else {
        return false;
    };
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return !v.eq_ignore_ascii_case("0") && !v.eq_ignore_ascii_case("false");
        }
    }
    false
}

pub(super) fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
