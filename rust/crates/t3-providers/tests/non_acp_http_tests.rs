//! Integration and property tests for `OpenCode` HTTP/SSE, xAI Grok HTTP/SSE,
//! and incremental [`SseStreamParser`] framing.

use proptest::prelude::*;
use t3_contracts::orchestration::ProviderApprovalDecision;
use t3_providers::non_acp::{
    DEFAULT_XAI_API_BASE, GrokChatMessage, NonAcpParseError, NonAcpTurnUpdate, SseStreamParser,
    build_grok_chat_completions_request, build_grok_models_request, build_opencode_abort_request,
    build_opencode_create_session_request, build_opencode_event_stream_request,
    build_opencode_permission_reply_request, build_opencode_prompt_request,
    parse_grok_models_response, parse_grok_sse_chunk, parse_opencode_sse_data,
};

#[test]
fn sse_stream_parser_handles_chunk_splits_multiline_data_and_done()
-> Result<(), Box<dyn std::error::Error>> {
    let mut sse = SseStreamParser::new();
    let payload = concat!(
        ": heartbeat\n",
        "event: message\n",
        "id: 42\n",
        "data: {\"line\":1,\n",
        "data: \"emoji\":\"😀\"}\n\n",
        "data: [DONE]\n\n"
    );
    let bytes = payload.as_bytes();
    // Feed in tiny chunks splitting across UTF-8 multi-byte boundaries (`😀`).
    let mut frames = Vec::new();
    for chunk in bytes.chunks(3) {
        frames.extend(sse.push_bytes(chunk)?);
    }
    frames.extend(sse.finish()?);

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].event.as_deref(), Some("message"));
    assert_eq!(frames[0].id.as_deref(), Some("42"));
    assert_eq!(frames[0].data, "{\"line\":1,\n\"emoji\":\"😀\"}");
    assert!(!frames[0].is_done);
    assert!(frames[1].is_done);
    Ok(())
}

#[test]
fn sse_stream_parser_rejects_invalid_utf8_and_flushes_eof_without_trailing_blank()
-> Result<(), Box<dyn std::error::Error>> {
    let mut bad = SseStreamParser::new();
    assert!(matches!(
        bad.push_bytes(b"data: \xff\xfe\n\n"),
        Err(NonAcpParseError::Utf8(_))
    ));

    let mut eof = SseStreamParser::new();
    assert_eq!(eof.push_bytes(b"data: {\"ok\":true}\n")?.len(), 0);
    let flushed = eof.finish()?;
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].data, "{\"ok\":true}");
    Ok(())
}

#[test]
fn opencode_http_request_builders_set_basic_auth_and_endpoints() {
    let create = build_opencode_create_session_request(
        "http://127.0.0.1:4096/",
        Some("pw"),
        Some("/repo"),
        Some("Debug"),
    );
    assert_eq!(create.method, "POST");
    assert_eq!(create.url, "http://127.0.0.1:4096/session");
    assert_eq!(
        create.headers.get("Authorization").map(String::as_str),
        Some("Basic b3BlbmNvZGU6cHc=")
    );

    let prompt = build_opencode_prompt_request(
        "http://127.0.0.1:4096",
        Some("pw"),
        "s-1",
        "Check bug",
        Some(("anthropic", "claude-sonnet-4-5")),
    );
    assert_eq!(prompt.url, "http://127.0.0.1:4096/session/s-1/message");
    assert_eq!(
        prompt
            .body
            .as_ref()
            .and_then(|b| b["model"]["providerID"].as_str()),
        Some("anthropic")
    );

    let abort = build_opencode_abort_request("http://127.0.0.1:4096", None, "s-1");
    assert_eq!(abort.url, "http://127.0.0.1:4096/session/s-1/abort");
    assert!(!abort.headers.contains_key("Authorization"));

    let perm = build_opencode_permission_reply_request(
        "http://127.0.0.1:4096",
        Some("pw"),
        "s-1",
        "perm-9",
        ProviderApprovalDecision::AcceptForSession,
    );
    assert_eq!(
        perm.body.as_ref().and_then(|b| b["response"].as_str()),
        Some("always")
    );

    let stream = build_opencode_event_stream_request("http://127.0.0.1:4096", Some("pw"), None);
    assert_eq!(stream.method, "GET");
    assert_eq!(
        stream.headers.get("Accept").map(String::as_str),
        Some("text/event-stream")
    );
}

#[test]
fn opencode_sse_data_translates_v2_and_v1_events() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_opencode_sse_data(r#"{"type":"session.text.delta","data":{"delta":"chunk-1"}}"#)?,
        vec![NonAcpTurnUpdate::AssistantTextDelta {
            text: "chunk-1".to_owned()
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(r#"{"type":"session.reasoning.delta","data":{"delta":"think"}}"#)?,
        vec![NonAcpTurnUpdate::ReasoningDelta {
            text: "think".to_owned()
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"session.tool.called","data":{"id":"tc-1","name":"bash","input":{"cmd":"ls"}}}"#
        )?,
        vec![NonAcpTurnUpdate::ToolCallStarted {
            call_id: "tc-1".to_owned(),
            name: "bash".to_owned(),
            input_json: serde_json::json!({"cmd":"ls"}),
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"session.tool.failed","data":{"id":"tc-1","error":{"message":"exit 2"}}}"#
        )?,
        vec![NonAcpTurnUpdate::ToolCallCompleted {
            call_id: "tc-1".to_owned(),
            output_json: serde_json::json!({"message":"exit 2"}),
            is_error: true,
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"permission.asked","data":{"id":"p-1","permission":"edit"}}"#
        )?
        .len(),
        1
    );
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"session.step.ended","data":{"tokens":{"input":30,"output":12,"reasoning":3}}}"#
        )?,
        vec![NonAcpTurnUpdate::UsageReported {
            input_tokens: 30,
            output_tokens: 15,
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(r#"{"type":"session.idle","properties":{}}"#)?,
        vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: Some("session.idle".to_owned())
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"session.execution.failed","data":{"error":{"message":"rate limit"}}}"#
        )?,
        vec![NonAcpTurnUpdate::TurnFailed {
            message: "rate limit".to_owned()
        }]
    );
    Ok(())
}

