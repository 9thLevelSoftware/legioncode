#![forbid(unsafe_code)]
//! Smoke self-test for the opaque-box Rust project workflow board E2E harness.

#[path = "workflow_board_e2e/mod.rs"]
mod workflow_board_e2e;

use std::fs;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use workflow_board_e2e::TestResult;
use workflow_board_e2e::git_fixtures::{
    CliLockKind, CopyLegionCliOpts, commit_file, copy_disposable_legion_cli, create_worktree,
    seed_active_cli_lock,
};
use workflow_board_e2e::harness::OpaqueBoxHarness;
use workflow_board_e2e::ws_rpc::OpaqueWsClient;

async fn verify_ws_and_fixtures(harness: &OpaqueBoxHarness, token: &str) -> TestResult<()> {
    let auth_header = format!("Bearer {token}");
    let mut ws = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2&frameParts=1",
        &[("Authorization", &auth_header)],
    )
    .await?;

    let probe = ws.call_ok("probe-1", "server.probe", json!({})).await?;
    assert!(probe["serverTime"].as_str().is_some());

    ws.subscribe("sub-cfg", "subscribeServerConfig", json!({}))
        .await?;
    // Calling unary while a stream is open verifies `pending_chunks` preserves stream items.
    let ping_cfg = ws.call_ok("cfg-1", "server.getConfig", json!({})).await?;
    assert!(ping_cfg["providers"].is_array());
    let first_chunk = ws.next_chunk_for("sub-cfg").await?;
    assert_eq!(first_chunk["type"], "snapshot");
    ws.interrupt("sub-cfg").await?;

    let wt_dir = harness.workspace_dir().join("wt-smoke");
    create_worktree(harness.workspace_dir(), &wt_dir, "feat/rust-e2e-smoke")?;
    let sha = commit_file(&wt_dir, "smoke.txt", "rust smoke\n", "add smoke file")?;
    assert!(sha.len() >= 7);

    let cli_copy = copy_disposable_legion_cli(
        harness.workspace_dir(),
        CopyLegionCliOpts {
            include_map: false,
            include_cache: false,
        },
    )?;
    assert!(cli_copy.join("workflow").join("execution.yaml").is_file());
    assert!(cli_copy.join("tasks").join("TSK-0002.md").is_file());
    assert!(!cli_copy.join("map").exists());

    let engine_lock = seed_active_cli_lock(harness.workspace_dir(), CliLockKind::EngineLock)?;
    assert_eq!(
        engine_lock,
        harness
            .workspace_dir()
            .join(".legion-cli")
            .join("index")
            .join("engine.lock")
    );
    let engine_json: Value = serde_json::from_str(&fs::read_to_string(&engine_lock)?)?;
    assert!(engine_json["pid"].as_u64().is_some());
    assert!(engine_json["acquiredAt"].as_str().is_some());
    assert!(engine_json["hostname"].as_str().is_some());
    assert!(engine_json["platform"].as_str().is_some());

    let live_spawn = seed_active_cli_lock(harness.workspace_dir(), CliLockKind::LiveSpawn)?;
    assert_eq!(
        live_spawn,
        harness
            .workspace_dir()
            .join(".legion-cli")
            .join("cache")
            .join("live-spawn")
            .join("synthetic-active-run.json")
    );
    let live_json: Value = serde_json::from_str(&fs::read_to_string(&live_spawn)?)?;
    assert_eq!(live_json["schemaVersion"], "legion-cli-live-run/v1");
    assert_eq!(live_json["runId"], "synthetic-active-run");
    assert!(live_json["skillId"].as_str().is_some());
    assert!(live_json["enginePid"].as_u64().is_some());
    assert!(live_json["startedAt"].as_str().is_some());

    let resume_lock = seed_active_cli_lock(harness.workspace_dir(), CliLockKind::ResumeJson)?;
    assert_eq!(
        resume_lock,
        harness
            .workspace_dir()
            .join(".legion-cli")
            .join("cache")
            .join("runs")
            .join("synthetic-active-run")
            .join("resume.json")
    );
    let resume_json: Value = serde_json::from_str(&fs::read_to_string(&resume_lock)?)?;
    assert_eq!(resume_json["schemaVersion"], "legion-cli-resume/v1");
    assert_eq!(resume_json["runId"], "synthetic-active-run");
    assert!(resume_json["skillId"].as_str().is_some());
    assert!(resume_json["pid"].as_u64().is_some());
    assert!(resume_json["startedAt"].as_str().is_some());
    Ok(())
}

