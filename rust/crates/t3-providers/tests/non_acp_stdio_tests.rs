//! Integration tests for Codex app-server and Claude `stream-json` non-ACP stdio codecs.

use serde_json::{Value, json};
use t3_contracts::orchestration::ProviderApprovalDecision;
use t3_providers::non_acp::{
    CodexAppServerCodec, CodexApprovalDecision, CodexThreadStartParams, CodexTurnStartParams,
    NonAcpParseError, NonAcpTurnUpdate, build_claude_control_response,
    build_claude_interrupt_control_request, build_claude_permission_control_response,
    build_claude_user_prompt_envelope, codex_decision_from_approval,
    encode_apply_patch_approval_response, encode_claude_user_prompt_line,
    encode_exec_command_approval_response, parse_claude_stream_json_line,
    parse_codex_app_server_line,
};

#[test]
fn codex_app_server_request_builders_omit_jsonrpc_and_cover_lifecycle()
-> Result<(), Box<dyn std::error::Error>> {
    let mut codec = CodexAppServerCodec::new();
    let (id1, init) = codec.encode_initialize("t3-code", "T3 Code", "0.1.0", true)?;
    let initialized = codec.encode_initialized()?;
    let (id2, thread) = codec.encode_thread_start(&CodexThreadStartParams {
        cwd: "/repo".to_owned(),
        model: Some("gpt-5.4".to_owned()),
        approval_policy: "on-request".to_owned(),
        sandbox: "workspace-write".to_owned(),
        experimental_raw_events: false,
    })?;
    let (id3, turn) = codec.encode_turn_start(&CodexTurnStartParams {
        thread_id: "thr-1".to_owned(),
        text: "Run tests".to_owned(),
        effort: Some("high".to_owned()),
    })?;
    let (id4, steer) = codec.encode_turn_steer("thr-1", "turn-1", "Check clippy too")?;
    let (id5, interrupt) = codec.encode_turn_interrupt("thr-1", "turn-1")?;

    assert_eq!((id1, id2, id3, id4, id5), (1, 2, 3, 4, 5));
    let v_init: Value = serde_json::from_str(init.trim())?;
    assert!(v_init.get("jsonrpc").is_none());
    assert_eq!(v_init["method"], "initialize");
    assert_eq!(
        serde_json::from_str::<Value>(initialized.trim())?["method"],
        "initialized"
    );
    assert_eq!(
        serde_json::from_str::<Value>(thread.trim())?["method"],
        "thread/start"
    );
    assert_eq!(
        serde_json::from_str::<Value>(turn.trim())?["method"],
        "turn/start"
    );
    assert_eq!(
        serde_json::from_str::<Value>(steer.trim())?["method"],
        "turn/steer"
    );
    assert_eq!(
        serde_json::from_str::<Value>(interrupt.trim())?["method"],
        "turn/interrupt"
    );
    Ok(())
}

#[test]
fn codex_app_server_approval_responses_map_decisions() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        codex_decision_from_approval(ProviderApprovalDecision::Accept),
        CodexApprovalDecision::Approved
    );
    assert_eq!(
        codex_decision_from_approval(ProviderApprovalDecision::AcceptForSession),
        CodexApprovalDecision::ApprovedForSession
    );
    assert_eq!(
        codex_decision_from_approval(ProviderApprovalDecision::Decline),
        CodexApprovalDecision::Denied
    );
    assert_eq!(
        codex_decision_from_approval(ProviderApprovalDecision::Cancel),
        CodexApprovalDecision::Abort
    );

    let cmd_reply =
        encode_exec_command_approval_response(&json!(77), CodexApprovalDecision::Approved)?;
    let patch_reply =
        encode_apply_patch_approval_response(&json!("req-8"), CodexApprovalDecision::Denied)?;
    assert_eq!(
        serde_json::from_str::<Value>(cmd_reply.trim())?["result"]["decision"],
        "approved"
    );
    assert_eq!(
        serde_json::from_str::<Value>(patch_reply.trim())?["id"],
        "req-8"
    );
    Ok(())
}