#[test]
fn grok_http_builds_chat_and_models_requests_and_parses_models()
-> Result<(), Box<dyn std::error::Error>> {
    let chat = build_grok_chat_completions_request(
        None,
        "xai-secret",
        "grok-4-fast-reasoning",
        &[GrokChatMessage {
            role: "user".to_owned(),
            content: "Summarize".to_owned(),
        }],
        true,
        Some(2048),
    );
    assert_eq!(
        chat.url,
        format!("{DEFAULT_XAI_API_BASE}/v1/chat/completions")
    );
    assert_eq!(
        chat.headers.get("Authorization").map(String::as_str),
        Some("Bearer xai-secret")
    );
    assert_eq!(
        chat.body
            .as_ref()
            .and_then(|b| b["stream_options"]["include_usage"].as_bool()),
        Some(true)
    );

    let models_req = build_grok_models_request(Some("https://proxy.example.com/"), "xai-secret");
    assert_eq!(models_req.method, "GET");
    assert_eq!(models_req.url, "https://proxy.example.com/v1/models");

    let catalog = parse_grok_models_response(
        r#"{"data":[{"id":"grok-4"},{"id":"grok-3-mini"},{"id":"grok-4"}]}"#,
    )?;
    assert_eq!(catalog.len(), 2);
    assert!(catalog[0].supports_reasoning);
    assert!(!catalog[1].supports_reasoning);
    Ok(())
}

#[test]
fn grok_sse_chunk_handles_done_deltas_tools_usage_and_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let role_chunk = parse_grok_sse_chunk(
        r#"{"id":"chatcmpl-1","model":"grok-4","choices":[{"delta":{"role":"assistant","reasoning_content":"ponder"}}]}"#,
    )?;
    assert_eq!(
        role_chunk,
        vec![
            NonAcpTurnUpdate::SessionStarted {
                provider_session_id: "chatcmpl-1".to_owned(),
                model: Some("grok-4".to_owned()),
            },
            NonAcpTurnUpdate::ReasoningDelta {
                text: "ponder".to_owned(),
            },
        ]
    );

    let tool_chunk = parse_grok_sse_chunk(
        r#"{"choices":[{"delta":{"content":"call","tool_calls":[{"id":"c-9","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":14}}"#,
    )?;
    assert_eq!(tool_chunk.len(), 4);
    assert_eq!(
        parse_grok_sse_chunk("[DONE]")?,
        vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: Some("stop".to_owned())
        }]
    );
    assert_eq!(
        parse_grok_sse_chunk(r#"{"error":{"message":"invalid key"}}"#)?,
        vec![NonAcpTurnUpdate::TurnFailed {
            message: "invalid key".to_owned()
        }]
    );
    assert!(parse_grok_sse_chunk("{bad").is_err());
    assert!(parse_opencode_sse_data("{bad").is_err());
    Ok(())
}

#[test]
fn opencode_v1_deltas_and_permission_reply_decisions() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"message.part.delta","properties":{"delta":"legacy"}}"#
        )?,
        vec![NonAcpTurnUpdate::AssistantTextDelta {
            text: "legacy".to_owned()
        }]
    );
    assert_eq!(
        parse_opencode_sse_data(
            r#"{"type":"permission.updated","properties":{"requestID":"pr-1","title":"bash"}}"#
        )?
        .len(),
        1
    );
    assert_eq!(
        parse_opencode_sse_data(r#"{"type":"session.execution.interrupted","data":{}}"#)?,
        vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: Some("session.execution.interrupted".to_owned())
        }]
    );
    let once = build_opencode_permission_reply_request(
        "http://localhost:4096",
        None,
        "s1",
        "p1",
        ProviderApprovalDecision::Accept,
    );
    let reject = build_opencode_permission_reply_request(
        "http://localhost:4096",
        None,
        "s1",
        "p1",
        ProviderApprovalDecision::Cancel,
    );
    assert_eq!(
        once.body.as_ref().and_then(|b| b["response"].as_str()),
        Some("once")
    );
    assert_eq!(
        reject.body.as_ref().and_then(|b| b["response"].as_str()),
        Some("reject")
    );
    Ok(())
}

proptest! {
    #[test]
    fn prop_grok_request_bears_auth_and_matches_stream_flag(
        key in "[A-Za-z0-9_-]{4,24}",
        stream in any::<bool>(),
        trailing in "/{0,3}",
    ) {
        let base = format!("https://x.example.com{trailing}");
        let spec = build_grok_chat_completions_request(
            Some(&base),
            &key,
            "grok-4",
            &[GrokChatMessage { role: "user".to_owned(), content: "hi".to_owned() }],
            stream,
            None,
        );
        prop_assert_eq!(&spec.url, "https://x.example.com/v1/chat/completions");
        let expected_auth = format!("Bearer {key}");
        prop_assert_eq!(
            spec.headers.get("Authorization").map(String::as_str),
            Some(expected_auth.as_str())
        );
        let body = spec.body.unwrap_or_default();
        prop_assert_eq!(body.get("stream").and_then(serde_json::Value::as_bool), Some(stream));
        prop_assert_eq!(body.get("stream_options").is_some(), stream);
    }
}
