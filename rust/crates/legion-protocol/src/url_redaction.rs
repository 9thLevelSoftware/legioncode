//! URL query and signed-route redaction helpers for server logs and trace spans.
//!
//! Covers signed asset, attachment-upload, and webhook URL path tokens
//! (`c57ccdaf52` / #39464, `ed3b241eaa` / #40133) as well as signed query
//! parameters (`token`, `sig`, `signature`, `access_token`) while keeping
//! non-sensitive query parameters and ordinary route paths intact.

/// Replacement value written into redacted query parameters.
pub const REDACTED_QUERY_VALUE: &str = "[REDACTED]";

/// Replacement path segment written into redacted signed route prefixes.
pub const REDACTED_PATH_SEGMENT: &str = "REDACTED";

/// Query parameter keys treated as signed bearer credentials (ASCII case-insensitive).
pub const SENSITIVE_QUERY_KEYS: &[&str] = &["token", "sig", "signature", "access_token"];

/// URL path prefix for signed asset downloads.
pub const ASSET_ROUTE_PREFIX: &str = "/api/assets";

/// URL path prefix for signed attachment uploads.
pub const ATTACHMENT_UPLOAD_ROUTE_PREFIX: &str = "/api/attachments/upload";

/// URL path prefix for incoming webhooks (`<prefix>/<hook_id>/<token>`).
pub const WEBHOOK_ROUTE_PREFIX: &str = "/api/hooks";

/// Returns `true` when `key` identifies a signed token or signature query parameter.
#[must_use]
pub fn is_sensitive_query_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    SENSITIVE_QUERY_KEYS.contains(&normalized.as_str())
}

/// Redacts sensitive query parameters (`token`, `sig`, `signature`, `access_token`) to
/// `[REDACTED]` and masks HTTP `userinfo` credentials if present, preserving paths and
/// non-sensitive query parameters.
#[must_use]
pub fn redact_url_query_tokens(raw: &str) -> String {
    let (without_frag, frag) = match raw.find('#') {
        Some(idx) => (&raw[..idx], Some(&raw[idx..])),
        None => (raw, None),
    };
    let (head, query) = match without_frag.find('?') {
        Some(idx) => (&without_frag[..idx], Some(&without_frag[idx + 1..])),
        None => (without_frag, None),
    };

    let redacted_head = redact_authority_userinfo(head);
    let mut out = redacted_head;
    if let Some(q) = query {
        out.push('?');
        append_redacted_query(&mut out, q);
    }
    if let Some(f) = frag {
        out.push_str(f);
    }
    out
}

/// Redacts a signed route path (`/api/assets/...`, `/api/attachments/upload/...`,
/// `/api/hooks/<id>/...`) to end with `REDACTED`, returning `None` for normal routes.
#[must_use]
pub fn redact_signed_route_path(path: &str) -> Option<String> {
    if path.starts_with(&format!("{ASSET_ROUTE_PREFIX}/")) {
        return Some(format!("{ASSET_ROUTE_PREFIX}/{REDACTED_PATH_SEGMENT}"));
    }
    if path.starts_with(&format!("{ATTACHMENT_UPLOAD_ROUTE_PREFIX}/")) {
        return Some(format!(
            "{ATTACHMENT_UPLOAD_ROUTE_PREFIX}/{REDACTED_PATH_SEGMENT}"
        ));
    }
    if let Some(rest) = path.strip_prefix(&format!("{WEBHOOK_ROUTE_PREFIX}/"))
        && let Some((hook_id, _secret)) = rest.split_once('/')
        && !hook_id.is_empty()
    {
        return Some(format!(
            "{WEBHOOK_ROUTE_PREFIX}/{hook_id}/{REDACTED_PATH_SEGMENT}"
        ));
    }
    None
}

/// Redacts signed path tokens, authority `userinfo`, and sensitive query parameters (`token`,
/// `sig`, `signature`, `access_token` -> `[REDACTED]`) while preserving normal route paths
/// and non-sensitive query parameters.
#[must_use]
pub fn redact_url(raw: &str) -> String {
    let (without_frag, frag) = match raw.find('#') {
        Some(idx) => (&raw[..idx], Some(&raw[idx..])),
        None => (raw, None),
    };
    let (head, query) = match without_frag.find('?') {
        Some(idx) => (&without_frag[..idx], Some(&without_frag[idx + 1..])),
        None => (without_frag, None),
    };

    let redacted_head = redact_signed_head(head);
    let mut out = redacted_head;
    if let Some(q) = query {
        out.push('?');
        append_redacted_query(&mut out, q);
    }
    if let Some(f) = frag {
        out.push_str(f);
    }
    out
}

/// Redacts an HTTP request target (`http.url` span/log attribute) per `#39464`/`#40133`,
/// stripping trailing query noise on signed path routes and masking signed query tokens
/// on all other routes.
#[must_use]
pub fn redact_http_request_target(raw: &str) -> String {
    let (without_frag, _frag) = match raw.find('#') {
        Some(idx) => (&raw[..idx], Some(&raw[idx..])),
        None => (raw, None),
    };
    let head = without_frag
        .split_once('?')
        .map_or(without_frag, |(h, _)| h);
    let (prefix, path) = split_origin_and_path(head);
    if let Some(redacted_path) = redact_signed_route_path(path) {
        let mut out = redact_authority_userinfo(prefix);
        out.push_str(&redacted_path);
        return out;
    }
    redact_url_query_tokens(raw)
}

fn redact_signed_head(head: &str) -> String {
    let (prefix, path) = split_origin_and_path(head);
    let safe_prefix = redact_authority_userinfo(prefix);
    if let Some(redacted_path) = redact_signed_route_path(path) {
        return format!("{safe_prefix}{redacted_path}");
    }
    format!("{safe_prefix}{path}")
}

fn split_origin_and_path(head: &str) -> (&str, &str) {
    if let Some(scheme_idx) = head.find("://") {
        let after_scheme = &head[scheme_idx + 3..];
        if let Some(slash_rel) = after_scheme.find('/') {
            let split_at = scheme_idx + 3 + slash_rel;
            return (&head[..split_at], &head[split_at..]);
        }
        return (head, "");
    }
    ("", head)
}

fn redact_authority_userinfo(origin_or_head: &str) -> String {
    let Some(scheme_idx) = origin_or_head.find("://") else {
        return origin_or_head.to_owned();
    };
    let auth_start = scheme_idx + 3;
    let rest = &origin_or_head[auth_start..];
    let auth_end_rel = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..auth_end_rel];
    let Some(at_rel) = authority.rfind('@') else {
        return origin_or_head.to_owned();
    };
    let mut out = String::with_capacity(origin_or_head.len());
    out.push_str(&origin_or_head[..auth_start]);
    out.push_str(REDACTED_QUERY_VALUE);
    out.push_str(&rest[at_rel..]);
    out
}

fn append_redacted_query(out: &mut String, query: &str) {
    if query.is_empty() {
        return;
    }
    for (idx, part) in query.split('&').enumerate() {
        if idx > 0 {
            out.push('&');
        }
        match part.split_once('=') {
            Some((key, val)) => {
                out.push_str(key);
                out.push('=');
                if is_sensitive_query_key(key) {
                    out.push_str(REDACTED_QUERY_VALUE);
                } else {
                    out.push_str(val);
                }
            }
            None => {
                if is_sensitive_query_key(part) {
                    out.push_str(part);
                    out.push('=');
                    out.push_str(REDACTED_QUERY_VALUE);
                } else {
                    out.push_str(part);
                }
            }
        }
    }
}
