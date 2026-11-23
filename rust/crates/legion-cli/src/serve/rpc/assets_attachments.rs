//! Signed asset URLs (`assets.createUrl`, `assets.persistChatAttachments`) and
//! attachment upload URLs (`attachments.createUploadUrl`, `attachments.delete`).

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use legion_persistence::paths::sweep_stale_pending_attachments;
use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::assets_paths::resolve_target_for_resource;
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj};

const ASSET_TOKEN_TTL_MS: i64 = 60 * 60 * 1000;
const UPLOAD_TOKEN_TTL_MS: i64 = 15 * 60 * 1000;

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "assets.createUrl" => create_asset_url(state, payload),
        "assets.persistChatAttachments" => persist_chat_attachments(state, payload),
        "attachments.createUploadUrl" => create_upload_url(state, payload),
        "attachments.delete" => delete_attachment(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetCreateUrlPayload {
    resource: Value,
}

fn create_asset_url(state: &AppState, payload: Value) -> RpcExit {
    let req: AssetCreateUrlPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let (target_path, display_name) = match resolve_target_for_resource(state, &req.resource) {
        Ok(pair) => pair,
        Err(e) => return e,
    };
    let as_attachment = req.resource.get("disposition").and_then(Value::as_str)
        == Some("attachment")
        && matches!(
            req.resource.get("_tag").and_then(Value::as_str),
            Some("media-file" | "draft-workspace-file")
        );
    let signed = match state.runtime_http.create_signed_asset_url_with_disposition(
        &target_path,
        display_name.as_deref(),
        Some(ASSET_TOKEN_TTL_MS),
        as_attachment,
    ) {
        Ok(url) => url,
        Err(err) => {
            return rpc_fail_obj(json!({
                "_tag": "AssetWorkspaceContextResolutionError",
                "resource": req.resource,
                "cause": err.to_string(),
            }));
        }
    };
    RpcExit::Success {
        value: json!({
            "relativeUrl": signed.relative_url,
            "expiresAt": signed.expires_at,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistChatAttachmentsPayload {
    thread_id: String,
    #[serde(default)]
    attachments: Vec<Value>,
}

fn persist_chat_attachments(state: &AppState, payload: Value) -> RpcExit {
    let req: PersistChatAttachmentsPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Err(err) = fs::create_dir_all(&state.paths.attachments_dir) {
        return rpc_fail("AttachmentPersistError", &err.to_string());
    }
    let thread_seg = sanitize_thread_segment(&req.thread_id);
    let mut persisted = Vec::with_capacity(req.attachments.len());
    for att in req.attachments {
        let Some(mut obj) = att.as_object().cloned() else {
            continue;
        };
        if obj
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            obj.insert(
                "id".to_owned(),
                json!(format!("{thread_seg}-{}", Uuid::new_v4())),
            );
        }
        if let Some(data_url) = obj.get("dataUrl").and_then(Value::as_str)
            && let Some(id) = obj.get("id").and_then(Value::as_str)
            && !id.contains('/')
            && !id.contains('\\')
            && !id.contains("..")
        {
            let _ = write_data_url_if_base64(&state.paths.attachments_dir, id, data_url);
        }
        persisted.push(Value::Object(obj));
    }
    RpcExit::Success {
        value: json!({ "attachments": persisted }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUploadUrlPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    name: String,
    size_bytes: u64,
}

fn create_upload_url(state: &AppState, payload: Value) -> RpcExit {
    let req: CreateUploadUrlPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let _ = fs::create_dir_all(&state.paths.attachments_dir);
    let _ = sweep_stale_pending_attachments(&state.paths.attachments_dir, SystemTime::now());
    let attachment_type = req.kind.as_deref().unwrap_or("image");
    let ext = Path::new(&req.name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| !e.is_empty() && e != "part" && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".to_owned());
    let attachment_id = if attachment_type == "file" {
        format!("pending-{}-{ext}", Uuid::new_v4())
    } else {
        format!("pending-{}", Uuid::new_v4())
    };
    let signed = match state.runtime_http.create_attachment_upload_url(
        &attachment_id,
        Some(req.size_bytes),
        Some(UPLOAD_TOKEN_TTL_MS),
    ) {
        Ok(url) => url,
        Err(err) => {
            return rpc_fail_obj(json!({
                "_tag": "AttachmentUploadSigningKeyError",
                "cause": err.to_string(),
            }));
        }
    };
    RpcExit::Success {
        value: json!({
            "attachmentId": attachment_id,
            "relativeUrl": signed.relative_url,
            "expiresAt": signed.expires_at,
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteAttachmentPayload {
    attachment_id: String,
}

fn delete_attachment(state: &AppState, payload: Value) -> RpcExit {
    let req: DeleteAttachmentPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let id = req.attachment_id.trim();
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return rpc_fail("InvalidAttachmentId", "Invalid attachment ID.");
    }
    if let Ok(entries) = fs::read_dir(&state.paths.attachments_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == id || name_str.starts_with(&format!("{id}.")) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    RpcExit::Success { value: Value::Null }
}

fn sanitize_thread_segment(thread_id: &str) -> String {
    let cleaned: String = thread_id
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['-', '_']);
    if trimmed.is_empty() || trimmed == "pending" {
        "_pending".to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn write_data_url_if_base64(dir: &Path, id: &str, data_url: &str) -> Option<()> {
    let (_, b64) = data_url.split_once(";base64,")?;
    let bytes = decode_standard_base64(b64)?;
    let path = dir.join(format!("{id}.bin"));
    fs::write(path, bytes).ok()
}

fn decode_standard_base64(input: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return None;
    }
    let val = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for chunk in clean.as_chunks::<4>().0 {
        let c0 = val(chunk[0])?;
        let c1 = val(chunk[1])?;
        out.push((c0 << 2) | (c1 >> 4));
        if chunk[2] != b'=' {
            let c2 = val(chunk[2])?;
            out.push((c1 << 4) | (c2 >> 2));
            if chunk[3] != b'=' {
                let c3 = val(chunk[3])?;
                out.push((c2 << 6) | c3);
            }
        }
    }
    Some(out)
}
