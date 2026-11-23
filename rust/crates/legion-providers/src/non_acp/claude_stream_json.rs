//! Claude Code `stream-json` NDJSON prompt builders, control replies, and parser.
//!
//! Mirrors `@anthropic-ai/claude-agent-sdk` wire frames used by
//! `ClaudeTextGeneration.ts` and `ClaudeAdapterV2.ts`: outbound
//! `{"type":"user","message":{"role":"user","content":...}}`, inbound
//! `system`, `assistant`, `user`, `stream_event`, `control_request`, and `result`.

use legion_contracts::orchestration::ProviderApprovalDecision;
use legion_protocol::provider_env::NdjsonLineCodec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::claude_stream_usage::ClaudeStreamUsage;
use super::{NonAcpParseError, NonAcpTurnUpdate, str_field};

/// Content block serialized inside Claude `stream-json` user or assistant messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeContentBlock {
    /// Text prompt or reply segment.
    Text {
        /// Plain text or Markdown payload.
        text: String,
    },
    /// Base64-encoded image attachment accepted by `claude -p --input-format stream-json`.
    Image {
        /// Anthropic source object (`type`, `media_type`, `data`).
        source: ClaudeImageSource,
    },
    /// Tool invocation emitted by Claude inside an `assistant` message.
    ToolUse {
        /// Unique tool-use identifier matched by subsequent `tool_result`.
        id: String,
        /// Tool name (`Bash`, `Edit`, `Read`, `mcp__*`, etc.).
        name: String,
        /// Structured tool input arguments.
        input: Value,
    },
    /// Tool result echoed by the harness inside a `user` message.
    ToolResult {
        /// Matching `ToolUse::id`.
        tool_use_id: String,
        /// String or content block output.
        content: Value,
        /// Whether tool execution failed.
        #[serde(default)]
        is_error: bool,
    },
}

/// Base64 image source nested inside [`ClaudeContentBlock::Image`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeImageSource {
    /// Always `"base64"`.
    #[serde(rename = "type")]
    pub source_type: String,
    /// Media MIME type (`"image/png"`, `"image/jpeg"`, etc.).
    pub media_type: String,
    /// Base64 image bytes.
    pub data: String,
}

/// Builds a Claude `stream-json` user prompt envelope (`{"type":"user","message":...}`).
///
/// # Examples
///
/// ```
/// use legion_providers::non_acp::build_claude_user_prompt_envelope;
///
/// let env = build_claude_user_prompt_envelope("Explain Cargo.toml", &[], Some("sess-1"));
/// assert_eq!(env["type"], "user");
/// assert_eq!(env["message"]["role"], "user");
/// assert_eq!(env["session_id"], "sess-1");
/// ```
#[must_use]
pub fn build_claude_user_prompt_envelope(
    prompt: &str,
    images: &[(&str, &str)],
    session_id: Option<&str>,
) -> Value {
    let content: Value = if images.is_empty() {
        json!(prompt)
    } else {
        let mut blocks = vec![json!({ "type": "text", "text": prompt })];
        for (media_type, base64_data) in images {
            blocks.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": base64_data,
                }
            }));
        }
        Value::Array(blocks)
    };

    let mut frame = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": content,
        }
    });
    if let Some(sid) = session_id
        && let Some(map) = frame.as_object_mut()
    {
        map.insert("session_id".to_owned(), Value::String(sid.to_owned()));
    }
    frame
}

/// Encodes a Claude `stream-json` user prompt envelope as a single NDJSON line.
pub fn encode_claude_user_prompt_line(
    prompt: &str,
    images: &[(&str, &str)],
    session_id: Option<&str>,
) -> Result<String, NonAcpParseError> {
    let val = build_claude_user_prompt_envelope(prompt, images, session_id);
    Ok(NdjsonLineCodec::default().encode_line(&val)?)
}

/// Builds a Claude SDK `control_response` replying to an inbound `control_request`.
#[must_use]
pub fn build_claude_control_response(request_id: &str, outcome: Result<Value, String>) -> Value {
    match outcome {
        Ok(payload) => json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": payload,
            }
        }),
        Err(err) => json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request_id,
                "error": err,
            }
        }),
    }
}

/// Builds a `control_response` for Claude `can_use_tool` permission prompts.
#[must_use]
pub fn build_claude_permission_control_response(
    request_id: &str,
    decision: ProviderApprovalDecision,
    updated_input: Option<Value>,
) -> Value {
    let payload = match decision {
        ProviderApprovalDecision::Accept
        | ProviderApprovalDecision::AcceptAlways
        | ProviderApprovalDecision::AcceptForSession => json!({
            "behavior": "allow",
            "updatedInput": updated_input.unwrap_or_else(|| json!({})),
        }),
        ProviderApprovalDecision::Decline => json!({
            "behavior": "deny",
            "message": "Denied by user",
        }),
        ProviderApprovalDecision::Cancel => json!({
            "behavior": "deny",
            "message": "Cancelled by user",
            "interrupt": true,
        }),
    };
    build_claude_control_response(request_id, Ok(payload))
}

/// Builds a Claude `control_request` frame cancelling the active turn (`interrupt`).
#[must_use]
pub fn build_claude_interrupt_control_request(request_id: &str) -> Value {
    json!({ "type": "control_request", "request_id": request_id, "request": { "subtype": "interrupt" } })
}

