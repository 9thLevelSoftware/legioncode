//! Shared URI authority validation and `application/x-www-form-urlencoded` helpers.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Returns `true` when `host` identifies the local loopback interface
/// (`localhost`, `127.0.0.1`, `127.*`, `[::1]`, `::1`, or valid IPv6 loopback).
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ipv4) = host.parse::<Ipv4Addr>() {
        return ipv4.is_loopback();
    }
    let ipv6_candidate = if host.starts_with('[') || host.ends_with(']') {
        host.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
    } else {
        Some(host)
    };
    ipv6_candidate
        .and_then(|candidate| candidate.parse::<Ipv6Addr>().ok())
        .is_some_and(|ipv6| ipv6.is_loopback())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedUrlParts {
    pub(crate) scheme: String,
    pub(crate) origin: String,
    pub(crate) pathname: String,
    pub(crate) query_params: Vec<(String, String)>,
    pub(crate) has_userinfo: bool,
    pub(crate) fragment: Option<String>,
}

/// Returns `true` if `scheme` obeys RFC 3986 scheme syntax (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`).
pub(crate) fn is_valid_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// Normalizes URI authority (`[userinfo@]host[:port]`) for the lowercased `scheme`,
/// stripping default HTTP/WS ports and lowercasing the host.
pub(crate) fn normalize_authority(authority: &str, scheme: &str) -> Option<String> {
    if authority
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b == b'\\')
    {
        return None;
    }

    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((user, rest)) => (Some(user), rest),
        None => (None, authority),
    };

    let (host_lower, port) = parse_host_port(host_port)?;
    if host_lower.is_empty() && matches!(scheme, "http" | "https" | "ws" | "wss") {
        return None;
    }

    let is_default_port = matches!(
        (scheme, port),
        ("http" | "ws", Some(80)) | ("https" | "wss", Some(443))
    );

    let mut out = String::with_capacity(authority.len());
    if let Some(user) = userinfo {
        out.push_str(user);
        out.push('@');
    }
    out.push_str(&host_lower);
    if let Some(p) = port
        && !is_default_port
    {
        let _ = write!(out, ":{p}");
    }
    Some(out)
}

enum ParsedPort {
    Absent,
    Port(u16),
}

fn parse_host_port(host_port: &str) -> Option<(String, Option<u16>)> {
    if host_port.starts_with('[') {
        let close = host_port.find(']')?;
        let host = &host_port[..=close];
        if host.len() <= 2 {
            return None;
        }
        let rest = &host_port[close + 1..];
        let port = match parse_optional_port(rest)? {
            ParsedPort::Absent => None,
            ParsedPort::Port(p) => Some(p),
        };
        return Some((host.to_ascii_lowercase(), port));
    }

    if let Some((host, port_str)) = host_port.split_once(':') {
        if port_str.contains(':') {
            return None;
        }
        let port = match parse_optional_port_digits(port_str)? {
            ParsedPort::Absent => None,
            ParsedPort::Port(p) => Some(p),
        };
        Some((host.to_ascii_lowercase(), port))
    } else {
        Some((host_port.to_ascii_lowercase(), None))
    }
}

fn parse_optional_port(rest: &str) -> Option<ParsedPort> {
    if rest.is_empty() {
        return Some(ParsedPort::Absent);
    }
    let digits = rest.strip_prefix(':')?;
    parse_optional_port_digits(digits)
}

fn parse_optional_port_digits(digits: &str) -> Option<ParsedPort> {
    if digits.is_empty() {
        return Some(ParsedPort::Absent);
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = digits.parse::<u16>().ok()?;
    Some(ParsedPort::Port(port))
}

/// Encodes `input` per WHATWG `application/x-www-form-urlencoded` (matching `URLSearchParams`).
pub(crate) fn encode_form_urlencoded(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b' ' => out.push('+'),
            b'*' | b'-' | b'.' | b'0'..=b'9' | b'A'..=b'Z' | b'_' | b'a'..=b'z' => {
                out.push(char::from(byte));
            }
            other => {
                out.push('%');
                out.push(char::from(HEX[usize::from(other >> 4)]));
                out.push(char::from(HEX[usize::from(other & 0x0F)]));
            }
        }
    }
    out
}