#[test]
fn codex_app_server_parses_deltas_tools_approvals_usage_and_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let mut codec = CodexAppServerCodec::new();
    let stream = concat!(
        "{\"id\":2,\"result\":{\"thread\":{\"id\":\"thr-9\"},\"model\":\"gpt-5.4\"}}\n",
        "{\"method\":\"item/reasoning/summaryTextDelta\",\"params\":{\"delta\":\"Thinking...\"}}\n",
        "{\"method\":\"item/agentMessage/delta\",\"params\":{\"delta\":\"Patching\"}}\n",
        "{\"method\":\"item/started\",\"params\":{\"item\":{\"id\":\"cmd-1\",\"type\":\"commandExecution\",\"command\":\"cargo test\"}}}\n",
        "{\"id\":91,\"method\":\"execCommandApproval\",\"params\":{\"command\":\"rm -rf tmp\"}}\n",
        "{\"method\":\"item/completed\",\"params\":{\"item\":{\"id\":\"fc-1\",\"type\":\"fileChange\",\"status\":\"failed\",\"exitCode\":1}}}\n",
        "{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"tokenUsage\":{\"last\":{\"inputTokens\":120,\"outputTokens\":45}}}}\n",
        "{\"method\":\"turn/completed\",\"params\":{\"turn\":{\"status\":\"completed\"}}}\n"
    );
    let events = codec.push_bytes(stream.as_bytes())?;
    assert_eq!(
        events,
        vec![
            NonAcpTurnUpdate::SessionStarted {
                provider_session_id: "thr-9".to_owned(),
                model: Some("gpt-5.4".to_owned()),
            },
            NonAcpTurnUpdate::ReasoningDelta {
                text: "Thinking...".to_owned(),
            },
            NonAcpTurnUpdate::AssistantTextDelta {
                text: "Patching".to_owned(),
            },
            NonAcpTurnUpdate::ToolCallStarted {
                call_id: "cmd-1".to_owned(),
                name: "commandExecution".to_owned(),
                input_json: json!({"id":"cmd-1","type":"commandExecution","command":"cargo test"}),
            },
            NonAcpTurnUpdate::PermissionRequested {
                request_id: "91".to_owned(),
                tool_name: "execCommandApproval".to_owned(),
                detail: json!({"command":"rm -rf tmp"}),
            },
            NonAcpTurnUpdate::ToolCallCompleted {
                call_id: "fc-1".to_owned(),
                output_json: json!({"id":"fc-1","type":"fileChange","status":"failed","exitCode":1}),
                is_error: true,
            },
            NonAcpTurnUpdate::UsageReported {
                input_tokens: 120,
                output_tokens: 45,
            },
            NonAcpTurnUpdate::TurnCompleted {
                stop_reason: Some("completed".to_owned()),
            },
        ]
    );
    Ok(())
}

#[test]
fn codex_app_server_parses_failed_turn_and_rejects_malformed_json() {
    let failed = parse_codex_app_server_line(
        r#"{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"message":"sandbox error"}}}}"#,
    );
    assert_eq!(
        failed.ok(),
        Some(vec![NonAcpTurnUpdate::TurnFailed {
            message: "sandbox error".to_owned()
        }])
    );
    assert!(matches!(
        parse_codex_app_server_line("{bad json"),
        Err(NonAcpParseError::Json(_))
    ));
}

#[test]
fn claude_stream_json_prompt_image_blocks_and_control_replies()
-> Result<(), Box<dyn std::error::Error>> {
    let prompt_env =
        build_claude_user_prompt_envelope("Analyze image", &[("image/png", "AA==")], Some("s-7"));
    assert_eq!(prompt_env["type"], "user");
    assert_eq!(
        prompt_env["message"]["content"][1]["source"]["media_type"],
        "image/png"
    );
    let line = encode_claude_user_prompt_line("Hi", &[], None)?;
    assert!(line.ends_with('\n'));

    let ok_ctrl = build_claude_control_response("req-1", Ok(json!({"added": []})));
    assert_eq!(ok_ctrl["response"]["subtype"], "success");
    let allow = build_claude_permission_control_response(
        "req-2",
        ProviderApprovalDecision::Accept,
        Some(json!({"command":"ls"})),
    );
    assert_eq!(allow["response"]["response"]["behavior"], "allow");
    let cancel =
        build_claude_permission_control_response("req-3", ProviderApprovalDecision::Cancel, None);
    assert_eq!(cancel["response"]["response"]["interrupt"], true);
    let interrupt = build_claude_interrupt_control_request("req-4");
    assert_eq!(interrupt["request"]["subtype"], "interrupt");
    Ok(())
}

