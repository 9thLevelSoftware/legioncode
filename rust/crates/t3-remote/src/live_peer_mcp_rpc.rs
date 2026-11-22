//! Low-level HTTP/SSE JSON-RPC transport helpers for [`super::LivePeerMcpSession`].

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use t3_contracts::ids::{EnvironmentId, ThreadId};
use t3_protocol::mcp::{
    HEADER_MODE_LIMIT, HEADER_SENDER_THREAD, format_mode_limit, parse_sse_json_rpc_response,
};
use t3_protocol::rpc_codec::RpcId;

use super::types::{LivePeerMcpOptions, PeerMcpError};
use crate::acp_mcp_bridge::{MCP_PROTOCOL_VERSION, strip_duplicate_structured_text};

const SENDER_TITLE_MAX_CHARS: usize = 200;
pub(super) const HEADER_MCP_SESSION_ID: &str = "mcp-session-id";
pub(super) const HEADER_MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";

/// Formats `T3-Sender-Thread` matching `PeerMcpClient.ts:179-188`.
pub(super) fn format_sender_thread_header(
    env_id: &EnvironmentId,
    thread_id: &ThreadId,
    title: &str,
) -> String {
    let short_title: String = title.chars().take(SENDER_TITLE_MAX_CHARS).collect();
    format!(
        "{}/{}/{}",
        encode_uri_component(env_id.as_str()),
        encode_uri_component(thread_id.as_str()),
        encode_uri_component(&short_title),
    )
}

fn encode_uri_component(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

pub(super) fn build_initialize_payload(id: i64, client_name: &str, client_version: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": client_name, "version": client_version }
        }
    })
}

pub(super) fn decode_json_or_sse_response(
    content_type: &str,
    body: &str,
    expected_id: &RpcId,
) -> Result<Value, PeerMcpError> {
    let media = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if media == "text/event-stream" {
        return parse_sse_json_rpc_response(body, expected_id).map_err(|err| PeerMcpError::Parse {
            detail: err.to_string(),
        });
    }
    if body.trim().is_empty() {
        return Err(PeerMcpError::Parse {
            detail: "empty response body from /mcp".to_owned(),
        });
    }
    serde_json::from_str::<Value>(body).map_err(|err| PeerMcpError::Parse {
        detail: err.to_string(),
    })
}

pub(super) fn extract_tool_call_result(mut envelope: Value) -> Result<Value, PeerMcpError> {
    if let Some(err) = envelope.get("error").filter(|v| !v.is_null()) {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(-32603);
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("JSON-RPC call failed")
            .to_owned();
        return Err(PeerMcpError::JsonRpc { code, message });
    }
    let Some(result) = envelope.get_mut("result").map(Value::take) else {
        return Err(PeerMcpError::Parse {
            detail: "JSON-RPC response missing result object".to_owned(),
        });
    };
    let mut normalized = result;
    strip_duplicate_structured_text(&mut normalized);
    if normalized.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = collect_result_text(&normalized);
        if let Ok(parsed) = serde_json::from_str::<Value>(&text)
            && let Some(code) = parsed.get("code").and_then(Value::as_str)
        {
            let message = parsed
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(&text)
                .to_owned();
            return Err(PeerMcpError::ToolRefused {
                code: code.to_owned(),
                message,
            });
        }
        return Err(PeerMcpError::ToolRefused {
            code: "orchestration_error".to_owned(),
            message: if text.is_empty() {
                "The linked environment refused the call.".to_owned()
            } else {
                text
            },
        });
    }
    if let Some(structured) = normalized.get("structuredContent").filter(|v| !v.is_null()) {
        return Ok(structured.clone());
    }
    let text = collect_result_text(&normalized);
    if !text.is_empty()
        && let Ok(parsed) = serde_json::from_str::<Value>(&text)
    {
        return Ok(parsed);
    }
    Ok(normalized)
}

fn collect_result_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .map_or_default(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
}

