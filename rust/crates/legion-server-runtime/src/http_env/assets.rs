//! Signed assets (`/api/assets/{*tail}`), signed attachment uploads (`/api/attachments/{*token}`),
//! and API 404 fallback.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::error::ServerRuntimeError;
use crate::http_api::RuntimeHttpState;
use crate::http_auth::now_unix_ms;
use crate::http_env::{
    env_auth_invalid, env_bad_request, env_error_json, env_forbidden, env_internal, env_not_found,
    sign_b64url_payload, verify_b64url_payload,
};

const DEFAULT_SIGNED_ASSET_TTL_MS: i64 = 15 * 60 * 1_000;
const DEFAULT_MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;

/// Relative signed URL paired with its expiration epoch timestamp (milliseconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedHttpUrl {
    /// Server-relative signed URL (`/api/assets/...` or `/api/attachments/upload/...`).
    pub relative_url: String,
    /// Expiration timestamp in Unix epoch milliseconds.
    pub expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SignedAssetClaims {
    base_dir: String,
    rel_path: String,
    exp_ms: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SignedAttachmentUploadClaims {
    attachment_id: String,
    max_bytes: u64,
    exp_ms: i64,
}

/// Builds a signed `/api/assets/<token>/<filename>` relative URL for `path`.
pub(crate) fn build_signed_asset_url(
    secret: &[u8],
    path: &Path,
    filename: Option<&str>,
    ttl_ms: Option<i64>,
) -> Result<SignedHttpUrl, ServerRuntimeError> {
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_part = path.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        ServerRuntimeError::BadRequest("Asset path must have a file name".to_owned())
    })?;
    let ttl = ttl_ms.unwrap_or(DEFAULT_SIGNED_ASSET_TTL_MS).max(1_000);
    let expires_at = now_unix_ms().saturating_add(ttl);
    let claims = SignedAssetClaims {
        base_dir: base.to_string_lossy().into_owned(),
        rel_path: file_part.to_owned(),
        exp_ms: expires_at,
    };
    let bytes = serde_json::to_vec(&claims).map_err(|e| {
        ServerRuntimeError::Persistence(format!("asset claim serialize error: {e}"))
    })?;
    let token = sign_b64url_payload(secret, &bytes);
    let display_name = filename.unwrap_or(file_part).replace('/', "_");
    Ok(SignedHttpUrl {
        relative_url: format!("/api/assets/{token}/{display_name}"),
        expires_at,
    })
}

/// Builds a signed `/api/attachments/upload/<token>` relative URL for `attachment_id`.
pub(crate) fn build_signed_attachment_upload_url(
    secret: &[u8],
    attachment_id: &str,
    max_bytes: Option<u64>,
    ttl_ms: Option<i64>,
) -> Result<SignedHttpUrl, ServerRuntimeError> {
    let trimmed = attachment_id.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
    {
        return Err(ServerRuntimeError::BadRequest(
            "Invalid attachmentId for signed upload".to_owned(),
        ));
    }
    let ttl = ttl_ms.unwrap_or(DEFAULT_SIGNED_ASSET_TTL_MS).max(1_000);
    let expires_at = now_unix_ms().saturating_add(ttl);
    let claims = SignedAttachmentUploadClaims {
        attachment_id: trimmed.to_owned(),
        max_bytes: max_bytes.unwrap_or(DEFAULT_MAX_UPLOAD_BYTES),
        exp_ms: expires_at,
    };
    let bytes = serde_json::to_vec(&claims).map_err(|e| {
        ServerRuntimeError::Persistence(format!("upload claim serialize error: {e}"))
    })?;
    let token = sign_b64url_payload(secret, &bytes);
    Ok(SignedHttpUrl {
        relative_url: format!("/api/attachments/upload/{token}"),
        expires_at,
    })
}