async fn send_unmasked_ws_text(stream: &mut tokio::net::TcpStream, text: &str) -> TestResult<()> {
    let bytes = text.as_bytes();
    let mut frame = Vec::with_capacity(bytes.len() + 10);
    frame.push(0x81);
    if bytes.len() <= 125 {
        frame.push(u8::try_from(bytes.len())?);
    } else {
        frame.push(126);
        frame.extend_from_slice(&u16::try_from(bytes.len())?.to_be_bytes());
    }
    frame.extend_from_slice(bytes);
    stream.write_all(&frame).await?;
    Ok(())
}

async fn recv_masked_ws_text(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut header = [0_u8; 2];
    sock.read_exact(&mut header).await.ok()?;
    let masked = (header[1] & 0x80) != 0;
    let payload_len = match header[1] & 0x7f {
        len @ 0..=125 => usize::from(len),
        126 => {
            let mut ext = [0_u8; 2];
            sock.read_exact(&mut ext).await.ok()?;
            usize::from(u16::from_be_bytes(ext))
        }
        _ => {
            let mut ext = [0_u8; 8];
            sock.read_exact(&mut ext).await.ok()?;
            usize::try_from(u64::from_be_bytes(ext)).ok()?
        }
    };
    let mask = if masked {
        let mut m = [0_u8; 4];
        sock.read_exact(&mut m).await.ok()?;
        Some(m)
    } else {
        None
    };
    let mut payload = vec![0_u8; payload_len];
    sock.read_exact(&mut payload).await.ok()?;
    if let Some(m) = mask {
        for (idx, b) in payload.iter_mut().enumerate() {
            *b ^= m[idx % 4];
        }
    }
    String::from_utf8(payload).ok()
}

async fn read_until_client_tag(sock: &mut tokio::net::TcpStream, needle: &str) -> bool {
    while let Some(text) = recv_masked_ws_text(sock).await {
        if text.contains(needle) {
            return true;
        }
    }
    false
}

async fn accept_mock_ws(listener: &TcpListener) -> Result<tokio::net::TcpStream, String> {
    let (mut sock, _) = listener
        .accept()
        .await
        .map_err(|e| format!("accept failed: {e}"))?;
    let mut req = Vec::new();
    let mut tmp = [0_u8; 1024];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = sock
            .read(&mut tmp)
            .await
            .map_err(|e| format!("handshake read failed: {e}"))?;
        if n == 0 {
            return Err("EOF during mock WS handshake".to_owned());
        }
        req.extend_from_slice(&tmp[..n]);
    }
    let handshake = "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
    sock.write_all(handshake.as_bytes())
        .await
        .map_err(|e| format!("handshake write failed: {e}"))?;
    Ok(sock)
}

