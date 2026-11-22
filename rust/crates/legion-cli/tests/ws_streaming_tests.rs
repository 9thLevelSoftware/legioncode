//! Integration tests for `/ws` auth, multiplexing, `?frameParts=1`, backpressure, and streams.

#[path = "support/ws_client.rs"]
mod ws_client;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::sqlite::SqliteStore;
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use legion_shared::frame_parts::{
    FrameOutput, FramePartAssembler, PART_PREFIX, split_socket_frame,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use ws_client::WsTestClient;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn spawn_srv(base: &std::path::Path) -> Result<RunningServer, Box<dyn std::error::Error>> {
    spawn_server(ServeConfig {
        base_dir: base.to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: base.to_path_buf(),
    })
    .await
    .map_err(Into::into)
}

fn req_frame(id: &str, tag: &str, payload: Value) -> ClientRpcFrame {
    ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: tag.to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    }
}

async fn send_frame(ws: &mut WsTestClient, frame: &ClientRpcFrame) -> TestResult {
    ws.send_text(&encode_client_frame(frame)?).await
}

async fn send_req(ws: &mut WsTestClient, id: &str, tag: &str, payload: Value) -> TestResult {
    send_frame(ws, &req_frame(id, tag, payload)).await
}

async fn recv_frames(
    ws: &mut WsTestClient,
) -> Result<Vec<ServerRpcFrame>, Box<dyn std::error::Error>> {
    Ok(decode_server_frames(&ws.recv_text().await?)?)
}

