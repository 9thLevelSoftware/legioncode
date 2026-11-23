//! Webhook ID/path encoding, constant-time secret validation, and run-reference helpers.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::triggers_hooks::{DELIVERY_HEADERS, HOOK_PREFIX, WebhookRequest};
use crate::types::WorkflowReferenceInput;

/// Computes the collision-free hook ID (`wf-<encWorkflow>.<encName>`), matching `Workflows.ts`.
pub fn hook_id(workflow: &str, name: &str) -> String {
    format!("{HOOK_PREFIX}{}.{}", hook_part(workflow), hook_part(name))
}

/// Computes the relative HTTP path `/api/hooks/<encodedId>/<token>`.
pub fn hook_path(id: &str, token: &str) -> String {
    format!("/api/hooks/{}/{}", encode_uri_component(id), token)
}

fn hook_part(part: &str) -> String {
    encode_uri_component(part).replace('.', "%2E")
}

/// Percent-encodes UTF-8 bytes following ECMAScript `encodeURIComponent` unreserved characters.
pub fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            out.push(char::from(*byte));
        } else {
            out.push('%');
            out.push(hex_upper(byte >> 4));
            out.push(hex_upper(byte & 0x0f));
        }
    }
    out
}

const fn hex_upper(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Compares two strings in constant time over their SHA-256 digests so lengths do not leak.
pub fn constant_time_equals(a: &str, b: &str) -> bool {
    let left = Sha256::digest(a.as_bytes());
    let right = Sha256::digest(b.as_bytes());
    let mut diff = 0_u8;
    for (x, y) in left.iter().zip(right.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Extracts the idempotency key component for an inbound webhook request.
pub fn resolve_webhook_key(
    authored: Option<String>,
    request: &WebhookRequest,
    relay_delivery_id: Option<&str>,
    fallback_uuid: &str,
) -> String {
    if let Some(key) = authored {
        return key;
    }
    for header in DELIVERY_HEADERS {
        if let Some(val) = request.headers.get(*header)
            && !val.is_empty()
        {
            return format!("{header}:{val}");
        }
    }
    if let Some(id) = relay_delivery_id {
        return id.to_owned();
    }
    fallback_uuid.to_owned()
}

/// Builds a [`WorkflowReferenceInput`] from a raw issue or pull-request listing entry JSON.
pub fn reference_from_opened_entry(entry: &Value) -> Option<WorkflowReferenceInput> {
    let obj = entry.as_object()?;
    let project_id = obj.get("projectId")?.as_str()?.to_owned();
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let url = obj.get("url").and_then(Value::as_str).map(str::to_owned);
    if let Some(tracker) = obj.get("tracker").and_then(Value::as_str) {
        let host = obj.get("host").and_then(Value::as_str).map(str::to_owned);
        let scope = obj.get("scope")?.as_str()?.to_owned();
        let key_val = obj.get("key")?.as_str()?.to_owned();
        let state = obj
            .get("state")
            .and_then(|s| {
                s.get("category")
                    .and_then(Value::as_str)
                    .or_else(|| s.as_str())
            })
            .map(str::to_owned);
        let ref_key = serde_json::json!([
            tracker,
            host.as_deref().unwrap_or(""),
            scope.to_lowercase(),
            key_val
        ])
        .to_string();
        let mut address = serde_json::Map::new();
        address.insert("projectId".to_owned(), Value::String(project_id));
        address.insert("tracker".to_owned(), Value::String(tracker.to_owned()));
        if let Some(h) = host {
            address.insert("host".to_owned(), Value::String(h));
        }
        address.insert("scope".to_owned(), Value::String(scope));
        address.insert("key".to_owned(), Value::String(key_val));
        return Some(WorkflowReferenceInput {
            kind: "issue".to_owned(),
            key: ref_key,
            address: Value::Object(address),
            title,
            url,
            state,
        });
    }
    let host = obj.get("host").and_then(Value::as_str)?.to_owned();
    let repository = obj.get("repository").and_then(Value::as_str)?.to_owned();
    let number = obj.get("number").and_then(Value::as_i64)?;
    let state = obj.get("state").and_then(Value::as_str).map(str::to_owned);
    Some(WorkflowReferenceInput {
        kind: "pullRequest".to_owned(),
        key: serde_json::json!([host, repository, number]).to_string(),
        address: serde_json::json!({
            "projectId": project_id,
            "host": host,
            "repository": repository,
            "number": number,
        }),
        title,
        url,
        state,
    })
}