async fn serve_frame_parts_conn(listener: &TcpListener) -> Result<(), String> {
    let mut sock = accept_mock_ws(listener).await?;
    if !read_until_client_tag(&mut sock, "mock.fp1").await {
        return Err("missing mock.fp1 request".to_owned());
    }
    let fp1_json = json!([{
        "_tag": "Exit",
        "requestId": "fp1",
        "exit": { "_tag": "Success", "value": { "reassembled": true } }
    }])
    .to_string();
    let mid1 = fp1_json.len() / 3;
    let mid2 = (fp1_json.len() * 2) / 3;
    let parts = [
        format!("\u{001e}{}", &fp1_json[..mid1]),
        format!("\u{001e}{}", &fp1_json[mid1..mid2]),
        fp1_json[mid2..].to_owned(),
    ];
    for part in &parts {
        let _ = send_unmasked_ws_text(&mut sock, part).await;
    }
    while let Some(text) = recv_masked_ws_text(&mut sock).await {
        if text.contains("Pong") {
            return Err(format!(
                "OpaqueWsClient echoed IntermediateAck Pong back to server: {text}"
            ));
        }
        if text.contains("mock.fp2") {
            let fp2_json = json!([{
                "_tag": "Exit",
                "requestId": "fp2",
                "exit": { "_tag": "Success", "value": { "clean": true } }
            }])
            .to_string();
            let _ = send_unmasked_ws_text(&mut sock, &fp2_json).await;
            return Ok(());
        }
    }
    Err("missing mock.fp2 request after multi-part Exit".to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opaque_ws_client_preserves_batched_chunks_and_interleaved_exits() -> TestResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let server_task = tokio::spawn(async move {
        let mut sock = accept_mock_ws(&listener).await?;
        if !read_until_client_tag(&mut sock, "mock.u1").await {
            return Err("missing mock.u1".to_owned());
        }
        let batch = json!([
            {
                "_tag": "Exit",
                "requestId": "u1",
                "exit": { "_tag": "Success", "value": { "ok": true } }
            },
            {
                "_tag": "Chunk",
                "requestId": "sub1",
                "values": [{ "seq": 99 }]
            }
        ])
        .to_string();
        let _ = send_unmasked_ws_text(&mut sock, &batch).await;

        if !read_until_client_tag(&mut sock, "mock.u2").await {
            return Err("missing mock.u2".to_owned());
        }
        let interleaved = json!([
            {
                "_tag": "Exit",
                "requestId": "sub2",
                "exit": { "_tag": "Success", "value": null }
            },
            {
                "_tag": "Exit",
                "requestId": "u2",
                "exit": { "_tag": "Success", "value": { "u2": true } }
            }
        ])
        .to_string();
        let _ = send_unmasked_ws_text(&mut sock, &interleaved).await;
        drop(sock);

        serve_frame_parts_conn(&listener).await
    });

    let mut ws = OpaqueWsClient::connect(addr, "/ws?orchestrationProtocol=2", &[]).await?;
    let u1 = ws.call_ok("u1", "mock.u1", json!({})).await?;
    assert_eq!(u1["ok"], true);

    let chunk = ws.next_chunk_for("sub1").await?;
    assert_eq!(chunk["seq"], 99);

    let u2 = ws.call_ok("u2", "mock.u2", json!({})).await?;
    assert_eq!(u2["u2"], true);

    let sub2_err = ws
        .next_chunk_for("sub2")
        .await
        .expect_err("interleaved Exit for sub2 must be preserved");
    assert!(sub2_err.to_string().contains("exited early"));
    drop(ws);

    let mut ws_fp =
        OpaqueWsClient::connect(addr, "/ws?orchestrationProtocol=2&frameParts=1", &[]).await?;
    let fp1 = ws_fp.call_ok("fp1", "mock.fp1", json!({})).await?;
    assert_eq!(fp1["reassembled"], true);
    let fp2 = ws_fp.call_ok("fp2", "mock.fp2", json!({})).await?;
    assert_eq!(fp2["clean"], true);
    drop(ws_fp);

    server_task.await.map_err(|e| e.to_string())??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opaque_box_harness_boots_pairs_streams_restarts_and_stages_fixtures() -> TestResult<()> {
    let mut harness = OpaqueBoxHarness::start(true)?;
    assert!(harness.server()?.pairing_url().is_some());

    let env_desc = harness.probe_environment().await?;
    assert_eq!(env_desc["orchestrationProtocolVersion"], 2);
    assert_eq!(
        env_desc["capabilities"]["projectWorkflowBoard"].as_bool(),
        Some(true)
    );

    let token = harness.pair_client("e2e-rust-harness", None).await?;
    verify_ws_and_fixtures(&harness, &token).await?;

    let ws_str = harness.workspace_dir().to_string_lossy().into_owned();
    let base_str = harness.home_dir().to_string_lossy().into_owned();
    let add_out = harness.run_cli(&[
        "project",
        "add",
        &ws_str,
        "--title",
        "Rust E2E Board Workspace",
        "--base-dir",
        &base_str,
    ])?;
    assert!(
        add_out.status.success(),
        "project add failed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let mcp_resp = harness
        .post_mcp(
            Some(&token),
            &[("x-legioncode-e2e", "1")],
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "e2e-smoke", "version": "1.0.0" }
                }
            }),
        )
        .await?;
    assert!(mcp_resp.status().as_u16() < 500);

    harness.restart_server()?;

    let token_after = harness
        .pair_client("e2e-rust-after-restart", Some(&["orchestration:read"]))
        .await?;
    let auth_after = format!("Bearer {token_after}");
    let mut ws_after = OpaqueWsClient::connect(
        harness.addr()?,
        "/ws?orchestrationProtocol=2",
        &[("Authorization", &auth_after)],
    )
    .await?;
    let shell = ws_after
        .call_ok("shell-1", "orchestration.v2.getShellSnapshot", json!({}))
        .await?;
    assert_eq!(shell["projects"].as_array().map(Vec::len), Some(1));

    let db = harness.open_db_readonly()?;
    let migration_head: i64 = db.query_row(
        "SELECT MAX(migration_id) FROM effect_sql_migrations",
        [],
        |row| row.get(0),
    )?;
    assert!(migration_head >= 110);

    Ok(())
}
