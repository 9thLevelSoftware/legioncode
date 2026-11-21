//! Static asset fallback router mirroring `apps/server/src/http.ts:616-774`.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{
    ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, ETAG, IF_NONE_MATCH, LAST_MODIFIED, RANGE, VARY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::Response;

use crate::static_assets_http::{
    HTML_CSP, ParsedByteRange, SVG_CSP, build_weak_etag, content_type_for_path,
    decode_and_validate_url_path, format_http_date_unix_secs, if_none_match_matches,
    is_workspace_preview_entry_path, load_manifest_assets, parse_byte_range,
    resolve_previewable_workspace_asset, system_time_unix_secs,
};

#[derive(Clone)]
struct StaticAssetState {
    static_dir: Option<PathBuf>,
    manifest_assets: HashSet<String>,
}

/// Build an Axum fallback [`Router`] serving built static web assets with SPA fallback,
/// `ETag`/`304`, HTTP byte `Range`/`206`/`416`, traversal rejection (`400`), and frame/CSP headers.
pub fn static_assets_router(static_dir: Option<PathBuf>) -> Router {
    let manifest_assets = static_dir
        .as_deref()
        .map(load_manifest_assets)
        .unwrap_or_default();
    let state = Arc::new(StaticAssetState {
        static_dir,
        manifest_assets,
    });
    Router::new()
        .fallback(handle_static_asset)
        .with_state(state)
}

async fn handle_static_asset(
    State(state): State<Arc<StaticAssetState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return text_response_with_header(
            StatusCode::METHOD_NOT_ALLOWED,
            "Method not allowed",
            ALLOW,
            "GET, HEAD",
        );
    }
    let Some(ref configured_dir) = state.static_dir else {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "No static directory configured",
        );
    };
    let Ok(canonical_root) = std::fs::canonicalize(configured_dir) else {
        return text_response(StatusCode::NOT_FOUND, "Static directory not found");
    };

    let Ok(decoded) = decode_and_validate_url_path(uri.path()) else {
        return text_response(StatusCode::BAD_REQUEST, "Invalid static file path");
    };
    let normalized = if decoded == "/" {
        "index.html".to_owned()
    } else {
        decoded.trim_start_matches('/').to_owned()
    };
    if normalized.is_empty() {
        return serve_spa_index(&canonical_root, method == Method::HEAD);
    }

    let candidate = resolve_candidate_path(&canonical_root, &normalized);
    let is_symlink =
        std::fs::symlink_metadata(&candidate).is_ok_and(|meta| meta.file_type().is_symlink());
    match std::fs::canonicalize(&candidate) {
        Ok(canonical_file) => {
            if !canonical_file.starts_with(&canonical_root) {
                return text_response(StatusCode::BAD_REQUEST, "Invalid static file path");
            }
            if is_symlink
                && !canonical_file.is_dir()
                && !is_workspace_preview_entry_path(&canonical_file)
            {
                return text_response(StatusCode::FORBIDDEN, "Symlink target is not previewable");
            }
            if canonical_file.is_dir() {
                return serve_spa_index(&canonical_root, method == Method::HEAD);
            }
            let rel = canonical_file
                .strip_prefix(&canonical_root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel == "index.html" {
                return serve_spa_index(&canonical_root, method == Method::HEAD);
            }
            serve_static_file(
                &canonical_file,
                &rel,
                &state.manifest_assets,
                method == Method::HEAD,
                &headers,
            )
        }
        Err(_) => {
            // Check whether normalized still contains non-existent path inside valid root vs traversal
            serve_spa_index(&canonical_root, method == Method::HEAD)
        }
    }
}

fn resolve_candidate_path(canonical_root: &Path, normalized_rel: &str) -> PathBuf {
    let mut joined = canonical_root.to_path_buf();
    for segment in normalized_rel.split('/').filter(|s| !s.is_empty()) {
        joined.push(segment);
    }
    if Path::new(normalized_rel).extension().is_none() {
        let dir_index = joined.join("index.html");
        if dir_index.is_file() {
            return dir_index;
        }
    }
    joined
}

