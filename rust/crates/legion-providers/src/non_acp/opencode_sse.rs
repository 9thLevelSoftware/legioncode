//! Incremental Server-Sent Events (SSE) parser and `OpenCode` HTTP/SSE adapters.
//!
//! Supports both `OpenCode` 2.x event streams (`session.text.delta`,
//! `session.tool.*`, `permission.asked`, `session.step.ended`,
//! `session.execution.*`) and legacy 1.x (`message.part.updated`,
//! `session.error`, `session.idle`) over [`SseStreamParser`].

use indexmap::IndexMap;
use legion_contracts::orchestration::ProviderApprovalDecision;
use serde_json::{Value, json};

use crate::drivers::opencode_basic_auth_header;

pub use super::sse::{SseEventFrame, SseStreamParser};
use super::{NonAcpHttpRequestSpec, NonAcpParseError, NonAcpTurnUpdate, str_field, u64_field};

fn opencode_headers(password: Option<&str>, accept: &str) -> IndexMap<String, String> {
    let mut headers = IndexMap::new();
    headers.insert("Accept".to_owned(), accept.to_owned());
    if accept == "application/json" {
        headers.insert("Content-Type".to_owned(), "application/json".to_owned());
    }
    if let Some(pw) = password.filter(|s| !s.is_empty()) {
        headers.insert("Authorization".to_owned(), opencode_basic_auth_header(pw));
    }
    headers
}

fn opencode_spec(
    method: &str,
    url: String,
    password: Option<&str>,
    accept: &str,
    body: Option<Value>,
) -> NonAcpHttpRequestSpec {
    NonAcpHttpRequestSpec {
        method: method.to_owned(),
        url,
        headers: opencode_headers(password, accept),
        body,
    }
}

/// Builds a `POST <base>/session` HTTP specification creating an `OpenCode` session.
#[must_use]
pub fn build_opencode_create_session_request(
    base_url: &str,
    password: Option<&str>,
    directory: Option<&str>,
    title: Option<&str>,
) -> NonAcpHttpRequestSpec {
    let base = base_url.trim_end_matches('/');
    let mut body = json!({});
    if let Some(map) = body.as_object_mut() {
        if let Some(dir) = directory {
            map.insert("directory".to_owned(), Value::String(dir.to_owned()));
        }
        if let Some(t) = title {
            map.insert("title".to_owned(), Value::String(t.to_owned()));
        }
    }
    opencode_spec(
        "POST",
        format!("{base}/session"),
        password,
        "application/json",
        Some(body),
    )
}

/// Builds a `POST <base>/session/<id>/message` prompt submission specification.
#[must_use]
pub fn build_opencode_prompt_request(
    base_url: &str,
    password: Option<&str>,
    session_id: &str,
    text: &str,
    model: Option<(&str, &str)>,
) -> NonAcpHttpRequestSpec {
    let base = base_url.trim_end_matches('/');
    let mut body = json!({ "parts": [{ "type": "text", "text": text }] });
    if let Some((provider_id, model_id)) = model
        && let Some(map) = body.as_object_mut()
    {
        map.insert(
            "model".to_owned(),
            json!({ "providerID": provider_id, "modelID": model_id }),
        );
    }
    opencode_spec(
        "POST",
        format!("{base}/session/{session_id}/message"),
        password,
        "application/json",
        Some(body),
    )
}

/// Builds a `POST <base>/session/<id>/abort` cancellation specification.
#[must_use]
pub fn build_opencode_abort_request(
    base_url: &str,
    password: Option<&str>,
    session_id: &str,
) -> NonAcpHttpRequestSpec {
    let base = base_url.trim_end_matches('/');
    opencode_spec(
        "POST",
        format!("{base}/session/{session_id}/abort"),
        password,
        "application/json",
        None,
    )
}

/// Builds an `OpenCode` permission reply HTTP specification (`once` / `always` / `reject`).
#[must_use]
pub fn build_opencode_permission_reply_request(
    base_url: &str,
    password: Option<&str>,
    session_id: &str,
    permission_id: &str,
    decision: ProviderApprovalDecision,
) -> NonAcpHttpRequestSpec {
    let base = base_url.trim_end_matches('/');
    let reply = match decision {
        ProviderApprovalDecision::Accept => "once",
        ProviderApprovalDecision::AcceptAlways | ProviderApprovalDecision::AcceptForSession => {
            "always"
        }
        ProviderApprovalDecision::Decline | ProviderApprovalDecision::Cancel => "reject",
    };
    opencode_spec(
        "POST",
        format!("{base}/session/{session_id}/permissions/{permission_id}"),
        password,
        "application/json",
        Some(json!({ "response": reply })),
    )
}

