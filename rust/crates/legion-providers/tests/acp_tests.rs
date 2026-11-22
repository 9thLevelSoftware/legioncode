//! Integration tests for ACP JSON-RPC 2.0 wire framing, session updates,
//! permission policy, and tool-call presentation normalization.

use legion_contracts::orchestration::{
    ProviderApprovalDecision, ProviderInteractionMode, is_known_turn_item_type,
};
use legion_contracts::policy::RuntimeMode;
use legion_providers::acp::{
    ACP_PROTOCOL_VERSION, AcpCodecError, AcpStdioCodec, AcpToolCallState, CancelNotification,
    ClientCapabilities, EnvNameValue, ImplementationInfo, InitializeRequest, InitializeResponse,
    JsonRpcEnvelope, JsonRpcId, METHOD_INITIALIZE, METHOD_SESSION_CANCEL, McpServerSpec,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind,
    PromptContentBlock, PromptRequest, PromptResponse, RPC_REQUEST_ID_START,
    RequestPermissionOutcome, RequestPermissionRequest, SessionUpdateKind,
    SessionUpdateNotification, StopReason, TOOL_CALL_CONTENT_MAX_CHARS, ToolActivityCategory,
    ToolCallKind, ToolCallLocation, ToolCallStatus, ToolCallUpdatePayload,
    extract_mcp_tool_identity, map_tool_call_presentation, select_permission_option,
    should_emit_tool_call_update,
};
use serde_json::json;

#[test]
fn stdio_codec_handles_fragmented_ndjson_and_monotonic_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let mut codec = AcpStdioCodec::new();
    let init = InitializeRequest {
        protocol_version: ACP_PROTOCOL_VERSION,
        client_capabilities: ClientCapabilities::default(),
        client_info: Some(ImplementationInfo {
            name: "legioncode".to_owned(),
            title: Some("LegionCode".to_owned()),
            version: "0.1.0".to_owned(),
        }),
        meta: None,
    };

    let (id1, req_line) = codec.encode_request(METHOD_INITIALIZE, Some(&init))?;
    let (id2, notif_test) =
        codec.encode_request("session/new", Some(&json!({ "cwd": "/repo" })))?;
    assert_eq!(id1, JsonRpcId::Number(RPC_REQUEST_ID_START));
    assert_eq!(id2, JsonRpcId::Number(RPC_REQUEST_ID_START + 1));

    let resp_line = codec.encode_response_ok(
        id1.clone(),
        json!({ "protocolVersion": 1, "authMethods": [] }),
    )?;
    let combined = format!("{req_line}{resp_line}{notif_test}");
    let bytes = combined.as_bytes();

    // Feed arbitrary mid-line splits to ensure partial NDJSON buffering waits for `\n`.
    let split_a = 11;
    let split_b = req_line.len() + 7;
    assert_eq!(codec.push_bytes(&bytes[..split_a])?.len(), 0);
    assert_eq!(codec.pending_bytes(), split_a);

    let batch_1 = codec.push_bytes(&bytes[split_a..split_b])?;
    assert_eq!(batch_1.len(), 1);
    assert!(matches!(&batch_1[0], JsonRpcEnvelope::Request(r) if r.id == id1));

    let batch_2 = codec.push_bytes(&bytes[split_b..])?;
    assert_eq!(batch_2.len(), 2);
    assert!(matches!(&batch_2[0], JsonRpcEnvelope::Response(r) if r.id == id1));
    assert!(matches!(&batch_2[1], JsonRpcEnvelope::Request(r) if r.id == id2));
    assert_eq!(codec.pending_bytes(), 0);
    Ok(())
}

#[test]
fn stdio_codec_rejects_wrong_jsonrpc_version_and_flushes_eof_notification()
-> Result<(), Box<dyn std::error::Error>> {
    let mut codec = AcpStdioCodec::new();
    let bad = br#"{"jsonrpc":"1.0","id":1,"method":"initialize"}"#;
    let mut bad_line = bad.to_vec();
    bad_line.push(b'\n');
    let err = codec
        .push_bytes(&bad_line)
        .err()
        .ok_or("expected InvalidVersion error")?;
    assert!(matches!(
        err,
        AcpCodecError::InvalidVersion {
            found: Some(ref v)
        } if v == "1.0"
    ));

    // Unterminated final frame without trailing `\n` flushes cleanly at EOF.
    let cancel = CancelNotification {
        session_id: "sess-42".to_owned(),
        meta: None,
    };
    let line = codec.encode_envelope(&cancel.into_envelope()?)?;
    let without_nl = line.trim_end_matches('\n');
    assert_eq!(codec.push_bytes(without_nl.as_bytes())?.len(), 0);
    let flushed = codec.finish()?.ok_or("expected EOF notification")?;
    assert!(matches!(
        flushed,
        JsonRpcEnvelope::Notification(n) if n.method == METHOD_SESSION_CANCEL
    ));
    Ok(())
}

