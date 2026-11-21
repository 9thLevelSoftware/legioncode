//! HTTP custom-scheme surface router (`/__surface/app/{*path}`) backed by `t3_surfaces::desktop`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use t3_surfaces::desktop::{DESKTOP_PRODUCTION_SCHEME, DesktopRouteDecision, DesktopSchemeRouter};

use super::AppState;

const SURFACE_PREFIX: &str = "/__surface/app";

const FALLBACK_SPA_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>T3 Code</title></head><body><div id=\"root\"></div></body></html>";

/// Handles `GET`/`HEAD` requests to `/__surface/app` and `/__surface/app/{*path}` via `DesktopSchemeRouter`.
pub(crate) async fn handle_surface_app(
    State(state): State<Arc<AppState>>,
    method: Method,
    OriginalUri(original_uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let clerk_host = env::var("T3CODE_CLERK_FRONTEND_API_HOSTNAME")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let router = DesktopSchemeRouter::new(DESKTOP_PRODUCTION_SCHEME, clerk_host.as_deref());

    let raw_suffix = strip_surface_prefix(original_uri.path());
    let scheme_url = format!("{DESKTOP_PRODUCTION_SCHEME}://app{raw_suffix}");
    let accept_header = headers.get("accept").and_then(|val| val.to_str().ok());
    let surface_dir = resolve_surface_dir(&state);

    let decision = router.route_with(method.as_str(), &scheme_url, accept_header, |rel| {
        surface_dir
            .as_deref()
            .and_then(|root| safe_asset_path(root, rel))
            .is_some_and(|p| p.is_file())
    });

    match decision {
        DesktopRouteDecision::ServeAsset {
            relative_path,
            mime_type,
            content_security_policy,
            head_only,
        } => {
            if let Some(root) = surface_dir.as_deref()
                && let Some(file_path) = safe_asset_path(root, &relative_path)
                && let Ok(bytes) = fs::read(&file_path)
            {
                return build_asset_response(
                    StatusCode::OK,
                    mime_type,
                    &content_security_policy,
                    if head_only { Vec::new() } else { bytes },
                );
            }
            build_rejected_response(StatusCode::NOT_FOUND, "not_found", &content_security_policy)
        }
        DesktopRouteDecision::FallbackSpaIndex {
            relative_path,
            mime_type,
            content_security_policy,
            head_only,
        } => {
            let bytes = surface_dir
                .as_deref()
                .and_then(|root| safe_asset_path(root, relative_path))
                .and_then(|file_path| fs::read(&file_path).ok())
                .unwrap_or_else(|| FALLBACK_SPA_HTML.as_bytes().to_vec());
            build_asset_response(
                StatusCode::OK,
                mime_type,
                &content_security_policy,
                if head_only { Vec::new() } else { bytes },
            )
        }
        DesktopRouteDecision::Rejected {
            status,
            reason,
            content_security_policy,
        } => {
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
            let reason_str = serde_json::to_value(reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "rejected".to_owned());
            build_rejected_response(code, &reason_str, &content_security_policy)
        }
    }
}

fn strip_surface_prefix(path: &str) -> &str {
    match path.strip_prefix(SURFACE_PREFIX) {
        Some("") | None => "/",
        Some(rest) if rest.starts_with('/') => rest,
        Some(_) => "/",
    }
}

fn resolve_surface_dir(state: &AppState) -> Option<PathBuf> {
    if let Ok(raw) = env::var("T3CODE_SURFACE_APP_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let dir = PathBuf::from(trimmed);
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    if let Some(fallback) = state
        .paths
        .state_dir
        .parent()
        .map(|b| b.join("surface-app"))
        && fallback.is_dir()
    {
        return Some(fallback);
    }
    None
}

fn safe_asset_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut joined = root.to_path_buf();
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains('\\') {
            return None;
        }
        joined.push(segment);
    }
    Some(joined)
}

fn build_asset_response(status: StatusCode, mime_type: &str, csp: &str, body: Vec<u8>) -> Response {
    let mut builder = Response::builder().status(status);
    if let Ok(ct) = HeaderValue::from_str(mime_type) {
        builder = builder.header(CONTENT_TYPE, ct);
    }
    if let Ok(csp_val) = HeaderValue::from_str(csp) {
        builder = builder.header(CONTENT_SECURITY_POLICY, csp_val);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn build_rejected_response(status: StatusCode, reason: &str, csp: &str) -> Response {
    let body = serde_json::to_vec(&json!({ "error": reason })).unwrap_or_default();
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8");
    if let Ok(csp_val) = HeaderValue::from_str(csp) {
        builder = builder.header(CONTENT_SECURITY_POLICY, csp_val);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| status.into_response())
}
