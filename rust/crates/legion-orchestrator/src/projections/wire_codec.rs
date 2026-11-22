//! Wire projection transformation helpers: base64url, cursor codec, and `WireProjection.ts` payload capping.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::projections::ProjectionError;

const MAX_WIRE_CHECKPOINT_FILES: usize = 200;
const MAX_DETAIL_STRING_BYTES: usize = 32_768;
const MAX_DYNAMIC_VALUE_BYTES: usize = 16_384;
const CURSOR_MAX_LEN: usize = 4_096;
const TRUNCATION_MARKER: &str = "\n… output truncated for transport";
const B64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Decoded pagination cursor matching `ThreadHistoryCursorPayload` in `threadHistoryPaging.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadHistoryCursorPayload {
    /// Format version (`1`).
    pub v: u8,
    /// Snapshot sequence captured with the cursor.
    pub seq: i64,
    /// Anchor item's owning source thread ID.
    pub st: String,
    /// Anchor item ID on `st`.
    pub si: String,
    /// Zero-based position at the time the cursor was emitted.
    pub p: usize,
    /// When `true`, include the anchor item in the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusive: Option<bool>,
}

/// Encodes `bytes` into unpadded RFC 4648 base64url.
pub(crate) fn encode_base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64URL_ALPHABET[(triple >> 18) & 0x3f] as char);
        out.push(B64URL_ALPHABET[(triple >> 12) & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(B64URL_ALPHABET[(triple >> 6) & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(B64URL_ALPHABET[triple & 0x3f] as char);
        }
    }
    out
}

/// Decodes unpadded (or padded) base64url into raw bytes.
pub(crate) fn decode_base64url(input: &str) -> Result<Vec<u8>, String> {
    let trimmed = input.trim_end_matches('=');
    let mut out = Vec::with_capacity((trimmed.len() * 3) / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for byte in trimmed.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err("invalid base64url character".to_owned()),
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    if bits > 0 && (buf & ((1 << bits) - 1)) != 0 {
        return Err("non-zero trailing bits in base64url".to_owned());
    }
    Ok(out)
}

/// Encodes a thread history pagination cursor identically to `encodeThreadHistoryCursor`.
pub(crate) fn encode_history_cursor(payload: &ThreadHistoryCursorPayload) -> String {
    let raw = serde_json::to_vec(payload).unwrap_or_default();
    encode_base64url(&raw)
}

/// Decodes and validates a thread history pagination cursor.
pub(crate) fn decode_history_cursor(
    cursor: &str,
) -> Result<ThreadHistoryCursorPayload, ProjectionError> {
    if cursor.is_empty() || cursor.len() > CURSOR_MAX_LEN {
        return Err(ProjectionError::InvalidCursor);
    }
    let bytes = decode_base64url(cursor).map_err(|_| ProjectionError::InvalidCursor)?;
    let parsed: ThreadHistoryCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| ProjectionError::InvalidCursor)?;
    if parsed.v != 1 || parsed.seq < 0 || parsed.st.is_empty() || parsed.si.is_empty() {
        return Err(ProjectionError::InvalidCursor);
    }
    Ok(parsed)
}

/// Normalizes a stored `ModelSelection` JSON object (`provider` -> `instanceId`, strip null `options`).
pub(crate) fn normalize_model_selection_wire(mut val: Value) -> Value {
    if let Some(obj) = val.as_object_mut() {
        if !obj.contains_key("instanceId")
            && let Some(prov) = obj.get("provider").cloned().filter(Value::is_string)
        {
            obj.insert("instanceId".to_owned(), prov);
        }
        obj.remove("provider");
        if obj.get("options").is_some_and(Value::is_null) {
            obj.remove("options");
        }
    }
    val
}

fn has_non_empty_trimmed_str(obj: &Map<String, Value>, key: &str) -> bool {
    obj.get(key)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

/// Validates a `ThreadTitleRegeneration` JSON value, returning `None` if malformed or `null`.
#[must_use]
pub fn sanitize_title_regeneration_value(val: &Value) -> Option<Value> {
    let obj = val.as_object()?;
    if has_non_empty_trimmed_str(obj, "requestId") && has_non_empty_trimmed_str(obj, "startedAt") {
        Some(json!({
            "requestId": obj["requestId"].clone(),
            "startedAt": obj["startedAt"].clone(),
        }))
    } else {
        None
    }
}

/// Validates a `LastThreadTitleRegeneration` JSON value, returning `None` if malformed or `null`.
#[must_use]
pub fn sanitize_last_title_regeneration_value(val: &Value) -> Option<Value> {
    let obj = val.as_object()?;
    let status = obj.get("status").and_then(Value::as_str)?;
    if !matches!(status, "renamed" | "unchanged" | "failed") {
        return None;
    }
    if !has_non_empty_trimmed_str(obj, "requestId")
        || !has_non_empty_trimmed_str(obj, "completedAt")
    {
        return None;
    }
    let mut cleaned = Map::new();
    cleaned.insert("status".to_owned(), Value::String(status.to_owned()));
    cleaned.insert("requestId".to_owned(), obj["requestId"].clone());
    cleaned.insert("completedAt".to_owned(), obj["completedAt"].clone());
    if let Some(msg) = obj.get("message").and_then(Value::as_str) {
        cleaned.insert("message".to_owned(), Value::String(msg.to_owned()));
    }
    Some(Value::Object(cleaned))
}

/// Normalizes stored `AppThread` JSON (`settledOverride: "unsettled" -> "active"`, title regen, model selection).
#[must_use]
pub fn normalize_app_thread_wire(mut val: Value) -> Value {
    if let Some(obj) = val.as_object_mut() {
        if obj.get("settledOverride").and_then(Value::as_str) == Some("unsettled") {
            obj.insert(
                "settledOverride".to_owned(),
                Value::String("active".to_owned()),
            );
        }
        if let Some(tr) = obj.get("titleRegeneration")
            && !tr.is_null()
        {
            let sanitized = sanitize_title_regeneration_value(tr).unwrap_or(Value::Null);
            obj.insert("titleRegeneration".to_owned(), sanitized);
        }
        if let Some(ltr) = obj.get("lastTitleRegeneration")
            && !ltr.is_null()
        {
            let sanitized = sanitize_last_title_regeneration_value(ltr).unwrap_or(Value::Null);
            obj.insert("lastTitleRegeneration".to_owned(), sanitized);
        }
        if let Some(ms) = obj.remove("modelSelection") {
            obj.insert(
                "modelSelection".to_owned(),
                normalize_model_selection_wire(ms),
            );
        }
    }
    val
}

pub(crate) fn truncate_utf8_detail(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let marker_bytes = TRUNCATION_MARKER.len();
    let budget = max_bytes.saturating_sub(marker_bytes);
    let mut end = budget.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &value[..end])
}

/// Applies `capCheckpointFiles` (`MAX_WIRE_CHECKPOINT_FILES = 200`).
pub(crate) fn cap_checkpoint_files_value(mut val: Value) -> Value {
    let Some(obj) = val.as_object_mut() else {
        return val;
    };
    if obj.get("filesTotal").is_some_and(|v| !v.is_null()) {
        return val;
    }
    let Some(files) = obj.get("files").and_then(Value::as_array) else {
        return val;
    };
    if files.len() <= MAX_WIRE_CHECKPOINT_FILES {
        return val;
    }
    let mut additions = 0_u64;
    let mut deletions = 0_u64;
    for f in files {
        additions += f.get("additions").and_then(Value::as_u64).unwrap_or(0);
        deletions += f.get("deletions").and_then(Value::as_u64).unwrap_or(0);
    }
    let count = files.len() as u64;
    let capped = files[..MAX_WIRE_CHECKPOINT_FILES].to_vec();
    obj.insert("files".to_owned(), Value::Array(capped));
    obj.insert(
        "filesTotal".to_owned(),
        json!({ "count": count, "additions": additions, "deletions": deletions }),
    );
    val
}

/// Applies `projectContextHandoffForWire` (`WireProjection.ts`).
pub(crate) fn project_context_handoff_for_wire(mut val: Value) -> Value {
    if let Some(obj) = val.as_object_mut() {
        obj.remove("history");
        obj.remove("delivery");
        obj.insert("summaryText".to_owned(), Value::String(String::new()));
    }
    val
}

/// Matches `commandIndicatesFailure` + `toolOutputIndicatesFailure` (`packages/shared/src/toolOutput.ts`).
pub(crate) fn command_indicates_failure(obj: &Map<String, Value>) -> bool {
    if let Some(code) = obj.get("exitCode").and_then(Value::as_i64) {
        return code != 0;
    }
    if obj.get("outputIndicatesFailure").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    let Some(output) = obj.get("output").and_then(Value::as_str) else {
        return false;
    };
    let slice = &output[..output.len().min(32_768)];
    let lower = slice.to_ascii_lowercase();
    lower.contains("file not found")
        || lower.contains("no files found")
        || lower.contains("enoent")
        || lower.contains("no such file")
        || lower.contains("commandnotfoundexception")
        || lower.contains("command not found")
        || lower.contains("is not recognized as the name of a cmdlet")
        || lower.contains("a parameter cannot be found that matches parameter name")
        || (lower.contains("cannot find path") && lower.contains("because it does not exist"))
        || has_nonzero_exit_code_text(&lower)
}

fn has_nonzero_exit_code_text(lower: &str) -> bool {
    for needle in ["exit code ", "exit code:", "exited with exit code "] {
        let mut search = lower;
        while let Some(idx) = search.find(needle) {
            let rest = search[idx + needle.len()..].trim_start();
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(code) = digits.parse::<i64>()
                && code > 0
            {
                return true;
            }
            search = &search[idx + needle.len()..];
        }
    }
    false
}

/// Applies `projectTurnItemForWire` (`apps/server/src/orchestration-v2/WireProjection.ts:158-203`).
pub(crate) fn project_turn_item_for_wire(mut val: Value) -> Value {
    let Some(kind) = val.get("type").and_then(Value::as_str).map(str::to_owned) else {
        return val;
    };
    match kind.as_str() {
        "checkpoint" => cap_checkpoint_files_value(val),
        "handoff" => {
            if let Some(obj) = val.as_object_mut() {
                obj.remove("summary");
            }
            val
        }
        "command_execution" => {
            if let Some(obj) = val.as_object_mut() {
                let failed = command_indicates_failure(obj);
                let had_nonempty = obj
                    .remove("output")
                    .and_then(|v| v.as_str().map(|s| !s.trim().is_empty()))
                    .unwrap_or(false);
                if failed {
                    obj.insert("outputIndicatesFailure".to_owned(), Value::Bool(true));
                }
                if had_nonempty {
                    obj.insert("outputOmitted".to_owned(), Value::Bool(true));
                }
            }
            val
        }
        "file_change" => {
            if let Some(obj) = val.as_object_mut() {
                let is_failed = obj.get("status").and_then(Value::as_str) == Some("failed");
                obj.remove("oldStr");
                obj.remove("newStr");
                let diff = obj.remove("diffStr");
                if is_failed
                    && let Some(Value::String(s)) = diff
                    && !s.trim().is_empty()
                {
                    obj.insert(
                        "diffStr".to_owned(),
                        Value::String(truncate_utf8_detail(&s, MAX_DETAIL_STRING_BYTES)),
                    );
                }
            }
            val
        }
        "subagent" => {
            if let Some(obj) = val.as_object_mut() {
                let prompt = obj
                    .get("prompt")
                    .and_then(Value::as_str)
                    .map(|s| truncate_utf8_detail(s, MAX_DETAIL_STRING_BYTES))
                    .unwrap_or_default();
                obj.insert("prompt".to_owned(), Value::String(prompt));
                if let Some(Value::String(p)) = obj.get("progress") {
                    let truncated = truncate_utf8_detail(p, MAX_DETAIL_STRING_BYTES);
                    obj.insert("progress".to_owned(), Value::String(truncated));
                }
                match obj.get("result") {
                    Some(Value::Null) | None => {
                        obj.insert("result".to_owned(), Value::Null);
                    }
                    Some(Value::String(r)) => {
                        let truncated = truncate_utf8_detail(r, MAX_DETAIL_STRING_BYTES);
                        obj.insert("result".to_owned(), Value::String(truncated));
                    }
                    _ => {}
                }
            }
            val
        }
        "dynamic_tool" => project_dynamic_tool_for_wire(val),
        _ => val,
    }
}

fn project_dynamic_tool_for_wire(mut val: Value) -> Value {
    let Some(obj) = val.as_object_mut() else {
        return val;
    };
    if let Some(input) = obj.get("input").cloned() {
        obj.insert("input".to_owned(), summarize_dynamic_value(input));
    }
    let raw_output = obj.remove("output");
    if let Some(ref raw) = raw_output {
        let has_val = match raw {
            Value::Null => false,
            Value::String(s) => !s.trim().is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(m) => !m.is_empty(),
            _ => true,
        };
        if has_val {
            obj.insert("outputOmitted".to_owned(), Value::Bool(true));
        }
        let compact = serde_json::to_string(raw)
            .ok()
            .filter(|s| s.len() <= MAX_DYNAMIC_VALUE_BYTES);
        if compact.is_some() {
            obj.insert("output".to_owned(), raw.clone());
        }
    }
    val
}

fn summarize_dynamic_value(value: Value) -> Value {
    let Ok(serialized) = (match &value {
        Value::String(s) => Ok(s.clone()),
        _ => serde_json::to_string(&value),
    }) else {
        return json!({ "summary": "Unserializable tool output", "truncated": true });
    };
    if serde_json::to_string(&value).is_ok_and(|s| s.len() <= MAX_DYNAMIC_VALUE_BYTES) {
        return value;
    }
    let line = serialized.lines().next().unwrap_or("Large tool output");
    let normalized: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped: String = normalized.chars().take(160).collect();
    let summary = if normalized.chars().count() <= 160 {
        clipped
    } else {
        format!("{}…", clipped.trim_end())
    };
    json!({
        "summary": if summary.is_empty() { "Large tool output" } else { &summary },
        "truncated": true,
    })
}
