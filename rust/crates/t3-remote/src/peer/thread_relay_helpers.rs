//! Helper parsers, cursor defaults, and error mappers for [`super::thread_relay`].

use std::sync::OnceLock;

use regex::Regex;
use reqwest::header::HeaderMap;
use serde_json::{Map, Value};
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use t3_contracts::peer_links::{
    LINK_EXPIRES_AT_HEADER, PeerLinkThreadReadError, PeerLinkThreadReadReason,
};

use crate::peer::links::{PeerLinkError, PeerLinkErrorReason};

/// Maximum idle interval before an open linked thread SSE stream is treated as dead (`#39406`).
pub const SILENT_STREAM_TIMEOUT_SECS: u64 = 75;
/// Cursor sentinel (`Number.MAX_SAFE_INTEGER`) used to request the newest thread page (`#40003`).
pub const NEWEST_PAGE_BEFORE_POSITION: i64 = 9_007_199_254_740_991;

fn before_position_refusal_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r#"^Invalid parameters for tool '(?:t3|legion)_thread_read': Unknown arguments? (?:"[^"]+", )*"beforePosition"(?:, "[^"]+")*\. Accepted arguments: "#,
        );
        // INFALLIBLE: constant compile-time validated MCP parameter refusal pattern (#40003).
        compiled.expect("beforePosition refusal regex must compile")
    })
}

/// Returns actionable guidance explaining how to enable linked environments both via UI and on a
/// headless server (`#39964`).
#[must_use]
pub fn headless_linked_environments_disabled_hint() -> String {
    headless_linked_environments_disabled_hint_for_brand(&ACTIVE_BRAND)
}

/// Brand-parameterized variant of [`headless_linked_environments_disabled_hint`] (`#39964`).
#[must_use]
pub fn headless_linked_environments_disabled_hint_for_brand(brand: &BrandConfig) -> String {
    format!(
        "Linked environments is off on that machine. Turn it on in its Settings → Labs, or without a desktop, set \"linkedEnvironmentsEnabled\": true in userdata/settings.json in its {} home (~/{} by default).",
        brand.product_name, brand.home_dir_name
    )
}

/// Maps a local [`PeerLinkError`] into a client-facing [`PeerLinkThreadReadError`] (`#39406`).
#[must_use]
pub fn from_link_error(error: &PeerLinkError, label: &str) -> PeerLinkThreadReadError {
    let reason = match error.reason {
        PeerLinkErrorReason::UnknownLink => PeerLinkThreadReadReason::UnknownLink,
        PeerLinkErrorReason::Expired => PeerLinkThreadReadReason::Expired,
        PeerLinkErrorReason::Incompatible => PeerLinkThreadReadReason::Incompatible,
        PeerLinkErrorReason::Disabled => PeerLinkThreadReadReason::Disabled,
        _ => PeerLinkThreadReadReason::Unreachable,
    };
    PeerLinkThreadReadError {
        reason,
        label: Some(label.to_owned()),
        message: error.message.clone(),
    }
}

/// Maps a peer HTTP refusal status + optional JSON error code into a [`PeerLinkThreadReadError`]
/// (`#39406`, `#39964`).
#[must_use]
pub fn map_relay_http_status(
    status: u16,
    code: Option<&str>,
    label: &str,
) -> PeerLinkThreadReadError {
    map_relay_http_status_for_brand(status, code, label, &ACTIVE_BRAND)
}

