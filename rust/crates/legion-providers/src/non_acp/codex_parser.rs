//! Inbound notification and JSON-RPC result parser for Codex app-server lines.

use serde_json::{Value, json};

use super::{NonAcpParseError, NonAcpTurnUpdate, id_to_string, str_field, u64_field};

/// Parses one Codex app-server NDJSON line into normalized [`NonAcpTurnUpdate`] items.
pub fn parse_codex_app_server_line(line: &str) -> Result<Vec<NonAcpTurnUpdate>, NonAcpParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let val: Value = serde_json::from_str(trimmed)?;
    let obj = val.as_object().ok_or(NonAcpParseError::MalformedFrame {
        reason: "codex frame must be a JSON object",
    })?;

    if let Some(err) = obj.get("error") {
        let msg = str_field(Some(err), "message").unwrap_or("Codex JSON-RPC error");
        return Ok(vec![NonAcpTurnUpdate::TurnFailed {
            message: msg.to_owned(),
        }]);
    }

    if let Some(method) = obj.get("method").and_then(Value::as_str) {
        return Ok(translate_codex_method(
            method,
            obj.get("id"),
            obj.get("params"),
        ));
    }

    if let Some(result) = obj.get("result")
        && let Some(thread) = result.get("thread")
        && let Some(id) = str_field(Some(thread), "id")
    {
        return Ok(vec![NonAcpTurnUpdate::SessionStarted {
            provider_session_id: id.to_owned(),
            model: str_field(Some(result), "model").map(ToOwned::to_owned),
        }]);
    }

    Ok(Vec::new())
}

fn translate_codex_method(
    method: &str,
    request_id: Option<&Value>,
    params: Option<&Value>,
) -> Vec<NonAcpTurnUpdate> {
    match method {
        "item/agentMessage/delta" => str_field(params, "delta")
            .filter(|s| !s.is_empty())
            .map(|text| NonAcpTurnUpdate::AssistantTextDelta {
                text: text.to_owned(),
            })
            .into_iter()
            .collect(),
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            str_field(params, "delta")
                .filter(|s| !s.is_empty())
                .map(|text| NonAcpTurnUpdate::ReasoningDelta {
                    text: text.to_owned(),
                })
                .into_iter()
                .collect()
        }
        "item/started" => translate_codex_item_started(params),
        "item/completed" => translate_codex_item_completed(params),
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            let call_id = str_field(params, "itemId").unwrap_or("unknown").to_owned();
            let delta = str_field(params, "delta").unwrap_or("").to_owned();
            vec![NonAcpTurnUpdate::ToolCallStarted {
                call_id,
                name: method.to_owned(),
                input_json: json!({ "delta": delta }),
            }]
        }
        "item/fileChange/patchUpdated" => {
            let call_id = str_field(params, "itemId").unwrap_or("unknown").to_owned();
            vec![NonAcpTurnUpdate::ToolCallCompleted {
                call_id,
                output_json: params.cloned().unwrap_or(Value::Null),
                is_error: false,
            }]
        }
        "execCommandApproval"
        | "applyPatchApproval"
        | "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval" => {
            let req_id = request_id.map_or_else(|| "0".to_owned(), id_to_string);
            vec![NonAcpTurnUpdate::PermissionRequested {
                request_id: req_id,
                tool_name: method.to_owned(),
                detail: params.cloned().unwrap_or(Value::Null),
            }]
        }
        "thread/tokenUsage/updated" => translate_codex_usage(params),
        "turn/completed" => translate_codex_turn_completed(params),
        _ => Vec::new(),
    }
}

fn translate_codex_item_started(params: Option<&Value>) -> Vec<NonAcpTurnUpdate> {
    let Some(item) = params.and_then(|p| p.get("item")) else {
        return Vec::new();
    };
    let kind = str_field(Some(item), "type").unwrap_or("");
    if !matches!(kind, "commandExecution" | "fileChange") {
        return Vec::new();
    }
    let call_id = str_field(Some(item), "id").unwrap_or("unknown").to_owned();
    vec![NonAcpTurnUpdate::ToolCallStarted {
        call_id,
        name: kind.to_owned(),
        input_json: item.clone(),
    }]
}

fn translate_codex_item_completed(params: Option<&Value>) -> Vec<NonAcpTurnUpdate> {
    let Some(item) = params.and_then(|p| p.get("item")) else {
        return Vec::new();
    };
    let kind = str_field(Some(item), "type").unwrap_or("");
    if !matches!(kind, "commandExecution" | "fileChange") {
        return Vec::new();
    }
    let call_id = str_field(Some(item), "id").unwrap_or("unknown").to_owned();
    let status = str_field(Some(item), "status").unwrap_or("completed");
    let exit_err = item
        .get("exitCode")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0);
    let is_error = matches!(status, "failed" | "declined") || exit_err;
    vec![NonAcpTurnUpdate::ToolCallCompleted {
        call_id,
        output_json: item.clone(),
        is_error,
    }]
}

fn translate_codex_usage(params: Option<&Value>) -> Vec<NonAcpTurnUpdate> {
    let usage = params
        .and_then(|p| p.get("tokenUsage").or(Some(p)))
        .and_then(|u| u.get("last").or(Some(u)));
    let input = u64_field(usage, "inputTokens")
        .or_else(|| u64_field(usage, "input_tokens"))
        .unwrap_or(0);
    let output = u64_field(usage, "outputTokens")
        .or_else(|| u64_field(usage, "output_tokens"))
        .unwrap_or(0);
    if input == 0 && output == 0 {
        Vec::new()
    } else {
        vec![NonAcpTurnUpdate::UsageReported {
            input_tokens: input,
            output_tokens: output,
        }]
    }
}

fn translate_codex_turn_completed(params: Option<&Value>) -> Vec<NonAcpTurnUpdate> {
    let turn = params.and_then(|p| p.get("turn"));
    let status = str_field(turn, "status").or_else(|| str_field(params, "status"));
    if status == Some("failed")
        || turn
            .and_then(|t| t.get("error"))
            .is_some_and(|e| !e.is_null())
    {
        let err_obj = turn.and_then(|t| t.get("error"));
        let message = str_field(err_obj, "message")
            .unwrap_or("Codex turn failed")
            .to_owned();
        vec![NonAcpTurnUpdate::TurnFailed { message }]
    } else {
        vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: status.map(ToOwned::to_owned),
        }]
    }
}