/// Decodes a WHATWG `application/x-www-form-urlencoded` component (`+` -> space, `%XX` -> bytes).
fn decode_form_urlencoded(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => {
                out.push(b' ');
                idx += 1;
            }
            b'%' if idx + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) =
                    (hex_nibble(bytes[idx + 1]), hex_nibble(bytes[idx + 2]))
                {
                    out.push((hi << 4) | lo);
                    idx += 3;
                } else {
                    out.push(b'%');
                    idx += 1;
                }
            }
            other => {
                out.push(other);
                idx += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Drops all `"token"` query parameters and re-serializes remaining parameters in `URLSearchParams` order.
pub(crate) fn serialize_query_without_token(raw_query: &str) -> String {
    let mut out = String::new();
    for part in raw_query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (raw_key, raw_val) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        let key = decode_form_urlencoded(raw_key);
        if key == "token" {
            continue;
        }
        let val = decode_form_urlencoded(raw_val);
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&encode_form_urlencoded(&key));
        out.push('=');
        out.push_str(&encode_form_urlencoded(&val));
    }
    out
}

/// Rewrites `connection_string` to `/pair`, preserves non-`token` query parameters,
/// and encodes `token` in the `#token=...` hash fragment (`startupAccess.ts`).
pub(crate) fn build_pairing_url_impl(connection_string: &str, token: &str) -> Option<String> {
    let trimmed = connection_string.trim();
    if trimmed.is_empty() || trimmed.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return None;
    }

    let scheme_end = trimmed.find("://")?;
    let raw_scheme = &trimmed[..scheme_end];
    if !is_valid_scheme(raw_scheme) {
        return None;
    }
    let scheme = raw_scheme.to_ascii_lowercase();

    let after_scheme = &trimmed[scheme_end + 3..];
    let without_fragment = match after_scheme.split_once('#') {
        Some((before, _)) => before,
        None => after_scheme,
    };
    let (before_query, raw_query) = match without_fragment.split_once('?') {
        Some((before, q)) => (before, Some(q)),
        None => (without_fragment, None),
    };
    let authority = match before_query.split_once('/') {
        Some((auth, _)) => auth,
        None => before_query,
    };

    let normalized_authority = normalize_authority(authority, &scheme)?;
    let query = raw_query.map_or_else(String::new, serialize_query_without_token);
    let encoded_token = encode_form_urlencoded(token);

    let mut out = format!("{scheme}://{normalized_authority}/pair");
    if !query.is_empty() {
        out.push('?');
        out.push_str(&query);
    }
    out.push_str("#token=");
    out.push_str(&encoded_token);
    Some(out)
}

/// Decodes all key/value query parameters in order.
pub(crate) fn parse_query_params(raw_query: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for part in raw_query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (raw_key, raw_val) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        params.push((
            decode_form_urlencoded(raw_key),
            decode_form_urlencoded(raw_val),
        ));
    }
    params
}

/// Parses an explicit-scheme URI into normalized origin, pathname, query params, userinfo flag, and fragment.
pub(crate) fn parse_url_parts(raw: &str) -> Option<ParsedUrlParts> {
    if raw.is_empty() || raw.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return None;
    }
    let scheme_end = raw.find("://")?;
    let raw_scheme = &raw[..scheme_end];
    if !is_valid_scheme(raw_scheme) {
        return None;
    }
    let scheme = raw_scheme.to_ascii_lowercase();
    let after_scheme = &raw[scheme_end + 3..];
    let (before_fragment, fragment) = match after_scheme.split_once('#') {
        Some((before, frag)) => (before, Some(frag.to_owned())),
        None => (after_scheme, None),
    };
    let (before_query, raw_query) = match before_fragment.split_once('?') {
        Some((before, q)) => (before, Some(q)),
        None => (before_fragment, None),
    };
    let (authority, pathname) = match before_query.split_once('/') {
        Some((auth, rest)) => (auth, format!("/{rest}")),
        None => (before_query, "/".to_owned()),
    };
    if authority
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b == b'\\')
    {
        return None;
    }
    let (has_userinfo, host_port) = match authority.rsplit_once('@') {
        Some((_user, rest)) => (true, rest),
        None => (false, authority),
    };
    let normalized_host_port = normalize_authority(host_port, &scheme)?;
    let origin = format!("{scheme}://{normalized_host_port}");
    let query_params = raw_query.map_or_else(Vec::new, parse_query_params);
    Some(ParsedUrlParts {
        scheme,
        origin,
        pathname,
        query_params,
        has_userinfo,
        fragment,
    })
}