/// Brand-parameterized variant of [`map_relay_http_status`].
#[must_use]
pub fn map_relay_http_status_for_brand(
    status: u16,
    code: Option<&str>,
    label: &str,
    brand: &BrandConfig,
) -> PeerLinkThreadReadError {
    if status == 404 && code == Some("thread_not_found") {
        return PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::ThreadNotFound,
            label: Some(label.to_owned()),
            message: format!("That thread is no longer on {label}."),
        };
    }
    if status == 404 {
        return PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::Incompatible,
            label: Some(label.to_owned()),
            message: format!(
                "{label} runs a {} version that does not share its threads over a link. Update it to read its threads from here.",
                brand.product_name
            ),
        };
    }
    if status == 401 || status == 403 {
        let message = if code == Some("linked_environments_off") {
            format!(
                "{label} has Linked environments turned off. Turn it on in its Settings → Labs, or set \"linkedEnvironmentsEnabled\": true in userdata/settings.json in its {} home (~/{} by default).",
                brand.product_name, brand.home_dir_name
            )
        } else {
            format!(
                "{label} refused the read. Its link may have been revoked there; link it again."
            )
        };
        return PeerLinkThreadReadError {
            reason: PeerLinkThreadReadReason::Rejected,
            label: Some(label.to_owned()),
            message,
        };
    }
    PeerLinkThreadReadError {
        reason: PeerLinkThreadReadReason::Unreachable,
        label: Some(label.to_owned()),
        message: format!("{label} did not answer (HTTP {status})."),
    }
}

/// Extracts a renewed `t3-link-expires-at` timestamp from a peer relay HTTP response header.
#[must_use]
pub fn extract_renewed_link_expiry(headers: &HeaderMap) -> Option<String> {
    headers
        .get(LINK_EXPIRES_AT_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Parses SSE `data:` payloads from `raw_sse`, dropping events whose `"kind"` is `"unknown-event"`.
#[must_use]
pub fn parse_sse_stream_items(raw_sse: &str) -> Vec<Value> {
    let mut items = Vec::new();
    let normalized = raw_sse.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let mut data_lines = Vec::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start_matches(' '));
            }
        }
        if data_lines.is_empty() {
            continue;
        }
        let payload = data_lines.join("\n");
        let Ok(value) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        if value.get("kind").and_then(Value::as_str) == Some("unknown-event") {
            continue;
        }
        items.push(value);
    }
    items
}

/// Returns `true` only when a peer rejected `beforePosition` during `thread_read` parameter
/// validation (`#40003`).
#[must_use]
pub fn refuses_before_position(code: &str, message: &str) -> bool {
    code == "invalid_request" && before_position_refusal_regex().is_match(message)
}

/// Injects `beforePosition: NEWEST_PAGE_BEFORE_POSITION` for cursorless agent `thread_read`
/// queries forwarded to linked environments (`#39981`, `#40003`).
#[must_use]
pub fn newest_page_read_args(params: &Value, is_agent_call: bool) -> Option<Value> {
    if !is_agent_call {
        return None;
    }
    let obj = params.as_object()?;
    if obj.contains_key("afterPosition") || obj.contains_key("beforePosition") {
        return None;
    }
    let mut next: Map<String, Value> = obj.clone();
    next.insert(
        "beforePosition".to_owned(),
        Value::from(NEWEST_PAGE_BEFORE_POSITION),
    );
    Some(Value::Object(next))
}

/// Executes a newest-page thread read and falls back once without `beforePosition` if an older peer
/// rejects that argument during schema validation (`#40003`).
pub async fn read_newest_or_retry<T, F, Fut>(
    params: Value,
    is_agent_call: bool,
    mut call: F,
) -> Result<T, (String, String)>
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Result<T, (String, String)>>,
{
    let Some(newest) = newest_page_read_args(&params, is_agent_call) else {
        return call(params).await;
    };
    match call(newest).await {
        Ok(value) => Ok(value),
        Err((code, message)) if refuses_before_position(&code, &message) => call(params).await,
        Err(other) => Err(other),
    }
}

/// Encodes percent-escaped URL query pairs for peer thread relay endpoints.
#[must_use]
pub fn build_relay_route_url(origin: &str, route: &str, params: &[(&str, &str)]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect();
    if query.is_empty() {
        format!("{origin}{route}")
    } else {
        format!("{origin}{route}?{}", query.join("&"))
    }
}

fn percent_encode(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}