fn has_unsafe_rel_segments(rel: &str) -> bool {
    let decoded = percent_decode_path(rel);
    if decoded.contains('\0') {
        return true;
    }
    Path::new(&decoded).components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

fn percent_decode_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%'
            && idx + 2 < bytes.len()
            && let (Some(h), Some(l)) = (
                (bytes[idx + 1] as char).to_digit(16),
                (bytes[idx + 2] as char).to_digit(16),
            )
        {
            out.push(u8::try_from((h << 4) | l).unwrap_or(0));
            idx += 3;
            continue;
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn guess_mime_from_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("pdf") => "application/pdf",
        Some("txt" | "log" | "md") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("mp4") => "video/mp4",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

fn parse_byte_range(header: &str, total_len: usize) -> Option<(usize, usize)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || total_len == 0 {
        return None;
    }
    let (start_str, end_str) = spec.split_once('-')?;
    if start_str.is_empty() {
        let suffix = end_str.trim().parse::<usize>().ok()?.min(total_len);
        if suffix == 0 {
            return None;
        }
        return Some((total_len - suffix, total_len - 1));
    }
    let start = start_str.trim().parse::<usize>().ok()?;
    let end = if end_str.trim().is_empty() {
        total_len - 1
    } else {
        end_str.trim().parse::<usize>().ok()?.min(total_len - 1)
    };
    (start <= end && start < total_len).then_some((start, end))
}

/// `GET|HEAD /api/assets/{*tail}` — verifies signed asset tokens and serves files with 206 Range.
pub(crate) async fn handle_signed_asset(
    State(state): State<Arc<RuntimeHttpState>>,
    method: Method,
    AxumPath(tail): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let (token, sub_opt) = match tail.split_once('/') {
        Some((tok, rest)) => (tok, Some(rest)),
        None => (tail.as_str(), None),
    };
    let Some(claims_bytes) = verify_b64url_payload(&state.signed_asset_secret, token) else {
        return env_auth_invalid("invalid_credential", "Invalid signed asset token");
    };
    let Ok(claims) = serde_json::from_slice::<SignedAssetClaims>(&claims_bytes) else {
        return env_bad_request("Malformed signed asset claims");
    };
    if claims.exp_ms <= now_unix_ms() {
        return env_auth_invalid("invalid_credential", "Signed asset URL expired");
    }
    if let Some(sub) = sub_opt
        && has_unsafe_rel_segments(sub)
    {
        return env_bad_request("Invalid asset subpath");
    }

    let Ok(canon_base) = fs::canonicalize(Path::new(&claims.base_dir)) else {
        return env_not_found("Asset root directory not found");
    };
    let Ok(canon_target) = fs::canonicalize(canon_base.join(&claims.rel_path)) else {
        return env_not_found("Asset file not found");
    };
    if !canon_target.starts_with(&canon_base) || !canon_target.is_file() {
        return env_forbidden("Asset access denied");
    }

    let Ok(bytes) = fs::read(&canon_target) else {
        return env_not_found("Failed to read asset file");
    };
    let total_len = bytes.len();
    let mime = guess_mime_from_path(&canon_target);
    let etag = format!("\"{}\"", hex::encode(&Sha256::digest(&bytes)[..12]));
    let is_head = method == Method::HEAD;

    if let Some(range_hdr) = headers.get(RANGE).and_then(|v| v.to_str().ok()) {
        let Some((start, end)) = parse_byte_range(range_hdr, total_len) else {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(hv) = HeaderValue::from_str(&format!("bytes */{total_len}")) {
                resp.headers_mut().insert(CONTENT_RANGE, hv);
            }
            return resp;
        };
        let slice_len = end - start + 1;
        let body_bytes = if is_head {
            Vec::new()
        } else {
            bytes[start..=end].to_vec()
        };
        let mut resp = (StatusCode::PARTIAL_CONTENT, body_bytes).into_response();
        let h = resp.headers_mut();
        h.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
        h.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        if let Ok(hv) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total_len}")) {
            h.insert(CONTENT_RANGE, hv);
        }
        if let Ok(hv) = HeaderValue::from_str(&slice_len.to_string()) {
            h.insert(CONTENT_LENGTH, hv);
        }
        if let Ok(hv) = HeaderValue::from_str(&etag) {
            h.insert(ETAG, hv);
        }
        return resp;
    }

    let body_bytes = if is_head { Vec::new() } else { bytes };
    let mut resp = (StatusCode::OK, body_bytes).into_response();
    let h = resp.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(hv) = HeaderValue::from_str(&total_len.to_string()) {
        h.insert(CONTENT_LENGTH, hv);
    }
    if let Ok(hv) = HeaderValue::from_str(&etag) {
        h.insert(ETAG, hv);
    }
    resp
}

/// `POST /api/attachments/{*tail}` — verifies signed attachment upload token and stores file.
pub(crate) async fn handle_signed_attachment_upload(
    State(state): State<Arc<RuntimeHttpState>>,
    AxumPath(tail): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = tail.strip_prefix("upload/").unwrap_or(&tail);
    let Some(claims_bytes) = verify_b64url_payload(&state.signed_asset_secret, token) else {
        return env_auth_invalid(
            "invalid_credential",
            "Invalid signed attachment upload token",
        );
    };
    let Ok(claims) = serde_json::from_slice::<SignedAttachmentUploadClaims>(&claims_bytes) else {
        return env_bad_request("Malformed attachment upload claims");
    };
    if claims.exp_ms <= now_unix_ms() {
        return env_auth_invalid(
            "invalid_credential",
            "Signed attachment upload token expired",
        );
    }
    if body.is_empty() {
        return env_bad_request("Attachment upload body cannot be empty");
    }
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > claims.max_bytes {
        return env_error_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({
                "_tag": "EnvironmentHttpBadRequestError",
                "message": "Attachment exceeds maximum upload size",
            }),
        );
    }

    if let Err(err) = fs::create_dir_all(&state.attachments_dir) {
        return env_internal(format!("Failed to create attachments dir: {err}"));
    }
    let dest: PathBuf = state.attachments_dir.join(&claims.attachment_id);
    if let Err(err) = fs::write(&dest, &body) {
        return env_internal(format!("Failed to write attachment: {err}"));
    }

    let mime_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    let signed_read = build_signed_asset_url(&state.signed_asset_secret, &dest, None, None).ok();
    Json(json!({
        "attachmentId": claims.attachment_id,
        "sizeBytes": body.len(),
        "mimeType": mime_type,
        "relativeUrl": signed_read.map(|u| u.relative_url),
    }))
    .into_response()
}

/// JSON 404 fallback for unknown `/api/*` and `/oauth/*` paths so SPA `index.html` is never served.
pub(crate) async fn handle_api_not_found() -> Response {
    env_not_found("Not Found")
}