async fn raw_upgrade(
    addr: std::net::SocketAddr,
    path: &str,
    hdrs: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(addr).await?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{hdrs}\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    let (mut raw, mut buf) = (Vec::new(), [0_u8; 2048]);
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        let s = String::from_utf8_lossy(&raw);
        let ready = s.starts_with("HTTP/1.1 101") || raw.len() > 256 || s.contains('}');
        if s.contains("\r\n\r\n") && ready {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

async fn pair_session(server: &RunningServer) -> Result<String, Box<dyn std::error::Error>> {
    let now_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let issued = {
        let store = SqliteStore::open(&server.paths().db_path)?;
        let input = IssuePairingTokenInput::default();
        issue_pairing_token_with_conn(store.connection(), &input, now_ms)?
    };
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": issued.credential, "clientLabel": "ws-test" }))
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    Ok(body["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned())
}

async fn call_unary(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<RpcExit, Box<dyn std::error::Error>> {
    send_req(ws, id, tag, payload).await?;
    loop {
        for frame in recv_frames(ws).await? {
            if let ServerRpcFrame::Exit { request_id, exit } = frame
                && request_id == RpcId::from(id)
            {
                return Ok(exit);
            }
        }
    }
}

async fn create_thread(ws: &mut WsTestClient, thread_id: &str) -> TestResult {
    let cmd = json!({
        "type": "thread.create", "commandId": format!("cmd-{thread_id}"), "threadId": thread_id,
        "projectId": "proj-ws-1", "title": "Initial Title",
        "modelSelection": { "instanceId": "codex", "model": "gpt-5" },
        "runtimeMode": "full-access", "interactionMode": "default",
        "createdBy": "user", "creationSource": "web"
    });
    assert!(matches!(
        call_unary(ws, "c-th", "orchestration.dispatchCommand", cmd).await?,
        RpcExit::Success { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn ws_upgrade_enforces_protocol_auth_bug03_and_origin() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-auth")).await?;
    let (addr, tok) = (server.addr(), pair_session(&server).await?);

    let b = format!("Authorization: Bearer wrong\r\nCookie: legioncode_session={tok}\r\n");
    let auth_hdr = format!("Authorization: Bearer {tok}\r\n");
    let cook_hdr = format!("Cookie: legioncode_session={tok}\r\n");
    let ticket_path = format!("/ws?orchestrationProtocol=2&wsTicket={tok}");
    let ws2 = "/ws?orchestrationProtocol=2";
    for (p, h, want) in [
        ("/ws", "", "HTTP/1.1 426"),
        (
            "/ws?orchestrationProtocol=2&wsTicket=bogus",
            "",
            "HTTP/1.1 401",
        ),
        (ws2, b.as_str(), "HTTP/1.1 401"),
        (ws2, "X-Forwarded-For: 10.0.0.1\r\n", "HTTP/1.1 401"),
        (ws2, "Origin: https://evil.example\r\n", "HTTP/1.1 403"),
        (ticket_path.as_str(), "", "HTTP/1.1 101"),
        (ws2, auth_hdr.as_str(), "HTTP/1.1 101"),
        (ws2, cook_hdr.as_str(), "HTTP/1.1 101"),
        (ws2, "", "HTTP/1.1 101"),
    ] {
        let resp = raw_upgrade(addr, p, h).await?;
        assert!(resp.starts_with(want), "{p} got: {resp}");
    }
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_multiplexes_concurrent_streams_and_interrupt() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-mux")).await?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    create_thread(&mut ws, "thr-mux-1").await?;

    send_req(
        &mut ws,
        "sub-shell",
        "orchestration.subscribeShell",
        json!({}),
    )
    .await?;
    let th_sub = json!({ "threadId": "thr-mux-1", "acceptCompactIds": true });
    send_req(
        &mut ws,
        "sub-thread",
        "orchestration.subscribeThread",
        th_sub,
    )
    .await?;

    let (mut shell_items, mut thread_items) = (Vec::new(), Vec::new());
    while shell_items.len() < 2 || thread_items.len() < 2 {
        for f in recv_frames(&mut ws).await? {
            if let ServerRpcFrame::Chunk { request_id, values } = f {
                if request_id == RpcId::from("sub-shell") {
                    shell_items.extend(values);
                } else if request_id == RpcId::from("sub-thread") {
                    thread_items.extend(values);
                }
            }
        }
    }
    assert_eq!(
        (&shell_items[0]["kind"], &shell_items[1]["kind"]),
        (&json!("snapshot"), &json!("synchronized"))
    );
    assert_eq!(
        (&thread_items[0]["kind"], &thread_items[1]["kind"]),
        (&json!("snapshot"), &json!("synchronized"))
    );
    assert!(thread_items[0]["ids"].is_array() && thread_items[0].get("idTable").is_none());

    let msg_cmd = json!({
        "type": "thread.turn.start", "commandId": "cmd-msg-1", "threadId": "thr-mux-1",
        "message": { "messageId": "m-1", "text": "hello live streams", "attachments": [] }
    });
    send_req(&mut ws, "c-msg", "orchestration.dispatchCommand", msg_cmd).await?;
    let (mut msg_ok, mut shell_patch, mut thread_ev) = (false, false, false);
    while !(msg_ok && shell_patch && thread_ev) {
        for f in recv_frames(&mut ws).await? {
            match f {
                ServerRpcFrame::Exit { request_id, exit } if request_id == RpcId::from("c-msg") => {
                    assert!(matches!(exit, RpcExit::Success { .. }));
                    msg_ok = true;
                }
                ServerRpcFrame::Chunk { request_id, values } => {
                    let is_th_upd = values.iter().any(|v| v["kind"] == "thread.updated");
                    shell_patch |= request_id == RpcId::from("sub-shell") && is_th_upd;
                    thread_ev |= request_id == RpcId::from("sub-thread")
                        && values.iter().any(|v| v["kind"] == "event");
                }
                _ => {}
            }
        }
    }

    send_frame(
        &mut ws,
        &ClientRpcFrame::Interrupt {
            request_id: RpcId::from("sub-thread"),
        },
    )
    .await?;
    loop {
        if recv_frames(&mut ws).await?.iter().any(|f| {
            matches!(f, ServerRpcFrame::Exit { request_id, exit: RpcExit::Failure { .. } } if request_id == &RpcId::from("sub-thread"))
        }) {
            break;
        }
    }
    drop(ws);
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_frame_parts_splits_oversized_outbound_chunks() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-fp")).await?;
    let mut ws =
        WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2&frameParts=1").await?;
    create_thread(&mut ws, "thr-fp-1").await?;

    let big_cmd = json!({
        "type": "thread.turn.start", "commandId": "cmd-big", "threadId": "thr-fp-1",
        "message": { "messageId": "m-big", "text": "x".repeat(56 * 1024), "attachments": [] }
    });
    let wire = encode_client_frame(&req_frame(
        "c-big",
        "orchestration.dispatchCommand",
        big_cmd,
    ))?;
    let parts = split_socket_frame(&wire);
    assert!(parts.len() > 1);
    for p in parts {
        ws.send_text(&p).await?;
    }
    loop {
        if recv_frames(&mut ws).await?.iter().any(|f| {
            matches!(f, ServerRpcFrame::Exit { request_id, exit: RpcExit::Success { .. } } if request_id == &RpcId::from("c-big"))
        }) {
            break;
        }
    }

    send_req(
        &mut ws,
        "sub-fp",
        "orchestration.subscribeThread",
        json!({ "threadId": "thr-fp-1" }),
    )
    .await?;
    let (mut saw_part_prefix, mut asm) = (false, FramePartAssembler::new());
    loop {
        let raw = ws.recv_text().await?;
        saw_part_prefix |= raw.starts_with(PART_PREFIX);
        if let FrameOutput::Complete(full) = asm.push(&raw)
            && decode_server_frames(&full)?.iter().any(|f| {
                matches!(f, ServerRpcFrame::Chunk { values, .. } if values.iter().any(|v| v["kind"] == "snapshot"))
            })
        {
            break;
        }
    }
    assert!(
        saw_part_prefix,
        "expected oversized snapshot to split via ?frameParts=1"
    );
    drop(ws);
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_terminal_attach_output_window_backpressure() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-bp")).await?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;

    let open = call_unary(
        &mut ws,
        "t-open",
        "terminal.open",
        json!({ "threadId": "thr-term-1", "terminalId": "term-1", "cols": 80, "rows": 24 }),
    )
    .await?;
    assert!(matches!(open, RpcExit::Success { .. }));
    send_req(
        &mut ws,
        "t-attach",
        "terminal.attach",
        json!({ "threadId": "thr-term-1", "terminalId": "term-1" }),
    )
    .await?;
    assert!(
        matches!(&recv_frames(&mut ws).await?[..], [ServerRpcFrame::Chunk { values, .. }] if values[0]["type"] == "snapshot")
    );

    let big_chunk = "A".repeat(40_000);
    for chunk in [big_chunk.as_str(), big_chunk.as_str(), "after-ack"] {
        server
            .terminal_runtime()
            .terminal_write("thr-term-1", "term-1", chunk, false)?;
    }
    let mut received = 0_usize;
    while received < 2 {
        for f in recv_frames(&mut ws).await? {
            if let ServerRpcFrame::Chunk { request_id, .. } = f
                && request_id == RpcId::from("t-attach")
            {
                received += 1;
            }
        }
    }
    send_frame(&mut ws, &ClientRpcFrame::Ping).await?;
    assert_eq!(recv_frames(&mut ws).await?, vec![ServerRpcFrame::Pong]);

    let ack = ClientRpcFrame::Ack {
        request_id: RpcId::from("t-attach"),
    };
    send_frame(&mut ws, &ack).await?;
    send_frame(&mut ws, &ack).await?;
    assert!(
        matches!(&recv_frames(&mut ws).await?[..], [ServerRpcFrame::Chunk { values, .. }] if values[0]["data"] == "after-ack")
    );
    drop(ws);
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_emits_valid_snapshots_for_all_stream_subscriptions() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-streams")).await?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;
    let cwd_p = temp.path();
    let cases = [
        ("sc", "subscribeServerConfig", json!({})),
        ("sl", "subscribeServerLifecycle", json!({})),
        ("aa", "subscribeAuthAccess", json!({})),
        ("tm", "subscribeTerminalMetadata", json!({})),
        ("vs", "subscribeVcsStatus", json!({ "cwd": cwd_p })),
        ("vb", "subscribeVcsBranch", json!({ "cwd": cwd_p })),
        ("rt", "subscribeResourceTelemetry", json!({})),
        ("st", "scheduledTasks.subscribe", json!({})),
        ("pc", "subscribeProjectClones", json!({})),
        ("wu", "subscribeWorktreeSetup", json!({})),
        ("ds", "subscribeDiscoveredLocalServers", json!({})),
        ("dv", "subscribeDeviceState", json!({})),
        (
            "pa",
            "provider.auth.subscribe",
            json!({ "instanceId": "codex" }),
        ),
        (
            "pi",
            "provider.install.subscribe",
            json!({ "driver": "codex" }),
        ),
        (
            "ch",
            "provider.chatgpt.handoff.subscribe",
            json!({ "instanceId": "codex" }),
        ),
        ("pr", "pullRequests.subscribeRefreshes", json!({})),
        (
            "pl",
            "pullRequests.subscribeLinkedThreadRefreshes",
            json!({}),
        ),
        (
            "co",
            "orchestration.subscribeCommandOutput",
            json!({ "threadId": "thr-1", "itemId": "it-1" }),
        ),
        (
            "ps",
            "orchestration.subscribePromptSuggestion",
            json!({ "threadId": "thr-1" }),
        ),
        ("sr", "orchestration.searchThreadStream", json!({})),
        ("db", "server.getDatabaseStorage", json!({})),
    ];
    for (id, tag, payload) in &cases {
        send_req(&mut ws, id, tag, payload.clone()).await?;
    }
    let mut seen = HashMap::<String, Value>::new();
    while seen.len() < cases.len() {
        for frame in recv_frames(&mut ws).await? {
            if let ServerRpcFrame::Chunk {
                request_id: RpcId::String(id),
                values,
            } = frame
            {
                seen.entry(id).or_insert_with(|| values[0].clone());
            }
        }
    }
    assert_eq!(seen["sc"]["type"], "snapshot");
    let providers = seen["sc"]["config"]["providers"]
        .as_array()
        .ok_or("providers")?;
    assert!(providers.iter().any(|p| p["driver"] == "devReplay"));
    assert_eq!(seen["sl"]["type"], "welcome");
    assert_eq!(seen["aa"]["type"], "snapshot");
    assert_eq!(seen["rt"]["speedLimitPercent"]["_tag"], "None");
    assert!(seen["db"]["compactCommand"].as_str().is_some());
    drop(ws);
    server.shutdown().await?;
    Ok(())
}
