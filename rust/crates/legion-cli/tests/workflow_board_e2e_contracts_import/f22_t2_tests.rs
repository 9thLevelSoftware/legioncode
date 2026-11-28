//! F22 Tier 2 opaque-box Rust integration tests (`[F22-T2-04]`..`[F22-T2-06]`).

use std::net::SocketAddr;

use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcCauseReason, RpcExit, RpcId, ServerRpcFrame, decode_server_frames,
    encode_client_frame,
};
use legion_shared::frame_parts::{PART_PREFIX, PART_RECEIVED_FRAME, split_socket_frame};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::helpers::{
    board_capability_enabled, call_mcp_tool, ensure_project_added, extract_card_id,
    extract_thread_id, mint_scoped_session, post_mcp_raw_accept, read_ws_text_frame,
    write_ws_masked_text,
};
use super::workflow_board_e2e::TestResult;
use super::workflow_board_e2e::harness::OpaqueBoxHarness;
use super::workflow_board_e2e::ws_rpc::OpaqueWsClient;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f22_t2_04_mcp_http_boundary_and_thread_scope_isolation() -> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    let tools_list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });

    let evil_origin = harness
        .post_mcp(None, &[("Origin", "https://evil.example")], &tools_list)
        .await?;
    assert_eq!(evil_origin.status().as_u16(), 403);

    let bad_accept = post_mcp_raw_accept(harness.origin()?, "text/plain", &tools_list).await?;
    assert_eq!(bad_accept.status().as_u16(), 406);

    let unauth = harness.post_mcp(None, &[], &tools_list).await?;
    assert_eq!(unauth.status().as_u16(), 401);

    let token = harness.pair_client("mcp-thread-boundary", None).await?;
    let list_resp = harness
        .post_mcp(
            Some(&token),
            &[("x-legioncode-thread-id", "thr-scope-1")],
            &tools_list,
        )
        .await?;
    assert_eq!(list_resp.status().as_u16(), 200);
    let list_json: Value = list_resp.json().await?;
    let has_hidden_workflow = list_json["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|t| t.get("name").and_then(Value::as_str) == Some("legion_workflow"));
    assert!(!has_hidden_workflow);

    for (idx, tool_name) in [
        "legion_workflow",
        "legion_workflow_board_approve",
        "workflowBoard.approveDelivery",
    ]
    .into_iter()
    .enumerate()
    {
        let call_resp = harness
            .post_mcp(
                Some(&token),
                &[("x-legioncode-thread-id", "thr-scope-1")],
                &json!({
                    "jsonrpc": "2.0",
                    "id": idx + 10,
                    "method": "tools/call",
                    "params": { "name": tool_name, "arguments": { "cardId": "c1" } }
                }),
            )
            .await?;
        assert_eq!(call_resp.status().as_u16(), 200);
        let body: Value = call_resp.json().await?;
        assert_eq!(
            body["result"]["isError"], true,
            "expected {tool_name} to fail in thread scope"
        );
    }

    // Also verify `mcp-client` session rejection on `/ws` upgrade against freshly built binary.
    let mcp_tok = mint_scoped_session(
        &harness,
        &token,
        "mcp-client",
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;
    let mut tcp = TcpStream::connect(harness.addr()?).await?;
    let req = format!(
        "GET /ws?orchestrationProtocol=2&wsTicket={mcp_tok} HTTP/1.1\r\nHost: {}\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        harness.addr()?
    );
    tcp.write_all(req.as_bytes()).await?;
    let mut buf = [0_u8; 512];
    let n = tcp.read(&mut buf).await?;
    assert!(
        String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 401"),
        "expected 401 for mcp-client on /ws"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f22_t2_05_scoped_mcp_rejects_cross_card_and_approval_bypass() -> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    assert!(!(!board_capability_enabled(&harness).await?), "required workflow board capability: [F22-T2-05] cross-card MCP isolation");

    let admin_tok = harness.pair_client("mcp-cross-card", None).await?;
    let admin_auth = format!("Bearer {admin_tok}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &admin_auth)],
    )
    .await?;
    let project_id = ensure_project_added(&harness, &mut ws).await?;
    let card_a = extract_card_id(
        &ws.call_ok(
            "create-a",
            "workflowBoard.createCard",
            json!({ "projectId": project_id, "title": "Card A" }),
        )
        .await?,
    )?;
    let card_b = extract_card_id(
        &ws.call_ok(
            "create-b",
            "workflowBoard.createCard",
            json!({ "projectId": project_id, "title": "Card B" }),
        )
        .await?,
    )?;
    let thr_a = extract_thread_id(
        &ws.call_ok(
            "start-a",
            "workflowBoard.startStage",
            json!({ "cardId": card_a, "stage": "spec", "expectedRevision": 1 }),
        )
        .await?,
    )?;
    let tok_a = mint_scoped_session(
        &harness,
        &admin_tok,
        &format!("thread:{thr_a}"),
        &["orchestration:read", "orchestration:operate"],
    )
    .await?;

    for (idx, (tool, args)) in [
        (
            "legion_workflow_board_read_brief",
            json!({ "cardId": card_b }),
        ),
        (
            "legion_workflow_board_propose",
            json!({ "cardId": card_b, "stage": "spec", "markdown": "Hijack" }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let body = call_mcp_tool(&harness, &tok_a, &thr_a, idx + 1, tool, args).await?;
        assert_eq!(body["result"]["isError"], true);
    }

    let _ = call_mcp_tool(
        &harness,
        &tok_a,
        &thr_a,
        99,
        "legion_workflow_board_propose",
        json!({
            "cardId": card_a,
            "stage": "ship",
            "approve": true,
            "approveDelivery": true,
            "markdown": "# Bypass attempt"
        }),
    )
    .await?;

    let card_a_detail = ws
        .call_ok(
            "get-a",
            "workflowBoard.getCard",
            json!({ "cardId": card_a }),
        )
        .await?;
    assert_ne!(card_a_detail["stage"], Value::String("ship".to_owned()));
    assert_ne!(
        card_a_detail["specApprovalStatus"],
        Value::String("valid".to_owned())
    );
    assert!(card_a_detail["specApproval"].is_null());
    assert!(card_a_detail["specApprovedRevision"].is_null());

    Ok(())
}

async fn verify_large_frame_parts_roundtrip(addr: SocketAddr, auth_header: &str) -> TestResult<()> {
    let mut stream = TcpStream::connect(addr).await?;
    let req = format!(
        "GET /ws?orchestrationProtocol=2&frameParts=1 HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {auth_header}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    let mut tmp = [0_u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err("EOF waiting for 101 handshake in frameParts test".into());
        }
        raw.extend_from_slice(&tmp[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = std::str::from_utf8(&raw[..header_end])?;
    assert!(head.starts_with("HTTP/1.1 101"), "expected 101: {head}");
    let mut buf = raw[header_end..].to_vec();

    let build_probe_req = |id: &str, payload: Value| ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: "server.probe".to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    };
    let encoded = encode_client_frame(&build_probe_req(
        "large-fp-1",
        json!({ "pad": "x".repeat(70_000) }),
    ))?;
    assert!(encoded.len() > 65_536, "expected >64KB encoded RPC frame");
    let parts = split_socket_frame(&encoded);
    assert!(parts.len() >= 2, "expected multi-part frame split");

    for (idx, part) in parts.iter().enumerate() {
        let is_last = idx + 1 == parts.len();
        if !is_last {
            assert!(part.starts_with(PART_PREFIX));
        }
        write_ws_masked_text(&mut stream, part).await?;
        if !is_last {
            let ack = read_ws_text_frame(&mut stream, &mut buf).await?;
            assert_eq!(ack, PART_RECEIVED_FRAME, "expected Pong ack for part {idx}");
        }
    }

    let exit_text = read_ws_text_frame(&mut stream, &mut buf).await?;
    let got_large = decode_server_frames(&exit_text)?.into_iter().any(|f| {
        matches!(
            f,
            ServerRpcFrame::Exit { request_id, exit: RpcExit::Success { value } }
                if request_id == RpcId::from("large-fp-1") && value["serverTime"].as_str().is_some()
        )
    });
    assert!(got_large, "expected Exit::Success for large frameParts");

    let followup = encode_client_frame(&build_probe_req("probe-after-fp", json!({})))?;
    write_ws_masked_text(&mut stream, &followup).await?;
    let followup_text = read_ws_text_frame(&mut stream, &mut buf).await?;
    assert!(decode_server_frames(&followup_text)?.into_iter().any(|f| {
        matches!(
            f,
            ServerRpcFrame::Exit { request_id, exit: RpcExit::Success { .. } }
                if request_id == RpcId::from("probe-after-fp")
        )
    }));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn f22_t2_06_ws_parse_error_and_frame_parts_resilience() -> TestResult<()> {
    let harness = OpaqueBoxHarness::start(true)?;
    let board_enabled = board_capability_enabled(&harness).await?;
    let token = harness.pair_client("parse-boundary", None).await?;
    let auth = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2&frameParts=1",
        &[("Authorization", &auth)],
    )
    .await?;

    let empty_exit = ws.call_unary("empty-1", "   ", json!({})).await?;
    match empty_exit {
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                return Err(format!("expected Fail cause for empty tag, got {cause:?}").into());
            };
            assert_eq!(error["_tag"], "ParseError");
        }
        RpcExit::Success { value } => {
            return Err(format!("expected ParseError for empty tag, got {value}").into());
        }
    }

    if board_enabled {
        let bad_create = ws
            .call_unary(
                "bad-create",
                "workflowBoard.createCard",
                json!({ "projectId": "", "title": 42 }),
            )
            .await?;
        assert!(matches!(bad_create, RpcExit::Failure { .. }));
    }

    let probe = ws.call_ok("probe-after", "server.probe", json!({})).await?;
    assert!(probe["serverTime"].as_str().is_some());

    verify_large_frame_parts_roundtrip(harness.addr()?, &auth).await?;

    Ok(())
}