pub(super) fn build_mcp_request_headers(
    token: &str,
    session_id: Option<&str>,
    protocol_version: Option<&str>,
    options: &LivePeerMcpOptions,
) -> Result<HeaderMap, PeerMcpError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    let auth_val = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        PeerMcpError::InvalidSession {
            detail: "bearer token contains invalid header characters".to_owned(),
        }
    })?;
    headers.insert(AUTHORIZATION, auth_val);
    if let Some(sid) = session_id
        && let Ok(v) = HeaderValue::from_str(sid)
    {
        headers.insert(HeaderName::from_static(HEADER_MCP_SESSION_ID), v);
    }
    if let Some(proto) = protocol_version
        && let Ok(v) = HeaderValue::from_str(proto)
    {
        headers.insert(HeaderName::from_static(HEADER_MCP_PROTOCOL_VERSION), v);
    }
    if let Some(limit) = options.mode_limit
        && let Ok(name) = HeaderName::from_bytes(HEADER_MODE_LIMIT.as_bytes())
        && let Ok(v) = HeaderValue::from_str(&format_mode_limit(limit))
    {
        headers.insert(name, v);
    }
    if let Some((env_id, thread_id, title)) = &options.sender_thread
        && let Ok(name) = HeaderName::from_bytes(HEADER_SENDER_THREAD.as_bytes())
        && let Ok(v) = HeaderValue::from_str(&format_sender_thread_header(env_id, thread_id, title))
    {
        headers.insert(name, v);
    }
    Ok(headers)
}

pub(super) async fn post_mcp_json(
    http: &Client,
    origin: &str,
    headers: HeaderMap,
    payload: &Value,
    timeout: std::time::Duration,
) -> Result<reqwest::Response, PeerMcpError> {
    http.post(format!("{origin}/mcp"))
        .headers(headers)
        .json(payload)
        .timeout(timeout)
        .send()
        .await
        .map_err(|err| PeerMcpError::Transport {
            detail: err.to_string(),
        })
}

pub(super) async fn initialize_mcp_session(
    http: &Client,
    origin: &str,
    token: &str,
    options: &LivePeerMcpOptions,
    init_id: i64,
) -> Result<(String, String), PeerMcpError> {
    let init_payload =
        build_initialize_payload(init_id, &options.client_name, &options.client_version);
    let headers = build_mcp_request_headers(token, None, None, options)?;
    let resp = post_mcp_json(http, origin, headers, &init_payload, options.timeout).await?;
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let content_type = resp_headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let sid = resp_headers
        .get(HEADER_MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let body = resp.text().await.map_err(|err| PeerMcpError::Transport {
        detail: err.to_string(),
    })?;
    if !status.is_success() {
        return Err(PeerMcpError::HttpStatus {
            status: status.as_u16(),
            detail: body,
        });
    }
    let session_id = sid.ok_or_else(|| PeerMcpError::InvalidSession {
        detail: "initialize response omitted mcp-session-id header".to_owned(),
    })?;
    let entry = decode_json_or_sse_response(&content_type, &body, &RpcId::from(init_id))?;
    let proto = entry
        .get("result")
        .and_then(|r| r.get("protocolVersion"))
        .and_then(Value::as_str)
        .map_or_else(|| MCP_PROTOCOL_VERSION.to_owned(), str::to_owned);

    let notify_headers =
        build_mcp_request_headers(token, Some(&session_id), Some(&proto), options)?;
    let notify_resp = post_mcp_json(
        http,
        origin,
        notify_headers,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        options.timeout,
    )
    .await;
    match notify_resp {
        Ok(r) if r.status().is_success() => Ok((session_id, proto)),
        Ok(r) => {
            let _ = terminate_mcp_session(http, origin, token, &session_id, options).await;
            Err(PeerMcpError::HttpStatus {
                status: r.status().as_u16(),
                detail: "notifications/initialized rejected".to_owned(),
            })
        }
        Err(err) => {
            let _ = terminate_mcp_session(http, origin, token, &session_id, options).await;
            Err(err)
        }
    }
}

pub(super) async fn terminate_mcp_session(
    http: &Client,
    origin: &str,
    token: &str,
    session_id: &str,
    options: &LivePeerMcpOptions,
) -> Result<(), PeerMcpError> {
    let headers = build_mcp_request_headers(token, Some(session_id), None, options)?;
    let resp = http
        .delete(format!("{origin}/mcp"))
        .headers(headers)
        .timeout(options.timeout)
        .send()
        .await
        .map_err(|err| PeerMcpError::Transport {
            detail: err.to_string(),
        })?;
    let status = resp.status();
    if status.is_success()
        || status == StatusCode::NOT_FOUND
        || status == StatusCode::METHOD_NOT_ALLOWED
    {
        Ok(())
    } else {
        Err(PeerMcpError::HttpStatus {
            status: status.as_u16(),
            detail: "DELETE /mcp cleanup failed".to_owned(),
        })
    }
}
