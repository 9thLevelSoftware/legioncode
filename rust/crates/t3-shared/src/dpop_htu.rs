//! URL normalization and canonical JSON helpers for `DPoP` (`RFC 9449` / `RFC 7638`).

use std::fmt::Write as _;

use crate::url_authority::{is_valid_scheme, normalize_authority};

/// Normalizes a `DPoP` target URI (`htu`) by lowercasing scheme/host, stripping default ports,
/// resolving path dot segments, and discarding query and fragment components per RFC 9449.
///
/// Returns `None` if `url` is not a valid absolute URI with an authority (or lacks a host for
/// `http`/`https`/`ws`/`wss`).
///
/// # Examples
///
/// ```
/// use t3_shared::dpop::normalize_dpop_htu;
///
/// assert_eq!(
///     normalize_dpop_htu("https://example.com/v1/environments/env/connect?foo=bar#frag"),
///     Some("https://example.com/v1/environments/env/connect".to_owned()),
/// );
/// assert_eq!(
///     normalize_dpop_htu("HTTPS://Example.COM:443?a=1"),
///     Some("https://example.com/".to_owned()),
/// );
/// assert_eq!(normalize_dpop_htu("/relative/path"), None);
/// ```
pub fn normalize_dpop_htu(url: &str) -> Option<String> {
    let trimmed = url.trim();
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
    let without_fragment = after_scheme.split('#').next().unwrap_or(after_scheme);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);

    let (authority, raw_path) = match without_query.find('/') {
        Some(idx) => (&without_query[..idx], &without_query[idx..]),
        None => (without_query, "/"),
    };

    let normalized_authority = normalize_authority(authority, &scheme)?;
    let normalized_path = normalize_path(raw_path);
    Some(format!(
        "{scheme}://{normalized_authority}{normalized_path}"
    ))
}

fn normalize_path(raw_path: &str) -> String {
    if !raw_path.contains("/.") {
        return if raw_path.is_empty() {
            "/".to_owned()
        } else {
            raw_path.to_owned()
        };
    }

    let trailing_slash = raw_path.ends_with('/')
        || raw_path.ends_with("/.")
        || raw_path.ends_with("/..")
        || raw_path == "."
        || raw_path == "..";

    let mut stack: Vec<&str> = Vec::new();
    for segment in raw_path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }

    if stack.is_empty() {
        return "/".to_owned();
    }

    let mut out = String::with_capacity(raw_path.len());
    for segment in stack {
        out.push('/');
        out.push_str(segment);
    }
    if trailing_slash {
        out.push('/');
    }
    out
}

/// Appends `value` as a JSON-escaped string literal (`"..."`) per RFC 8259.
pub(crate) fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            '\x08' => out.push_str(r"\b"),
            '\x0C' => out.push_str(r"\f"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
