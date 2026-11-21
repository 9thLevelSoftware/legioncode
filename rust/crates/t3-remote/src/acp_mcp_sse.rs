//! Incremental SSE frame decoding, JSON-RPC request replies, and result normalization.

use reqwest::header::CONTENT_TYPE;
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};

use crate::acp_mcp_bridge::McpBridgeError;

pub(super) async fn decode_response_entries(
    mut resp: Response,
) -> Result<Vec<Value>, McpBridgeError> {
    if matches!(resp.status(), StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
        let _ = resp.bytes().await;
        return Ok(Vec::new());
    }

    let media_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    if media_type == "text/event-stream" {
        let mut decoder = SseFrameDecoder::default();
        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|err| McpBridgeError::Transport {
                detail: err.to_string(),
            })?
        {
            let mut events = decoder
                .push_bytes(&chunk)
                .map_err(|detail| McpBridgeError::Parse { detail })?;
            out.append(&mut events);
        }
        let mut tail = decoder
            .finish()
            .map_err(|detail| McpBridgeError::Parse { detail })?;
        out.append(&mut tail);
        return Ok(out);
    }

    let body = resp.text().await.map_err(|err| McpBridgeError::Transport {
        detail: err.to_string(),
    })?;
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    let val: Value = serde_json::from_str(&body).map_err(|err| McpBridgeError::Parse {
        detail: err.to_string(),
    })?;
    Ok(vec![val])
}

/// Incremental SSE event decoder matching `packages/shared/src/mcpResponsePayloads.ts`.
#[derive(Debug, Default)]
pub(super) struct SseFrameDecoder {
    buffer: String,
}

impl SseFrameDecoder {
    pub(super) fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<Value>, String> {
        let chunk = std::str::from_utf8(bytes)
            .map_err(|err| format!("invalid UTF-8 in SSE stream: {err}"))?;
        let normalized = chunk.replace("\r\n", "\n").replace('\r', "\n");
        self.buffer.push_str(&normalized);
        self.drain_complete_events()
    }

    pub(super) fn finish(&mut self) -> Result<Vec<Value>, String> {
        let mut events = self.drain_complete_events()?;
        let trailing = self.buffer.trim();
        if !trailing.is_empty()
            && let Some(data) = extract_sse_event_data(&self.buffer)
        {
            let parsed: Value = serde_json::from_str(&data)
                .map_err(|err| format!("invalid JSON in trailing SSE block: {err}"))?;
            events.push(parsed);
        }
        self.buffer.clear();
        Ok(events)
    }

    fn drain_complete_events(&mut self) -> Result<Vec<Value>, String> {
        let mut events = Vec::new();
        while let Some(sep_idx) = self.buffer.find("\n\n") {
            let raw_event = self.buffer[..sep_idx].to_owned();
            self.buffer.drain(..sep_idx + 2);
            if let Some(data) = extract_sse_event_data(&raw_event) {
                let parsed: Value = serde_json::from_str(&data)
                    .map_err(|err| format!("invalid JSON in SSE block: {err}"))?;
                events.push(parsed);
            }
        }
        Ok(events)
    }
}

fn extract_sse_event_data(raw_event: &str) -> Option<String> {
    let mut parts = Vec::new();
    for line in raw_event.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            parts.push(rest.trim_start());
        }
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("\n");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

pub(super) fn protocol_version_of(entry: &Value) -> Option<&str> {
    entry
        .get("result")?
        .get("protocolVersion")?
        .as_str()
        .filter(|s| !s.is_empty())
}

pub(super) fn is_server_request(entry: &Value) -> bool {
    entry.get("id").is_some() && entry.get("method").and_then(Value::as_str).is_some()
}

pub(super) fn reply_to_server_request(entry: &Value) -> Value {
    let id = entry.get("id").cloned().unwrap_or(Value::Null);
    let method = entry.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "Method not found" }
        })
    }
}

/// Removes duplicated compact-JSON text parts when `structuredContent` is present (`AcpMcpStdioBridge.ts:396-419`).
pub fn strip_duplicate_structured_text(result: &mut Value) {
    let Some(obj) = result.as_object_mut() else {
        return;
    };
    let Some(structured) = obj.get("structuredContent").filter(|v| !v.is_null()) else {
        return;
    };
    let Ok(encoded) = serde_json::to_string(structured) else {
        return;
    };
    let Some(content) = obj.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    content.retain(|part| {
        let is_dup = part.get("type").and_then(Value::as_str) == Some("text")
            && part.get("text").and_then(Value::as_str) == Some(encoded.as_str());
        !is_dup
    });
}