#[test]
fn session_update_decodes_known_and_unknown_variants_round_trip()
-> Result<(), Box<dyn std::error::Error>> {
    let chunk_json = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "hello world" }
        }
    });
    let decoded: SessionUpdateNotification = serde_json::from_value(chunk_json)?;
    assert_eq!(
        decoded.update,
        SessionUpdateKind::AgentMessageChunk(PromptContentBlock::Text {
            text: "hello world".to_owned()
        })
    );

    let future_json = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "future_telemetry_v9",
            "payload": { "score": 99 }
        }
    });
    let unknown: SessionUpdateNotification = serde_json::from_value(future_json)?;
    assert!(matches!(
        &unknown.update,
        SessionUpdateKind::Unknown { session_update, .. } if session_update == "future_telemetry_v9"
    ));

    let re_encoded = serde_json::to_value(&unknown)?;
    let round_tripped: SessionUpdateNotification = serde_json::from_value(re_encoded)?;
    assert_eq!(round_tripped, unknown);

    let stop: PromptResponse = serde_json::from_value(json!({ "stopReason": "cancelled" }))?;
    assert_eq!(stop.stop_reason, StopReason::Cancelled);
    Ok(())
}

#[test]
fn tool_call_state_merge_caps_content_and_throttles_updates() {
    let initial = ToolCallUpdatePayload {
        tool_call_id: "tc-9".to_owned(),
        title: Some("Read workspace manifest".to_owned()),
        kind: Some(ToolCallKind::Read),
        status: Some(ToolCallStatus::InProgress),
        locations: Some(vec![ToolCallLocation {
            path: "Cargo.toml".to_owned(),
            line: Some(1),
        }]),
        content: Some(vec![
            json!({ "text": "a".repeat(TOOL_CALL_CONTENT_MAX_CHARS + 500) }),
        ]),
        raw_input: Some(json!({ "path": "Cargo.toml" })),
        raw_output: None,
    };

    let mut state = AcpToolCallState::from_payload(initial);
    assert_eq!(
        state.content_preview.chars().count(),
        TOOL_CALL_CONTENT_MAX_CHARS
    );

    // Empty title / empty locations must not erase prior valid fields.
    let no_op_emit = state.merge(ToolCallUpdatePayload {
        tool_call_id: "tc-9".to_owned(),
        title: Some("   ".to_owned()),
        kind: None,
        status: None,
        locations: Some(Vec::new()),
        content: None,
        raw_input: None,
        raw_output: None,
    });
    assert!(!no_op_emit);
    assert_eq!(state.title, "Read workspace manifest");
    assert_eq!(state.locations.len(), 1);

    // Coalescing triggers at 10 updates even without size growth.
    assert!(should_emit_tool_call_update(100, 100, 10, false));
    assert!(!should_emit_tool_call_update(100, 150, 3, false));
    assert!(should_emit_tool_call_update(100, 400, 3, false));

    let mcp_state = AcpToolCallState::from_payload(ToolCallUpdatePayload {
        tool_call_id: "tc-mcp".to_owned(),
        title: Some("mcp:linear:create_issue".to_owned()),
        kind: Some(ToolCallKind::Execute),
        status: Some(ToolCallStatus::Failed),
        locations: None,
        content: None,
        raw_input: None,
        raw_output: None,
    });
    let view = map_tool_call_presentation(&mcp_state);
    assert_eq!(view.category, ToolActivityCategory::McpTool);
    assert!(is_known_turn_item_type(view.turn_item_type));
    assert_eq!(view.subtitle.as_deref(), Some("linear/create_issue"));
    assert!(view.failed);
    assert!(extract_mcp_tool_identity("Edit src/lib.rs").is_none());
    assert!(extract_mcp_tool_identity("/usr/bin/env").is_none());
}

