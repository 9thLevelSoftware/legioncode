//! Cursor sidecar NDJSON and Pi `--mode json` event/request codecs.
//!
//! Covers Cursor's local sidecar `InteractionUpdate` envelopes
//! (`packages/provider-cursor/src/server/adapter.ts`) and Pi's `--mode json` /
//! RPC streaming event stream (`packages/provider-pi/src/server/adapter.ts`).

use serde_json::{Value, json};

use super::{NonAcpParseError, NonAcpTurnUpdate, str_field, u64_field};

/// Builds a Cursor sidecar JSON prompt/resume/summarize request envelope.
///
/// # Examples
///
/// ```
/// use legion_providers::non_acp::build_cursor_send_request;
///
/// let req = build_cursor_send_request("req-1", Some("ag-1"), "Fix lint", Some("composer-2"), false, false);
/// assert_eq!(req["type"], "send");
/// assert_eq!(req["model"], "composer-2");
/// ```
#[must_use]
pub fn build_cursor_send_request(
    request_id: &str,
    agent_id: Option<&str>,
    message: &str,
    model: Option<&str>,
    summarize: bool,
    resume_checkpoint: bool,
) -> Value {
    let mut obj = json!({
        "id": request_id,
        "type": if summarize { "summarize" } else if resume_checkpoint { "resume" } else { "send" },
        "message": message,
    });
    if let Some(map) = obj.as_object_mut() {
        if let Some(aid) = agent_id {
            map.insert("agentId".to_owned(), Value::String(aid.to_owned()));
        }
        if let Some(m) = model {
            map.insert("model".to_owned(), Value::String(m.to_owned()));
        }
    }
    obj
}

/// Builds a Cursor sidecar cancellation request envelope.
#[must_use]
pub fn build_cursor_cancel_request(request_id: &str, agent_id: &str) -> Value {
    json!({
        "id": request_id,
        "type": "cancel",
        "agentId": agent_id,
    })
}

/// Parses one Cursor sidecar NDJSON line into normalized [`NonAcpTurnUpdate`] events.
pub fn parse_cursor_sidecar_line(line: &str) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let val: Value = serde_json::from_str(trimmed)?;
    let kind = str_field(Some(&val), "type").ok_or(NonAcpParseError::MalformedFrame {
        reason: "cursor event missing string `type`",
    })?;

    Ok(match kind {
        "session" | "session-started" => {
            let sid = str_field(Some(&val), "agentId")
                .or_else(|| str_field(Some(&val), "sessionId"))
                .unwrap_or("cursor-session")
                .to_owned();
            vec![NonAcpTurnUpdate::SessionStarted {
                provider_session_id: sid,
                model: str_field(Some(&val), "model").map(ToOwned::to_owned),
            }]
        }
        "text-delta" => str_field(Some(&val), "text")
            .filter(|t| !t.is_empty())
            .map(|text| NonAcpTurnUpdate::AssistantTextDelta {
                text: text.to_owned(),
            })
            .into_iter()
            .collect(),
        "thinking-delta" => str_field(Some(&val), "text")
            .filter(|t| !t.is_empty())
            .map(|text| NonAcpTurnUpdate::ReasoningDelta {
                text: text.to_owned(),
            })
            .into_iter()
            .collect(),
        "tool-call-started" | "partial-tool-call" => {
            let call_id = str_field(Some(&val), "callId")
                .unwrap_or("unknown")
                .to_owned();
            let tool = val.get("toolCall");
            let name = str_field(tool, "type")
                .or_else(|| str_field(tool, "name"))
                .unwrap_or("tool")
                .to_owned();
            let input_json = tool.cloned().unwrap_or_else(|| json!({}));
            vec![NonAcpTurnUpdate::ToolCallStarted {
                call_id,
                name,
                input_json,
            }]
        }
        "tool-call-completed" => {
            let call_id = str_field(Some(&val), "callId")
                .unwrap_or("unknown")
                .to_owned();
            let tool = val.get("toolCall");
            let is_error = tool
                .and_then(|t| t.get("error").or_else(|| t.get("isError")))
                .is_some_and(|e| e.as_bool().unwrap_or(!e.is_null()));
            vec![NonAcpTurnUpdate::ToolCallCompleted {
                call_id,
                output_json: tool.cloned().unwrap_or(Value::Null),
                is_error,
            }]
        }
        "tool-call-delta" => val
            .get("taskUpdate")
            .and_then(|u| serde_json::to_string(u).ok())
            .and_then(|sub| parse_cursor_sidecar_line(&sub).ok())
            .unwrap_or_default(),
        "usage" => {
            let input = u64_field(Some(&val), "inputTokens").unwrap_or(0);
            let output = u64_field(Some(&val), "outputTokens").unwrap_or(0);
            vec![NonAcpTurnUpdate::UsageReported {
                input_tokens: input,
                output_tokens: output,
            }]
        }
        "turn-ended" | "result" => vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: str_field(Some(&val), "stopReason").map(ToOwned::to_owned),
        }],
        "error" => vec![NonAcpTurnUpdate::TurnFailed {
            message: str_field(Some(&val), "message")
                .unwrap_or("Cursor sidecar error")
                .to_owned(),
        }],
        _ => Vec::new(),
    })
}