fn serve_spa_index(canonical_root: &Path, is_head: bool) -> Response {
    let index_path = canonical_root.join("index.html");
    let Ok(bytes) = std::fs::read(&index_path) else {
        return text_response(StatusCode::NOT_FOUND, "Not found");
    };
    let body = if is_head {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::OK;
    let h = resp.headers_mut();
    insert_hdr(h, CONTENT_TYPE, "text/html; charset=utf-8");
    insert_hdr(h, CACHE_CONTROL, "no-cache");
    insert_hdr(h, HeaderName::from_static("x-frame-options"), "DENY");
    insert_hdr(h, CONTENT_SECURITY_POLICY, "frame-ancestors 'none'");
    resp
}

fn serve_static_file(
    file_path: &Path,
    relative_path: &str,
    manifest_assets: &HashSet<String>,
    is_head: bool,
    req_headers: &HeaderMap,
) -> Response {
    let Ok(mut file) = File::open(file_path) else {
        return text_response(StatusCode::NOT_FOUND, "Not found");
    };
    let Ok(meta) = file.metadata() else {
        return text_response(StatusCode::NOT_FOUND, "Not found");
    };
    let size = meta.len();
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let content_type = content_type_for_path(file_path);
    let is_html = content_type.starts_with("text/html");
    let is_svg = content_type.starts_with("image/svg+xml");

    if is_html {
        let body = if is_head {
            Body::empty()
        } else {
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_err() {
                return text_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file");
            }
            Body::from(buf)
        };
        let mut resp = Response::new(body);
        *resp.status_mut() = StatusCode::OK;
        let h = resp.headers_mut();
        insert_hdr(h, CONTENT_TYPE, content_type);
        insert_hdr(h, CACHE_CONTROL, "no-cache");
        insert_hdr(h, CONTENT_SECURITY_POLICY, HTML_CSP);
        return resp;
    }

    let etag = build_weak_etag(size, mtime);
    let last_modified = format_http_date_unix_secs(system_time_unix_secs(mtime));
    let cache_control = if manifest_assets.contains(relative_path) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    if let Some(inm) = req_headers.get(IF_NONE_MATCH).and_then(|v| v.to_str().ok())
        && if_none_match_matches(inm, &etag)
    {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::NOT_MODIFIED;
        let h = resp.headers_mut();
        insert_hdr(h, ETAG, &etag);
        insert_hdr(h, LAST_MODIFIED, &last_modified);
        insert_hdr(h, CACHE_CONTROL, cache_control);
        insert_hdr(h, VARY, "Accept-Encoding");
        return resp;
    }

    let range_hdr = req_headers.get(RANGE).and_then(|v| v.to_str().ok());
    let range = parse_byte_range(range_hdr, size);
    if matches!(range, ParsedByteRange::Unsatisfiable) {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
        let h = resp.headers_mut();
        insert_hdr(h, CONTENT_RANGE, &format!("bytes */{size}"));
        insert_hdr(h, ACCEPT_RANGES, "bytes");
        return resp;
    }

    let (status, slice_first, slice_len, content_range_hdr) = match range {
        ParsedByteRange::Satisfiable { first, last } => (
            StatusCode::PARTIAL_CONTENT,
            first,
            last - first + 1,
            Some(format!("bytes {first}-{last}/{size}")),
        ),
        ParsedByteRange::None | ParsedByteRange::Unsatisfiable => (StatusCode::OK, 0, size, None),
    };

    let body = if is_head {
        Body::empty()
    } else {
        let Ok(cap) = usize::try_from(slice_len) else {
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "Range too large");
        };
        let mut buf = vec![0_u8; cap];
        if file.seek(SeekFrom::Start(slice_first)).is_err() || file.read_exact(&mut buf).is_err() {
            return text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read file range",
            );
        }
        Body::from(buf)
    };

    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    insert_hdr(h, CONTENT_TYPE, content_type);
    insert_hdr(h, CONTENT_LENGTH, &slice_len.to_string());
    insert_hdr(h, ACCEPT_RANGES, "bytes");
    insert_hdr(h, ETAG, &etag);
    insert_hdr(h, LAST_MODIFIED, &last_modified);
    insert_hdr(h, CACHE_CONTROL, cache_control);
    insert_hdr(h, X_CONTENT_TYPE_OPTIONS, "nosniff");
    if is_svg {
        insert_hdr(h, CONTENT_SECURITY_POLICY, SVG_CSP);
    }
    if let Some(cr) = content_range_hdr {
        insert_hdr(h, CONTENT_RANGE, &cr);
    }
    resp
}

fn insert_hdr(headers: &mut HeaderMap, name: HeaderName, val: &str) {
    if let Ok(parsed) = HeaderValue::from_str(val) {
        headers.insert(name, parsed);
    }
}

fn text_response(status: StatusCode, body: &str) -> Response {
    let mut resp = Response::new(Body::from(body.to_owned()));
    *resp.status_mut() = status;
    insert_hdr(
        resp.headers_mut(),
        CONTENT_TYPE,
        "text/plain; charset=utf-8",
    );
    resp
}

fn text_response_with_header(
    status: StatusCode,
    body: &str,
    name: HeaderName,
    value: &str,
) -> Response {
    let mut resp = text_response(status, body);
    insert_hdr(resp.headers_mut(), name, value);
    resp
}

/// Validates and serves a workspace asset path (following symlinks) returning `403` when missing,
/// outside `allowed_roots`, or pointing to a non-previewable entry type (`#39491`).
pub fn serve_workspace_asset_file(
    candidate: &Path,
    allowed_roots: &[PathBuf],
    is_head: bool,
    req_headers: &HeaderMap,
) -> Response {
    let Some(canonical_target) = resolve_previewable_workspace_asset(candidate, allowed_roots)
    else {
        return text_response(
            StatusCode::FORBIDDEN,
            "Workspace asset target is missing, outside allowed roots, or not previewable",
        );
    };
    let rel = canonical_target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    serve_static_file(
        &canonical_target,
        rel,
        &HashSet::new(),
        is_head,
        req_headers,
    )
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceAssetQuery {
    pub path: Option<String>,
}

/// `GET /api/assets/workspace?path=...` — validates canonical target inside `allowed_workspace_roots`
/// and checks [`is_workspace_preview_entry_path`] (`#39491`).
pub(crate) async fn handle_workspace_asset(
    State(state): State<Arc<crate::http_api::RuntimeHttpState>>,
    headers: HeaderMap,
    method: Method,
    axum::extract::Query(query): axum::extract::Query<WorkspaceAssetQuery>,
) -> Result<Response, crate::error::ServerRuntimeError> {
    let _principal = crate::http_auth_guard::authorize_request(
        &state,
        &headers,
        &[t3_contracts::permissions::EnvironmentScope::FilesystemRead],
        false,
    )?;
    let Some(raw_path) = query.path.filter(|s| !s.trim().is_empty()) else {
        return Err(crate::error::ServerRuntimeError::BadRequest(
            "path query parameter is required".to_owned(),
        ));
    };
    let candidate = PathBuf::from(&raw_path);
    Ok(serve_workspace_asset_file(
        &candidate,
        &state.allowed_workspace_roots,
        method == Method::HEAD,
        &headers,
    ))
}
