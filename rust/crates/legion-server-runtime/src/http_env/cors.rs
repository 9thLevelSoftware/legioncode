//! Credentialed browser CORS middleware and initial dev/auth environment helpers.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, HOST,
    ORIGIN, VARY,
};
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::http_api::RuntimeHttpState;

pub(crate) const DESKTOP_RENDERER_ORIGINS: &[&str] = &[
    "legioncode://app",
    "legioncode-dev://app",
    "t3://app",
    "t3-dev://app",
];

pub(crate) const CORS_ALLOW_METHODS: &str = "GET, POST, PUT, PATCH, DELETE, OPTIONS";
pub(crate) const CORS_ALLOW_HEADERS: &str =
    "authorization, b3, traceparent, content-type, dpop, x-legioncode-orchestration-protocol";
pub(crate) const CORS_EXPOSE_HEADERS: &str =
    "content-range, accept-ranges, content-disposition, x-legioncode-hook-outcome";

pub(crate) fn normalize_origin(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let (scheme, rest) = trimmed.split_once("://")?;
    let authority = rest.split('/').next()?.trim();
    if authority.is_empty() {
        return None;
    }
    Some(format!("{}://{authority}", scheme.to_ascii_lowercase()))
}

fn is_valid_port_suffix(suffix: &str) -> bool {
    suffix
        .strip_prefix(':')
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn is_loopback_origin(origin: &str) -> bool {
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let Some((ipv6, port_part)) = rest.split_once(']') else {
            return false;
        };
        if !port_part.is_empty() && !is_valid_port_suffix(port_part) {
            return false;
        }
        ipv6
    } else if let Some((h, port_part)) = authority.split_once(':') {
        if !is_valid_port_suffix(&format!(":{port_part}")) {
            return false;
        }
        h
    } else {
        authority
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

pub(crate) fn is_allowed_cors_origin(
    state: &RuntimeHttpState,
    origin: &str,
    host_header: Option<&str>,
) -> bool {
    let Some(norm) = normalize_origin(origin) else {
        return false;
    };
    if is_loopback_origin(&norm) || DESKTOP_RENDERER_ORIGINS.contains(&norm.as_str()) {
        return true;
    }
    if norm == "https://app.legion.codes" {
        return true;
    }
    if let Some(dev_url) = state.dev_url.as_deref().and_then(normalize_origin)
        && norm == dev_url
    {
        return true;
    }
    if state
        .dev_allowed_origins
        .iter()
        .filter_map(|o| normalize_origin(o))
        .any(|o| o == norm)
    {
        return true;
    }
    if let Some(host) = host_header.map(str::trim).filter(|s| !s.is_empty())
        && (norm == format!("http://{host}") || norm == format!("https://{host}"))
    {
        return true;
    }
    false
}

/// Credentialed CORS middleware matching `apps/server/src/http.ts` and `httpCors.ts` (`#41092`).
pub(crate) async fn browser_api_cors_middleware(
    State(state): State<Arc<RuntimeHttpState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let origin = req
        .headers()
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let host = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let allowed = origin
        .as_deref()
        .is_some_and(|o| is_allowed_cors_origin(&state, o, host.as_deref()));

    if req.method() == Method::OPTIONS {
        let mut resp = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::NO_CONTENT.into_response());
        let headers = resp.headers_mut();
        with_vary_origin(headers);
        if origin.is_none() {
            headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
            headers.insert(
                ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static(CORS_ALLOW_METHODS),
            );
            headers.insert(
                ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static(CORS_ALLOW_HEADERS),
            );
            headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        } else if allowed
            && let Some(ref origin_val) = origin
            && let Ok(hv) = HeaderValue::from_str(origin_val)
        {
            headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, hv);
            headers.insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
            headers.insert(
                ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static(CORS_ALLOW_METHODS),
            );
            headers.insert(
                ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static(CORS_ALLOW_HEADERS),
            );
            headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
        }
        return resp;
    }

    let mut resp = next.run(req).await;
    if origin.is_none() {
        let headers = resp.headers_mut();
        with_vary_origin(headers);
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        headers.insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(CORS_EXPOSE_HEADERS),
        );
    } else {
        with_vary_origin(resp.headers_mut());
        if allowed
            && let Some(ref origin_val) = origin
            && let Ok(hv) = HeaderValue::from_str(origin_val)
        {
            let headers = resp.headers_mut();
            headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, hv);
            headers.insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
            headers.insert(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                HeaderValue::from_static(CORS_EXPOSE_HEADERS),
            );
        }
    }
    resp
}

fn with_vary_origin(headers: &mut axum::http::HeaderMap) {
    let existing = headers
        .get(VARY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Some(ref raw) = existing {
        if raw
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("origin"))
        {
            return;
        }
        if let Ok(hv) = HeaderValue::from_str(&format!("{raw}, Origin")) {
            headers.insert(VARY, hv);
        }
    } else {
        headers.insert(VARY, HeaderValue::from_static("Origin"));
    }
}

pub(crate) fn generate_signing_secret() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(Uuid::new_v4().as_bytes());
    hasher.update(Uuid::new_v4().as_bytes());
    let mut out = [0_u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

pub(crate) fn resolve_initial_dev_url() -> Option<String> {
    if let Ok(val) = std::env::var("VITE_DEV_SERVER_URL") {
        let trimmed = val.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let args: Vec<String> = std::env::args().collect();
    for idx in 0..args.len() {
        if args[idx] == "--dev-url"
            && let Some(next) = args.get(idx + 1)
        {
            let trimmed = next.trim().trim_end_matches('/');
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
        if let Some(val) = args[idx].strip_prefix("--dev-url=") {
            let trimmed = val.trim().trim_end_matches('/');
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

pub(crate) fn resolve_dev_allowed_origins() -> Vec<String> {
    std::env::var("LEGIONCODE_DEV_ALLOWED_ORIGINS")
        .ok()
        .map_or_default(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
}

pub(crate) fn resolve_initial_auth_policy() -> String {
    if std::env::var("LEGIONCODE_MODE").is_ok_and(|v| v.trim() == "desktop") {
        return "desktop-managed-local".to_owned();
    }
    let lan = std::env::var("LEGIONCODE_LAN")
        .is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    if lan {
        return "remote-reachable".to_owned();
    }
    if let Ok(host) = std::env::var("LEGIONCODE_HOST").or_else(|_| std::env::var("HOST")) {
        let h = host.trim();
        if !h.is_empty() && !matches!(h, "127.0.0.1" | "localhost" | "::1") {
            return "remote-reachable".to_owned();
        }
    }
    "loopback-browser".to_owned()
}