/// Builds a Pi CLI RPC/`--mode json` `prompt` or `steer` command payload.
#[must_use]
pub fn build_pi_prompt_command(id: Option<&str>, message: &str, steer: bool) -> Value {
    let mut obj = json!({
        "type": if steer { "steer" } else { "prompt" },
        "message": message,
    });
    if let Some(req_id) = id
        && let Some(map) = obj.as_object_mut()
    {
        map.insert("id".to_owned(), Value::String(req_id.to_owned()));
    }
    obj
}

/// Builds a Pi CLI `abort` command payload cancelling the running agent turn.
#[must_use]
pub fn build_pi_abort_command(id: Option<&str>) -> Value {
    let mut obj = json!({ "type": "abort" });
    if let Some(req_id) = id
        && let Some(map) = obj.as_object_mut()
    {
        map.insert("id".to_owned(), Value::String(req_id.to_owned()));
    }
    obj
}

/// Parses one Pi `--mode json` / RPC NDJSON line into normalized [`NonAcpTurnUpdate`] events.
pub fn parse_pi_json_line(line: &str) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let val: Value = serde_json::from_str(trimmed)?;
    let kind = str_field(Some(&val), "type").ok_or(NonAcpParseError::MalformedFrame {
        reason: "pi event missing string `type`",
    })?;

    match kind {
        "agent_start" | "session" => Ok(parse_pi_session_start(&val)),
        "message_update" => Ok(parse_pi_message_update(&val)),
        "message_end" => Ok(parse_pi_message_end(&val)),
        "tool_execution_start" => Ok(vec![NonAcpTurnUpdate::ToolCallStarted {
            call_id: str_field(Some(&val), "toolCallId")
                .unwrap_or("unknown")
                .to_owned(),
            name: str_field(Some(&val), "toolName")
                .unwrap_or("tool")
                .to_owned(),
            input_json: val.get("args").cloned().unwrap_or_else(|| json!({})),
        }]),
        "tool_execution_end" => Ok(vec![NonAcpTurnUpdate::ToolCallCompleted {
            call_id: str_field(Some(&val), "toolCallId")
                .unwrap_or("unknown")
                .to_owned(),
            output_json: val.get("result").cloned().unwrap_or(Value::Null),
            is_error: val.get("isError").and_then(Value::as_bool).unwrap_or(false),
        }]),
        "extension_ui_request" => Ok(parse_pi_extension_prompt(&val)),
        "turn_end" | "agent_end" => Ok(vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: str_field(Some(&val), "stopReason").map(ToOwned::to_owned),
        }]),
        "error" => Ok(vec![NonAcpTurnUpdate::TurnFailed {
            message: str_field(Some(&val), "errorMessage")
                .or_else(|| str_field(Some(&val), "message"))
                .unwrap_or("Pi agent error")
                .to_owned(),
        }]),
        _ => Ok(Vec::new()),
    }
}

fn parse_pi_session_start(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let sid = str_field(Some(val), "sessionId")
        .or_else(|| str_field(Some(val), "id"))
        .unwrap_or("pi-session")
        .to_owned();
    let model = match (
        str_field(Some(val), "provider"),
        str_field(Some(val), "model"),
    ) {
        (Some(p), Some(m)) => Some(format!("{p}/{m}")),
        (None, Some(m)) => Some(m.to_owned()),
        _ => None,
    };
    vec![NonAcpTurnUpdate::SessionStarted {
        provider_session_id: sid,
        model,
    }]
}

fn parse_pi_message_update(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let mut out = Vec::new();
    let delta_ev = val.get("assistantMessageEvent");
    let delta_text = str_field(delta_ev, "delta").unwrap_or("");
    match str_field(delta_ev, "type") {
        Some("text_delta") if !delta_text.is_empty() => {
            out.push(NonAcpTurnUpdate::AssistantTextDelta {
                text: delta_text.to_owned(),
            });
        }
        Some("thinking_delta") if !delta_text.is_empty() => {
            out.push(NonAcpTurnUpdate::ReasoningDelta {
                text: delta_text.to_owned(),
            });
        }
        _ => {}
    }

    if let Some(usage) = val.get("usage") {
        let input = u64_field(Some(usage), "input")
            .or_else(|| u64_field(Some(usage), "inputTokens"))
            .unwrap_or(0);
        let output = u64_field(Some(usage), "output")
            .or_else(|| u64_field(Some(usage), "outputTokens"))
            .unwrap_or(0);
        if input > 0 || output > 0 {
            out.push(NonAcpTurnUpdate::UsageReported {
                input_tokens: input,
                output_tokens: output,
            });
        }
    }
    out
}

fn parse_pi_message_end(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let msg = val.get("message");
    if str_field(msg, "stopReason") == Some("error") {
        return vec![NonAcpTurnUpdate::TurnFailed {
            message: str_field(msg, "errorMessage")
                .unwrap_or("Pi reported a model error")
                .to_owned(),
        }];
    }
    Vec::new()
}

fn parse_pi_extension_prompt(val: &Value) -> Vec<NonAcpTurnUpdate> {
    let method = str_field(Some(val), "method").unwrap_or("");
    if !matches!(method, "confirm" | "select") {
        return Vec::new();
    }
    let req_id = str_field(Some(val), "id").unwrap_or("pi-ui").to_owned();
    let tool_name = str_field(Some(val), "title").unwrap_or(method).to_owned();
    vec![NonAcpTurnUpdate::PermissionRequested {
        request_id: req_id,
        tool_name,
        detail: val.clone(),
    }]
}