/// Parses one Claude Code `stream-json` NDJSON line into [`NonAcpTurnUpdate`] items.
pub fn parse_claude_stream_json_line(
    line: &str,
) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let val: Value = serde_json::from_str(trimmed)?;
    let frame_type = str_field(Some(&val), "type").ok_or(NonAcpParseError::MalformedFrame {
        reason: "claude stream-json line missing string `type`",
    })?;
    match frame_type {
        "system" => Ok(parse_claude_system(&val)),
        "assistant" => Ok(parse_claude_assistant(&val)),
        "user" => Ok(parse_claude_user(&val)),
        "stream_event" => Ok(parse_claude_stream_event(&val)),
        "control_request" => Ok(parse_claude_control_request(&val)),
        "result" => Ok(parse_claude_result(&val)),
        _ => Ok(Vec::new()),
    }
}

fn parse_claude_system(val: &Value) -> Vec<NonAcpTurnUpdate> {
    if str_field(Some(val), "subtype") != Some("init") {
        return Vec::new();
    }
    str_field(Some(val), "session_id")
        .map(|sid| NonAcpTurnUpdate::SessionStarted {
            provider_session_id: sid.to_owned(),
            model: str_field(Some(val), "model").map(ToOwned::to_owned),
        })
        .into_iter()
        .collect()
}

fn parse_claude_assistant(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let mut out = Vec::new();
    let msg = val.get("message");
    if let Some(blocks) = msg.and_then(|m| m.get("content")).and_then(Value::as_array) {
        for block in blocks {
            match str_field(Some(block), "type") {
                Some("text") => {
                    if let Some(t) = str_field(Some(block), "text").filter(|s| !s.is_empty()) {
                        out.push(NonAcpTurnUpdate::AssistantTextDelta { text: t.to_owned() });
                    }
                }
                Some("thinking") => {
                    if let Some(t) = str_field(Some(block), "thinking").filter(|s| !s.is_empty()) {
                        out.push(NonAcpTurnUpdate::ReasoningDelta { text: t.to_owned() });
                    }
                }
                Some("tool_use") => {
                    out.push(NonAcpTurnUpdate::ToolCallStarted {
                        call_id: str_field(Some(block), "id").unwrap_or("unknown").to_owned(),
                        name: str_field(Some(block), "name").unwrap_or("tool").to_owned(),
                        input_json: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                }
                _ => {}
            }
        }
    }
    if str_field(msg, "model") != Some("<synthetic>") {
        append_claude_usage(msg.and_then(|m| m.get("usage")), &mut out);
    }
    out
}

fn parse_claude_user(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let Some(blocks) = val
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| str_field(Some(b), "type") == Some("tool_result"))
        .map(|block| NonAcpTurnUpdate::ToolCallCompleted {
            call_id: str_field(Some(block), "tool_use_id")
                .unwrap_or("unknown")
                .to_owned(),
            output_json: block.get("content").cloned().unwrap_or(Value::Null),
            is_error: block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

fn parse_claude_stream_event(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let event = val.get("event");
    match str_field(event, "type") {
        Some("content_block_delta") => {
            let delta = event.and_then(|e| e.get("delta"));
            match str_field(delta, "type") {
                Some("text_delta") => str_field(delta, "text")
                    .filter(|t| !t.is_empty())
                    .map(|t| NonAcpTurnUpdate::AssistantTextDelta { text: t.to_owned() })
                    .into_iter()
                    .collect(),
                Some("thinking_delta") => str_field(delta, "thinking")
                    .filter(|t| !t.is_empty())
                    .map(|t| NonAcpTurnUpdate::ReasoningDelta { text: t.to_owned() })
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        }
        Some("message_start") => {
            let msg = event.and_then(|e| e.get("message"));
            if str_field(msg, "model") == Some("<synthetic>") {
                return Vec::new();
            }
            let mut out = Vec::new();
            append_claude_usage(msg.and_then(|m| m.get("usage")), &mut out);
            out
        }
        Some("message_delta") => {
            let mut out = Vec::new();
            append_claude_usage(event.and_then(|e| e.get("usage")), &mut out);
            out
        }
        _ => Vec::new(),
    }
}

fn parse_claude_control_request(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let req = val.get("request");
    vec![NonAcpTurnUpdate::PermissionRequested {
        request_id: str_field(Some(val), "request_id")
            .unwrap_or("unknown")
            .to_owned(),
        tool_name: str_field(req, "tool_name")
            .or_else(|| str_field(req, "subtype"))
            .unwrap_or("control")
            .to_owned(),
        detail: req.cloned().unwrap_or(Value::Null),
    }]
}

fn parse_claude_result(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let mut out = Vec::new();
    append_claude_usage(val.get("usage"), &mut out);
    let subtype = str_field(Some(val), "subtype");
    let is_error = val
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || subtype.is_some_and(|s| s.starts_with("error"));
    if is_error {
        let first_err = val
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str);
        out.push(NonAcpTurnUpdate::TurnFailed {
            message: str_field(Some(val), "result")
                .or(first_err)
                .unwrap_or("Claude turn failed")
                .to_owned(),
        });
    } else {
        out.push(NonAcpTurnUpdate::TurnCompleted {
            stop_reason: str_field(Some(val), "stop_reason")
                .or(subtype)
                .map(ToOwned::to_owned),
        });
    }
    out
}

fn append_claude_usage(usage: Option<&Value>, out: &mut Vec<NonAcpTurnUpdate>) {
    let parsed = ClaudeStreamUsage::from_message_start(usage);
    let input = parsed.total_input_tokens();
    let output = parsed.output_tokens;
    if input > 0 || output > 0 {
        out.push(NonAcpTurnUpdate::UsageReported {
            input_tokens: input,
            output_tokens: output,
        });
    }
}
