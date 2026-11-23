//! xAI Grok HTTP REST/SSE request builders and OpenAI-compatible response parser.
//!
//! Builds `/v1/chat/completions` (`stream: true` / `false`) and `/v1/models`
//! request specs with `Authorization: Bearer` headers and parses incremental
//! SSE chunks (`[DONE]` sentinel, text/reasoning deltas, tool calls, usage).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::CatalogModelEntry;

use super::{NonAcpHttpRequestSpec, NonAcpParseError, NonAcpTurnUpdate, str_field, u64_field};

/// Default xAI public REST API origin.
pub const DEFAULT_XAI_API_BASE: &str = "https://api.x.ai";

/// Chat message serialized into xAI `/v1/chat/completions` requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokChatMessage {
    /// Speaker role (`"system"`, `"user"`, `"assistant"`, `"tool"`).
    pub role: String,
    /// Message text payload.
    pub content: String,
}

fn grok_headers(api_key: &str, json_body: bool) -> IndexMap<String, String> {
    let mut headers = IndexMap::new();
    headers.insert("Authorization".to_owned(), format!("Bearer {api_key}"));
    headers.insert("Accept".to_owned(), "application/json".to_owned());
    if json_body {
        headers.insert("Content-Type".to_owned(), "application/json".to_owned());
    }
    headers
}

/// Builds a `POST <base>/v1/chat/completions` request specification for xAI Grok.
///
/// # Examples
///
/// ```
/// use legion_providers::non_acp::{GrokChatMessage, build_grok_chat_completions_request};
///
/// let spec = build_grok_chat_completions_request(
///     None,
///     "xai-key",
///     "grok-4",
///     &[GrokChatMessage { role: "user".to_owned(), content: "Hello".to_owned() }],
///     true,
///     Some(1024),
/// );
/// assert_eq!(spec.url, "https://api.x.ai/v1/chat/completions");
/// assert_eq!(spec.headers.get("Authorization").map(String::as_str), Some("Bearer xai-key"));
/// ```
#[must_use]
pub fn build_grok_chat_completions_request(
    base_url: Option<&str>,
    api_key: &str,
    model: &str,
    messages: &[GrokChatMessage],
    stream: bool,
    max_tokens: Option<u64>,
) -> NonAcpHttpRequestSpec {
    let base = base_url
        .unwrap_or(DEFAULT_XAI_API_BASE)
        .trim_end_matches('/');
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });
    if let Some(map) = body.as_object_mut() {
        if stream {
            map.insert(
                "stream_options".to_owned(),
                json!({ "include_usage": true }),
            );
        }
        if let Some(limit) = max_tokens {
            map.insert("max_tokens".to_owned(), Value::Number(limit.into()));
        }
    }
    NonAcpHttpRequestSpec {
        method: "POST".to_owned(),
        url: format!("{base}/v1/chat/completions"),
        headers: grok_headers(api_key, true),
        body: Some(body),
    }
}

/// Builds a `GET <base>/v1/models` request specification for xAI Grok model discovery.
#[must_use]
pub fn build_grok_models_request(base_url: Option<&str>, api_key: &str) -> NonAcpHttpRequestSpec {
    let base = base_url
        .unwrap_or(DEFAULT_XAI_API_BASE)
        .trim_end_matches('/');
    NonAcpHttpRequestSpec {
        method: "GET".to_owned(),
        url: format!("{base}/v1/models"),
        headers: grok_headers(api_key, false),
        body: None,
    }
}

/// Parses an xAI `/v1/models` JSON response into deduplicated [`CatalogModelEntry`] items.
pub fn parse_grok_models_response(
    json_body: &str,
) -> Result<Vec<CatalogModelEntry>, NonAcpParseError> {
    let val: Value = serde_json::from_str(json_body)?;
    let arr =
        val.get("data")
            .and_then(Value::as_array)
            .ok_or(NonAcpParseError::MalformedFrame {
                reason: "xAI /v1/models response missing `data` array",
            })?;

    let mut out = Vec::new();
    for item in arr {
        let Some(id) = str_field(Some(item), "id") else {
            continue;
        };
        if out.iter().any(|m: &CatalogModelEntry| m.slug == id) {
            continue;
        }
        let lower = id.to_ascii_lowercase();
        out.push(CatalogModelEntry {
            slug: id.to_owned(),
            name: id.to_owned(),
            is_custom: false,
            supports_reasoning: lower.contains("reasoning") || lower.contains("grok-4"),
        });
    }
    Ok(out)
}

/// Parses one SSE `data:` frame from xAI Grok `/v1/chat/completions` (`[DONE]` aware).
pub fn parse_grok_sse_chunk(data: &str) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed == "[DONE]" {
        return Ok(vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: Some("stop".to_owned()),
        }]);
    }

    let val: Value = serde_json::from_str(trimmed)?;
    if let Some(err) = val.get("error") {
        let message = str_field(Some(err), "message")
            .unwrap_or("xAI API request failed")
            .to_owned();
        return Ok(vec![NonAcpTurnUpdate::TurnFailed { message }]);
    }

    let mut out = Vec::new();
    if let Some(id) = str_field(Some(&val), "id")
        && let Some(model) = str_field(Some(&val), "model")
        && val
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("role"))
            .is_some()
    {
        out.push(NonAcpTurnUpdate::SessionStarted {
            provider_session_id: id.to_owned(),
            model: Some(model.to_owned()),
        });
    }

    if let Some(choice) = val
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    {
        let delta = choice.get("delta");
        if let Some(reasoning) = str_field(delta, "reasoning_content")
            && !reasoning.is_empty()
        {
            out.push(NonAcpTurnUpdate::ReasoningDelta {
                text: reasoning.to_owned(),
            });
        }
        if let Some(content) = str_field(delta, "content")
            && !content.is_empty()
        {
            out.push(NonAcpTurnUpdate::AssistantTextDelta {
                text: content.to_owned(),
            });
        }
        if let Some(calls) = delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(Value::as_array)
        {
            for call in calls {
                if let Some(name) = call
                    .get("function")
                    .and_then(|f| str_field(Some(f), "name"))
                {
                    let call_id = str_field(Some(call), "id").unwrap_or("call_0").to_owned();
                    let args = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    out.push(NonAcpTurnUpdate::ToolCallStarted {
                        call_id,
                        name: name.to_owned(),
                        input_json: args,
                    });
                }
            }
        }
        if let Some(reason) = str_field(Some(choice), "finish_reason") {
            out.push(NonAcpTurnUpdate::TurnCompleted {
                stop_reason: Some(reason.to_owned()),
            });
        }
    }

    if let Some(usage) = val.get("usage") {
        let input = u64_field(Some(usage), "prompt_tokens").unwrap_or(0);
        let output = u64_field(Some(usage), "completion_tokens").unwrap_or(0);
        if input > 0 || output > 0 {
            out.push(NonAcpTurnUpdate::UsageReported {
                input_tokens: input,
                output_tokens: output,
            });
        }
    }

    Ok(out)
}
