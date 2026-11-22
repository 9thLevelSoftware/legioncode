//! Integration and property tests for Cursor sidecar, Pi `--mode json`,
//! and malformed non-ACP NDJSON resilience.

use legion_providers::non_acp::{
    NonAcpParseError, NonAcpTurnUpdate, SseStreamParser, build_cursor_cancel_request,
    build_cursor_send_request, build_pi_abort_command, build_pi_prompt_command,
    parse_claude_stream_json_line, parse_codex_app_server_line, parse_cursor_sidecar_line,
    parse_grok_models_response, parse_grok_sse_chunk, parse_opencode_sse_data, parse_pi_json_line,
};
use proptest::prelude::*;
use serde_json::json;

#[test]
fn cursor_sidecar_builds_requests_and_parses_events() -> Result<(), Box<dyn std::error::Error>> {
    let send =
        build_cursor_send_request("r1", Some("ag-1"), "Fix", Some("composer-2"), false, false);
    assert_eq!(send["type"], "send");
    let cancel = build_cursor_cancel_request("r2", "ag-1");
    assert_eq!(cancel["type"], "cancel");

    assert_eq!(
        parse_cursor_sidecar_line(r#"{"type":"text-delta","text":"hi"}"#)?,
        vec![NonAcpTurnUpdate::AssistantTextDelta {
            text: "hi".to_owned()
        }]
    );
    assert_eq!(
        parse_cursor_sidecar_line(
            r#"{"type":"tool-call-started","callId":"c1","toolCall":{"type":"shell","command":"ls"}}"#
        )?,
        vec![NonAcpTurnUpdate::ToolCallStarted {
            call_id: "c1".to_owned(),
            name: "shell".to_owned(),
            input_json: json!({"type":"shell","command":"ls"}),
        }]
    );
    assert_eq!(
        parse_cursor_sidecar_line(
            r#"{"type":"tool-call-delta","callId":"sub1","taskUpdate":{"type":"thinking-delta","text":"sub"}}"#
        )?,
        vec![NonAcpTurnUpdate::ReasoningDelta {
            text: "sub".to_owned()
        }]
    );
    assert_eq!(
        parse_cursor_sidecar_line(r#"{"type":"turn-ended","stopReason":"end_turn"}"#)?,
        vec![NonAcpTurnUpdate::TurnCompleted {
            stop_reason: Some("end_turn".to_owned())
        }]
    );
    Ok(())
}

#[test]
fn pi_json_builds_commands_and_parses_events() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        build_pi_prompt_command(Some("p1"), "Go", false)["type"],
        "prompt"
    );
    assert_eq!(
        build_pi_prompt_command(None, "Steer", true)["type"],
        "steer"
    );
    assert_eq!(build_pi_abort_command(Some("p2"))["type"], "abort");

    assert_eq!(
        parse_pi_json_line(
            r#"{"type":"agent_start","sessionId":"pi-1","provider":"anthropic","model":"sonnet"}"#
        )?,
        vec![NonAcpTurnUpdate::SessionStarted {
            provider_session_id: "pi-1".to_owned(),
            model: Some("anthropic/sonnet".to_owned()),
        }]
    );
    let mu = parse_pi_json_line(
        r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hello"},"usage":{"input":12,"output":4}}"#,
    )?;
    assert_eq!(mu.len(), 2);
    assert!(matches!(
        &mu[1],
        NonAcpTurnUpdate::UsageReported {
            input_tokens: 12,
            output_tokens: 4
        }
    ));

    assert_eq!(
        parse_pi_json_line(
            r#"{"type":"tool_execution_end","toolCallId":"t1","result":"fail","isError":true}"#
        )?,
        vec![NonAcpTurnUpdate::ToolCallCompleted {
            call_id: "t1".to_owned(),
            output_json: json!("fail"),
            is_error: true,
        }]
    );
    assert_eq!(
        parse_pi_json_line(
            r#"{"type":"extension_ui_request","id":"ui-1","method":"confirm","title":"Run bash?"}"#
        )?
        .len(),
        1
    );
    Ok(())
}

#[test]
fn pi_message_end_error_reports_turn_failed() -> Result<(), Box<dyn std::error::Error>> {
    let pi_err = parse_pi_json_line(
        r#"{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"context limit"}}"#,
    )?;
    assert_eq!(
        pi_err,
        vec![NonAcpTurnUpdate::TurnFailed {
            message: "context limit".to_owned()
        }]
    );
    Ok(())
}

#[test]
fn non_acp_stdio_parsers_reject_malformed_frames_gracefully() {
    assert!(parse_claude_stream_json_line("{not json").is_err());
    assert!(matches!(
        parse_claude_stream_json_line(r#"{"missing_type":true}"#),
        Err(NonAcpParseError::MalformedFrame { .. })
    ));
    assert!(matches!(
        parse_cursor_sidecar_line(r#"{"type":123}"#),
        Err(NonAcpParseError::MalformedFrame { .. })
    ));
    assert!(parse_pi_json_line("[\"array\"]").is_err());
}

proptest! {
    #[test]
    fn prop_non_acp_stdio_parsers_never_panic_on_arbitrary_strings(
        raw in "\\PC{0,180}"
    ) {
        let _ = parse_codex_app_server_line(&raw);
        let _ = parse_claude_stream_json_line(&raw);
        let _ = parse_cursor_sidecar_line(&raw);
        let _ = parse_pi_json_line(&raw);
    }

    #[test]
    fn prop_sse_parser_matches_single_shot_regardless_of_chunk_size(
        split in 1usize..32,
        msg_a in "[a-zA-Z0-9 ]{1,30}",
        msg_b in "[a-zA-Z0-9 ]{1,30}",
    ) {
        let stream = format!("data: {{\"a\":\"{msg_a}\"}}\n\ndata: {{\"b\":\"{msg_b}\"}}\n\ndata: [DONE]\n\n");
        let mut whole = SseStreamParser::new();
        let mut expected = whole.push_bytes(stream.as_bytes()).unwrap_or_default();
        expected.extend(whole.finish().unwrap_or_default());

        let mut chunked = SseStreamParser::new();
        let mut actual = Vec::new();
        for piece in stream.as_bytes().chunks(split) {
            actual.extend(chunked.push_bytes(piece).unwrap_or_default());
        }
        actual.extend(chunked.finish().unwrap_or_default());
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn prop_http_sse_parsers_never_panic_on_arbitrary_input(raw in "\\PC{0,180}") {
        let _ = parse_opencode_sse_data(&raw);
        let _ = parse_grok_sse_chunk(&raw);
        let _ = parse_grok_models_response(&raw);
    }
}