#[test]
fn claude_stream_json_parses_system_assistant_user_control_and_result()
-> Result<(), Box<dyn std::error::Error>> {
    let init = parse_claude_stream_json_line(
        r#"{"type":"system","subtype":"init","session_id":"cs-1","model":"claude-sonnet-4-5"}"#,
    )?;
    assert_eq!(
        init,
        vec![NonAcpTurnUpdate::SessionStarted {
            provider_session_id: "cs-1".to_owned(),
            model: Some("claude-sonnet-4-5".to_owned()),
        }]
    );

    let asst = parse_claude_stream_json_line(
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"plan"},{"type":"text","text":"ok"},{"type":"tool_use","id":"tu-1","name":"Bash","input":{"command":"pwd"}}],"usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":7}}}"#,
    )?;
    assert_eq!(asst.len(), 4);
    assert!(matches!(&asst[0], NonAcpTurnUpdate::ReasoningDelta { text } if text == "plan"));
    assert!(
        matches!(&asst[2], NonAcpTurnUpdate::ToolCallStarted { call_id, .. } if call_id == "tu-1")
    );

    let usr = parse_claude_stream_json_line(
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu-1","content":"/repo","is_error":false}]}}"#,
    )?;
    assert_eq!(
        usr,
        vec![NonAcpTurnUpdate::ToolCallCompleted {
            call_id: "tu-1".to_owned(),
            output_json: json!("/repo"),
            is_error: false,
        }]
    );

    let perm = parse_claude_stream_json_line(
        r#"{"type":"control_request","request_id":"cr-5","request":{"subtype":"can_use_tool","tool_name":"Write"}}"#,
    )?;
    assert!(matches!(
        &perm[0],
        NonAcpTurnUpdate::PermissionRequested { request_id, tool_name, .. }
        if request_id == "cr-5" && tool_name == "Write"
    ));

    let res = parse_claude_stream_json_line(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["boom"]}"#,
    )?;
    assert_eq!(
        res,
        vec![NonAcpTurnUpdate::TurnFailed {
            message: "boom".to_owned()
        }]
    );
    Ok(())
}

#[test]
fn codex_json_rpc_error_item_deltas_and_eof_finish() -> Result<(), Box<dyn std::error::Error>> {
    let mut codec = CodexAppServerCodec::new();
    let rpc_err =
        parse_codex_app_server_line(r#"{"id":1,"error":{"code":-32600,"message":"bad request"}}"#)?;
    assert_eq!(
        rpc_err,
        vec![NonAcpTurnUpdate::TurnFailed {
            message: "bad request".to_owned()
        }]
    );
    let delta = parse_codex_app_server_line(
        r#"{"method":"item/commandExecution/outputDelta","params":{"itemId":"cmd-2","delta":"ok\n"}}"#,
    )?;
    assert!(matches!(
        &delta[0],
        NonAcpTurnUpdate::ToolCallStarted { call_id, name, .. }
        if call_id == "cmd-2" && name == "item/commandExecution/outputDelta"
    ));
    let patch = parse_codex_app_server_line(
        r#"{"method":"item/fileChange/patchUpdated","params":{"itemId":"fc-2","diff":"+hi"}}"#,
    )?;
    assert!(matches!(
        &patch[0],
        NonAcpTurnUpdate::ToolCallCompleted { call_id, is_error: false, .. } if call_id == "fc-2"
    ));
    assert_eq!(
        codec
            .push_bytes(br#"{"method":"item/agentMessage/delta","params":{"delta":"eof"}}"#)?
            .len(),
        0
    );
    assert_eq!(
        codec.finish()?,
        vec![NonAcpTurnUpdate::AssistantTextDelta {
            text: "eof".to_owned()
        }]
    );
    Ok(())
}

#[test]
fn claude_stream_event_deltas_emit_text_and_reasoning() -> Result<(), Box<dyn std::error::Error>> {
    let td = parse_claude_stream_json_line(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#,
    )?;
    let rd = parse_claude_stream_json_line(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
    )?;
    assert_eq!(
        td,
        vec![NonAcpTurnUpdate::AssistantTextDelta {
            text: "hi".to_owned()
        }]
    );
    assert_eq!(
        rd,
        vec![NonAcpTurnUpdate::ReasoningDelta {
            text: "hmm".to_owned()
        }]
    );
    Ok(())
}