#[test]
fn permission_policy_enforces_plan_read_only_and_explicit_decisions() {
    let build_req = |kind: ToolCallKind| RequestPermissionRequest {
        session_id: "s-perm".to_owned(),
        tool_call: ToolCallUpdatePayload {
            tool_call_id: "tc-p".to_owned(),
            title: Some("Run action".to_owned()),
            kind: Some(kind),
            status: None,
            locations: None,
            content: None,
            raw_input: None,
            raw_output: None,
        },
        options: vec![
            PermissionOption {
                option_id: "once".to_owned(),
                name: "Allow once".to_owned(),
                kind: PermissionOptionKind::AllowOnce,
            },
            PermissionOption {
                option_id: "always".to_owned(),
                name: "Allow always".to_owned(),
                kind: PermissionOptionKind::AllowAlways,
            },
            PermissionOption {
                option_id: "deny".to_owned(),
                name: "Deny".to_owned(),
                kind: PermissionOptionKind::RejectOnce,
            },
        ],
    };

    // Plan mode denies mutations even under FullAccess.
    let mutating_resp = select_permission_option(
        &build_req(ToolCallKind::Execute),
        Some(ProviderApprovalDecision::Accept),
        Some(RuntimeMode::FullAccess),
        Some(ProviderInteractionMode::Plan),
    );
    assert_eq!(
        mutating_resp.outcome,
        RequestPermissionOutcome::Selected {
            option_id: "deny".to_owned()
        }
    );

    // AutoAcceptEdits permits Edit but rejects Execute when decision is None.
    let edit_resp = select_permission_option(
        &build_req(ToolCallKind::Edit),
        None,
        Some(RuntimeMode::AutoAcceptEdits),
        Some(ProviderInteractionMode::Default),
    );
    assert_eq!(
        edit_resp.outcome,
        RequestPermissionOutcome::Selected {
            option_id: "once".to_owned()
        }
    );

    let exec_resp = select_permission_option(
        &build_req(ToolCallKind::Execute),
        None,
        Some(RuntimeMode::AutoAcceptEdits),
        Some(ProviderInteractionMode::Default),
    );
    assert_eq!(
        exec_resp.outcome,
        RequestPermissionOutcome::Selected {
            option_id: "deny".to_owned()
        }
    );

    let cancel_resp = select_permission_option(
        &build_req(ToolCallKind::Read),
        Some(ProviderApprovalDecision::Cancel),
        Some(RuntimeMode::FullAccess),
        None,
    );
    assert_eq!(cancel_resp.outcome, RequestPermissionOutcome::Cancelled);
}

#[test]
fn new_session_and_prompt_envelopes_preserve_mcp_transports_and_images()
-> Result<(), Box<dyn std::error::Error>> {
    let new_req = NewSessionRequest {
        cwd: "/workspace/legioncode".to_owned(),
        mcp_servers: vec![
            McpServerSpec::Stdio {
                name: "fs".to_owned(),
                command: "npx".to_owned(),
                args: vec![
                    "-y".to_owned(),
                    "@modelcontextprotocol/server-filesystem".to_owned(),
                ],
                env: vec![EnvNameValue {
                    name: "DEBUG".to_owned(),
                    value: "0".to_owned(),
                }],
            },
            McpServerSpec::Http {
                name: "remote".to_owned(),
                url: "https://mcp.example.com".to_owned(),
                headers: Vec::new(),
            },
        ],
        meta: Some(json!({ "fx": { "profileMcpServers": true } })),
    };

    let mut codec = AcpStdioCodec::new();
    let (_id, line) = codec.encode_request("session/new", Some(&new_req))?;
    let decoded_envs = codec.push_bytes(line.as_bytes())?;
    assert_eq!(decoded_envs.len(), 1);
    let JsonRpcEnvelope::Request(req) = &decoded_envs[0] else {
        return Err("expected request".into());
    };
    let parsed_req: NewSessionRequest =
        serde_json::from_value(req.params.clone().ok_or("missing params")?)?;
    assert_eq!(parsed_req, new_req);

    let init_resp: InitializeResponse = serde_json::from_value(json!({
        "protocolVersion": 1,
        "authMethods": [{ "id": "oauth" }],
        "_meta": { "clientType": "extension" }
    }))?;
    assert_eq!(init_resp.protocol_version, 1);

    let new_resp: NewSessionResponse = serde_json::from_value(json!({
        "sessionId": "acp-sess-1",
        "modes": { "currentModeId": "default" }
    }))?;
    assert_eq!(new_resp.session_id, "acp-sess-1");

    let prompt = PromptRequest {
        session_id: new_resp.session_id,
        prompt: vec![
            PromptContentBlock::Text {
                text: "Check UI screenshot".to_owned(),
            },
            PromptContentBlock::Image {
                mime_type: "image/png".to_owned(),
                data: "iVBORw0KGgo=".to_owned(),
            },
        ],
        meta: None,
    };
    let round: PromptRequest = serde_json::from_value(serde_json::to_value(&prompt)?)?;
    assert_eq!(round, prompt);
    Ok(())
}

#[test]
fn stdio_codec_enforces_max_line_byte_limit() {
    let mut codec = AcpStdioCodec::with_max_line_bytes(32);
    let oversized =
        b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"x\":123456}}\n";
    let err = codec.push_bytes(oversized);
    assert!(err.is_err());
}
