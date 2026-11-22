//! Signed asset URLs (`assets.createUrl`, `assets.persistChatAttachments`) and
//! attachment upload URLs (`attachments.createUploadUrl`, `attachments.delete`).

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use legion_persistence::paths::sweep_stale_pending_attachments;
use legion_protocol::rpc_codec::RpcExit;
use legion_remote::secrets::FileSecretStore;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, base64url_encode, hmac_sha256, parse_payload, rpc_fail, rpc_fail_obj};

const ASSET_TOKEN_TTL_MS: u64 = 60 * 60 * 1000;
const UPLOAD_TOKEN_TTL_MS: u64 = 15 * 60 * 1000;

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

fn load_or_create_signing_key(state: &AppState) -> Result<Vec<u8>, String> {
    let store = FileSecretStore::new(&state.paths.secrets_dir).map_err(|e| e.to_string())?;
    let key_name = "asset-access-signing-key";
    if let Some(existing) = store.get(key_name).map_err(|e| e.to_string())?
        && !existing.is_empty()
    {
        return Ok(existing);
    }
    let key = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()).into_bytes();
    store.set(key_name, &key).map_err(|e| e.to_string())?;
    Ok(key)
}

fn now_millis() -> u64 {
    let raw = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(raw).unwrap_or(u64::MAX)
}

fn sign_claims(state: &AppState, claims: &Value) -> Result<String, String> {
    let key = load_or_create_signing_key(state)?;
    let encoded_payload = base64url_encode(claims.to_string().as_bytes());
    let mac = hmac_sha256(&key, encoded_payload.as_bytes());
    let sig = base64url_encode(&mac);
    Ok(format!("{encoded_payload}.{sig}"))
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
    let expires_at = now_millis().saturating_add(ASSET_TOKEN_TTL_MS);
    let mut claims = match req.resource.as_object() {
        Some(obj) => obj.clone(),
        None => {
            return rpc_fail_obj(json!({
                "_tag": "AssetSigningKeyLoadError",
                "message": "Invalid asset resource payload.",
            }));
        }
    };
    claims.insert("version".to_owned(), json!(1));
    claims.insert("expiresAt".to_owned(), json!(expires_at));
    let token = match sign_claims(state, &Value::Object(claims)) {
        Ok(t) => t,
        Err(msg) => {
            return rpc_fail_obj(json!({
                "_tag": "AssetSigningKeyLoadError",
                "message": msg,
            }));
        }
    };
    RpcExit::Success {
        value: json!({
            "relativeUrl": format!("/api/assets/{token}"),
            "expiresAt": expires_at,
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
    mime_type: String,
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
    let expires_at = now_millis().saturating_add(UPLOAD_TOKEN_TTL_MS);
    let claims = json!({
        "version": 1,
        "kind": "attachment-upload",
        "type": attachment_type,
        "attachmentId": attachment_id,
        "name": req.name,
        "mimeType": req.mime_type,
        "sizeBytes": req.size_bytes,
        "expiresAt": expires_at,
    });
    let token = match sign_claims(state, &claims) {
        Ok(t) => t,
        Err(msg) => {
            return rpc_fail_obj(json!({
                "_tag": "AttachmentUploadSigningKeyError",
                "message": msg,
            }));
        }
    };
    RpcExit::Success {
        value: json!({
            "attachmentId": attachment_id,
            "relativeUrl": format!("/api/attachments/upload/{token}"),
            "expiresAt": expires_at,
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
    RpcExit::Success { value: json!({}) }
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