/// Builds a `GET <base>/event` SSE subscription request specification.
#[must_use]
pub fn build_opencode_event_stream_request(
    base_url: &str,
    password: Option<&str>,
    directory: Option<&str>,
) -> NonAcpHttpRequestSpec {
    let base = base_url.trim_end_matches('/');
    let url = match directory.filter(|s| !s.is_empty()) {
        Some(dir) => format!("{base}/event?directory={dir}"),
        None => format!("{base}/event"),
    };
    opencode_spec("GET", url, password, "text/event-stream", None)
}

/// Parses one `OpenCode` SSE `data:` JSON payload into [`NonAcpTurnUpdate`] events.
pub fn parse_opencode_sse_data(data: &str) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(Vec::new());
    }
    let val: Value = serde_json::from_str(trimmed)?;
    let kind = str_field(Some(&val), "type").ok_or(NonAcpParseError::MalformedFrame {
        reason: "opencode SSE payload missing string `type`",
    })?;
    let payload = val.get("data").or_else(|| val.get("properties"));

    Ok(match kind {
        "session.text.delta" | "message.part.delta" => str_field(payload, "delta")
            .filter(|s| !s.is_empty())
            .map(|t| NonAcpTurnUpdate::AssistantTextDelta { text: t.to_owned() })
            .into_iter()
            .collect(),
        "session.reasoning.delta" => str_field(payload, "delta")
            .filter(|s| !s.is_empty())
            .map(|t| NonAcpTurnUpdate::ReasoningDelta { text: t.to_owned() })
            .into_iter()
            .collect(),
        "session.tool.input.started" | "session.tool.called" => {
            vec![NonAcpTurnUpdate::ToolCallStarted {
                call_id: str_field(payload, "id").unwrap_or("unknown").to_owned(),
                name: str_field(payload, "name").unwrap_or("tool").to_owned(),
                input_json: payload
                    .and_then(|p| p.get("input"))
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            }]
        }
        "session.tool.success" | "session.tool.failed" => {
            vec![NonAcpTurnUpdate::ToolCallCompleted {
                call_id: str_field(payload, "id").unwrap_or("unknown").to_owned(),
                output_json: payload
                    .and_then(|p| p.get("content").or_else(|| p.get("error")))
                    .cloned()
                    .unwrap_or(Value::Null),
                is_error: kind == "session.tool.failed",
            }]
        }
        "permission.asked" | "permission.updated" => {
            vec![NonAcpTurnUpdate::PermissionRequested {
                request_id: str_field(payload, "id")
                    .or_else(|| str_field(payload, "requestID"))
                    .unwrap_or("perm")
                    .to_owned(),
                tool_name: str_field(payload, "permission")
                    .or_else(|| str_field(payload, "title"))
                    .unwrap_or("permission")
                    .to_owned(),
                detail: payload.cloned().unwrap_or(Value::Null),
            }]
        }
        "session.step.ended" => {
            let tokens = payload.and_then(|p| p.get("tokens"));
            let input = u64_field(tokens, "input").unwrap_or(0);
            let output = u64_field(tokens, "output").unwrap_or(0)
                + u64_field(tokens, "reasoning").unwrap_or(0);
            if input == 0 && output == 0 {
                Vec::new()
            } else {
                vec![NonAcpTurnUpdate::UsageReported {
                    input_tokens: input,
                    output_tokens: output,
                }]
            }
        }
        "session.execution.succeeded" | "session.execution.interrupted" | "session.idle" => {
            vec![NonAcpTurnUpdate::TurnCompleted {
                stop_reason: Some(kind.to_owned()),
            }]
        }
        "session.execution.failed" | "session.error" => {
            let msg = payload
                .and_then(|p| p.get("error").or(Some(p)))
                .and_then(|e| str_field(Some(e), "message"))
                .unwrap_or("OpenCode turn failed")
                .to_owned();
            vec![NonAcpTurnUpdate::TurnFailed { message: msg }]
        }
        _ => Vec::new(),
    })
}
