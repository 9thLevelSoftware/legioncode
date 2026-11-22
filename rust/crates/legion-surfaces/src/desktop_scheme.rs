//! Desktop custom-scheme URL/path helpers, CSP header builder, and MIME lookup.

use crate::desktop::{DESKTOP_PRODUCTION_SCHEME, DesktopRejectionReason, DesktopRouteDecision};

/// Generates the desktop renderer `Content-Security-Policy` matching `ElectronProtocol.ts:69-98`.
#[must_use]
pub fn build_desktop_csp(scheme: &str, clerk_frontend_api_hostname: Option<&str>) -> String {
    let clerk = clerk_frontend_api_hostname
        .and_then(sanitize_hostname_token)
        .map(|host| format!(" https://{host}"));
    let clerk_src = clerk.as_deref().unwrap_or("");
    let clean_scheme = sanitize_scheme_token(scheme);
    [
        "default-src 'self'".to_owned(),
        format!(
            "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'{clerk_src} https://challenges.cloudflare.com"
        ),
        "connect-src 'self' blob: http: https: ws: wss:".to_owned(),
        format!("img-src 'self' {clean_scheme}: blob: data: http: https:"),
        format!("media-src 'self' {clean_scheme}: blob: http: https:"),
        "style-src 'self' 'unsafe-inline'".to_owned(),
        format!("font-src 'self' {clean_scheme}: data:"),
        "worker-src 'self' blob:".to_owned(),
        "frame-src 'self' blob: http: https:".to_owned(),
        "object-src 'self' blob: http: https:".to_owned(),
        "form-action 'self'".to_owned(),
    ]
    .join("; ")
}

/// Maps a file path's extension to its MIME `Content-Type` header.
#[must_use]
pub fn mime_type_for_path(path: &str) -> &'static str {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, ext)) = file_name.rsplit_once('.') else {
        return "application/octet-stream";
    };
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "webm" => "video/webm",
        "mp4" => "video/mp4",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        _ => "application/octet-stream",
    }
}

pub(crate) fn rejected(
    status: u16,
    reason: DesktopRejectionReason,
    content_security_policy: String,
) -> DesktopRouteDecision {
    DesktopRouteDecision::Rejected {
        status,
        reason,
        content_security_policy,
    }
}

pub(crate) fn split_custom_scheme_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, after_scheme) = url.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let without_fragment = after_scheme.split('#').next().unwrap_or(after_scheme);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let (authority, path) = match without_query.find('/') {
        Some(idx) => (&without_query[..idx], &without_query[idx..]),
        None => (without_query, "/"),
    };
    Some((scheme, authority, path))
}

pub(crate) fn percent_decode_strict(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0_usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            let hi = hex_nibble(*bytes.get(idx + 1)?)?;
            let lo = hex_nibble(*bytes.get(idx + 2)?)?;
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn normalize_safe_relative_path(decoded_path: &str) -> Option<String> {
    if decoded_path.contains('\\') {
        return None;
    }
    let mut segments = Vec::new();
    for seg in decoded_path.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg == "." || seg == ".." {
            return None;
        }
        segments.push(seg);
    }
    Some(segments.join("/"))
}

pub(crate) fn has_file_extension(relative_path: &str) -> bool {
    let file_name = relative_path.rsplit('/').next().unwrap_or(relative_path);
    matches!(file_name.rsplit_once('.'), Some((stem, ext)) if !stem.is_empty() && !ext.is_empty())
}

pub(crate) fn sanitize_hostname_token(host: &str) -> Option<String> {
    let trimmed = host.trim();
    if !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
    {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

fn sanitize_scheme_token(scheme: &str) -> String {
    let clean: String = scheme
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .collect();
    if clean.is_empty() {
        DESKTOP_PRODUCTION_SCHEME.to_owned()
    } else {
        clean
    }
}
