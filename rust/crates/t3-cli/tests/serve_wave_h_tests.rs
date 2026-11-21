//! Integration tests for Wave H `t3 serve` endpoints (`/ws/voice`, Wave H WS RPCs, and `/__surface/app/*`).

mod support;
#[path = "support/ws_client.rs"]
mod ws_client;
#[path = "support/ws_voice_client.rs"]
mod ws_voice_client;

use std::fs;

use serde_json::{Value, json};
use t3_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use t3_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use t3_terminal_vt::{Rgba, decode_t3vt};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use support::{TestResult, isolated_env, run_t3};
use ws_client::WsTestClient;
use ws_voice_client::{WsIncomingFrame, WsVoiceTestClient};

async fn spawn_test_server(
    base_dir: &std::path::Path,
) -> Result<RunningServer, Box<dyn std::error::Error>> {
    Ok(spawn_server(ServeConfig {
        base_dir: base_dir.to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: base_dir.to_path_buf(),
    })
    .await?)
}

async fn call_ws_rpc(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<RpcExit, Box<dyn std::error::Error>> {
    let req = ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: tag.to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    };
    ws.send_text(&encode_client_frame(&req)?).await?;
    let frames = decode_server_frames(&ws.recv_text().await?)?;
    match frames.into_iter().next() {
        Some(ServerRpcFrame::Exit { exit, .. }) => Ok(exit),
        other => Err(format!("expected Exit frame, got {other:?}").into()),
    }
}

#[tokio::test]
async fn voice_ws_happy_path_and_bug10_error_frames_before_close() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_test_server(&temp.path().join("voice")).await?;

    // Happy path: start -> ping/pong -> audio -> stop -> finished + Close(1000).
    let mut ok_ws = WsVoiceTestClient::connect(server.addr(), "/ws/voice").await?;
    ok_ws.send_text(r#"{"type":"start"}"#).await?;
    let WsIncomingFrame::Text(ready_raw) = ok_ws.recv_frame().await? else {
        return Err("expected ready text frame".into());
    };
    let ready: Value = serde_json::from_str(&ready_raw)?;
    assert_eq!(ready["type"], "ready");

    ok_ws.send_text(r#"{"type":"ping","nonce":"v1"}"#).await?;
    let WsIncomingFrame::Text(pong_raw) = ok_ws.recv_frame().await? else {
        return Err("expected pong text frame".into());
    };
    assert_eq!(serde_json::from_str::<Value>(&pong_raw)?["nonce"], "v1");

    ok_ws.send_binary(&[0_u8; 1024]).await?;
    let WsIncomingFrame::Text(upd_raw) = ok_ws.recv_frame().await? else {
        return Err("expected update text frame".into());
    };
    let upd: Value = serde_json::from_str(&upd_raw)?;
    assert_eq!(upd["type"], "update");
    assert_eq!(upd["revision"], 1);

    ok_ws.send_text(r#"{"type":"stop"}"#).await?;
    let WsIncomingFrame::Text(fin_raw) = ok_ws.recv_frame().await? else {
        return Err("expected finished text frame".into());
    };
    assert_eq!(serde_json::from_str::<Value>(&fin_raw)?["type"], "finished");
    let WsIncomingFrame::Close { code, reason } = ok_ws.recv_frame().await? else {
        return Err("expected Close after finished".into());
    };
    assert_eq!(code, Some(1000));
    assert_ne!(reason, "");

    // BUG-10 timeout MUST send JSON error text frame BEFORE WebSocket Close frame.
    let mut timeout_ws =
        WsVoiceTestClient::connect(server.addr(), "/ws/voice?maxSessionDurationMs=80").await?;
    let WsIncomingFrame::Text(err_raw) = timeout_ws.recv_frame().await? else {
        return Err("expected BUG-10 error text frame before Close on timeout".into());
    };
    let err_json: Value = serde_json::from_str(&err_raw)?;
    assert_eq!(err_json["type"], "error");
    assert_eq!(err_json["code"], "session_timeout");
    let WsIncomingFrame::Close {
        code: close_code,
        reason: close_reason,
    } = timeout_ws.recv_frame().await?
    else {
        return Err("expected Close frame immediately after timeout error".into());
    };
    assert_eq!(close_code, Some(1008));
    assert!(close_reason.contains("timed out"));

    // BUG-10 buffer overflow emits buffer_overflow JSON BEFORE Close(1009).
    let mut overflow_ws =
        WsVoiceTestClient::connect(server.addr(), "/ws/voice?maxAudioBufferBytes=128").await?;
    overflow_ws.send_text(r#"{"type":"start"}"#).await?;
    let _ = overflow_ws.recv_frame().await?;
    overflow_ws.send_binary(&[0_u8; 256]).await?;
    let WsIncomingFrame::Text(ovf_raw) = overflow_ws.recv_frame().await? else {
        return Err("expected overflow error text frame before Close".into());
    };
    assert_eq!(
        serde_json::from_str::<Value>(&ovf_raw)?["code"],
        "buffer_overflow"
    );
    let WsIncomingFrame::Close {
        code: ovf_code,
        reason: ovf_reason,
    } = overflow_ws.recv_frame().await?
    else {
        return Err("expected Close after overflow error".into());
    };
    assert_eq!(ovf_code, Some(1008));
    assert_ne!(ovf_reason, "");

    // Verify synchronous `__service-preflight` coexists on a running server's DB path cleanly.
    let cli_env = isolated_env(temp.path(), &[]);
    let preflight_out = run_t3(
        &[
            "__service-preflight",
            "--database-path",
            &server.paths().db_path.to_string_lossy(),
            "--launcher-protocol",
            "4",
        ],
        &cli_env,
    )?;
    assert!(preflight_out.status.success());

    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_exposes_terminal_snapshot_resources_sample_and_surfaces_rpc() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_test_server(&temp.path().join("rpc-h")).await?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;

    // terminal.snapshot
    let RpcExit::Success { value: term_val } = call_ws_rpc(
        &mut ws,
        "wh-1",
        "terminal.snapshot",
        json!({
            "cols": 20,
            "rows": 4,
            "feed": "\x1b]2;WaveHTerm\x07\x1b[32mhi\x1b[0m",
            "pasteInput": "ok\x1b[201~evil",
            "paletteOverrides": [{ "index": 1, "color": "#fa0a0a" }]
        }),
    )
    .await?
    else {
        return Err("expected Success from terminal.snapshot".into());
    };
    assert_eq!(term_val["title"], "WaveHTerm");
    assert_eq!(term_val["sanitizedPaste"], "okevil");
    let raw_t3vt: Vec<u8> = serde_json::from_value(term_val["t3vtBytes"].clone())?;
    let decoded = decode_t3vt(&raw_t3vt)?;
    assert_eq!(decoded.cols, 20);
    assert_eq!(decoded.palette.override_at(1), Some(Rgba::rgb(250, 10, 10)));
    assert_eq!(decoded.palette.override_at(0), None);

    // resources.sample
    let RpcExit::Success { value: res_val } = call_ws_rpc(
        &mut ws,
        "wh-2",
        "resources.sample",
        json!({ "includeProcessTable": true }),
    )
    .await?
    else {
        return Err("expected Success from resources.sample".into());
    };
    assert_eq!(res_val["protocolVersion"], 3);
    assert_eq!(res_val["sample"]["type"], "snapshot");
    let expected_pid = std::process::id();
    assert!(
        res_val["sample"]["processes"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["pid"] == expected_pid))
    );
    assert!(
        res_val["processTable"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );

    verify_surfaces_rpc(&mut ws).await?;

    server.shutdown().await?;
    Ok(())
}

async fn verify_surfaces_rpc(ws: &mut WsTestClient) -> TestResult {
    // surfaces.decorateDiff (BUG-22)
    let RpcExit::Success { value: diff_val } = call_ws_rpc(
        ws,
        "wh-3",
        "surfaces.decorateDiff",
        json!({
            "tokens": [{
                "content": "foobar",
                "color": "#aabbcc",
                "fontStyle": 3,
                "attributes": { "scope": "identifier" }
            }],
            "wordHighlights": [{ "start": 3, "end": 6 }],
            "highlightBackground": "#332211"
        }),
    )
    .await?
    else {
        return Err("expected Success from surfaces.decorateDiff".into());
    };
    let runs = diff_val["runs"]
        .as_array()
        .ok_or("missing decorated runs")?;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1]["fontStyle"], 3);
    assert_eq!(runs[1]["wordHighlighted"], true);
    assert_eq!(runs[1]["backgroundColor"], "#332211");
    assert_eq!(runs[1]["attributes"]["scope"], "identifier");

    // surfaces.mobileChrome (BUG-24)
    let RpcExit::Success { value: chrome_val } = call_ws_rpc(
        ws,
        "wh-4",
        "surfaces.mobileChrome",
        json!({
            "language": "de",
            "activity": {
                "active": true,
                "rows": [{ "phase": "waiting_for_approval", "title": "Fix bug", "project": "t3code" }]
            }
        }),
    )
    .await?
    else {
        return Err("expected Success from surfaces.mobileChrome".into());
    };
    assert_eq!(chrome_val["locale"], "de");
    assert_eq!(chrome_val["navigation"]["settings"], "Einstellungen");
    assert_eq!(chrome_val["liveActivity"]["locale"], "de");
    assert_eq!(
        chrome_val["liveActivity"]["rows"][0]["phase"],
        "waiting_for_approval"
    );
    assert!(chrome_val["liveActivity"]["action"].is_string());
    Ok(())
}

#[tokio::test]
async fn surface_app_http_routes_enforces_csp_traversal_and_assets() -> TestResult {
    let temp = tempfile::tempdir()?;
    let base_dir = temp.path().join("surface-http");
    let assets_dir = base_dir.join("surface-app").join("assets");
    fs::create_dir_all(&assets_dir)?;
    fs::write(assets_dir.join("app.js"), "console.log('t3');")?;

    let server = spawn_test_server(&base_dir).await?;
    let client = reqwest::Client::new();

    // Concrete bundled asset
    let asset_res = client
        .get(format!("{}/__surface/app/assets/app.js", server.origin()))
        .send()
        .await?;
    assert_eq!(asset_res.status(), 200);
    assert!(
        asset_res
            .headers()
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|csp| csp.contains("t3code:") && csp.contains("default-src 'self'"))
    );
    assert_eq!(asset_res.text().await?, "console.log('t3');");

    // SPA fallback route
    let spa_res = client
        .get(format!("{}/__surface/app/settings", server.origin()))
        .send()
        .await?;
    assert_eq!(spa_res.status(), 200);
    assert!(spa_res.text().await?.contains("<div id=\"root\"></div>"));

    // Encoded directory traversal sent verbatim over TCP (reqwest Url::parse normalizes `/..`)
    let mut tcp = TcpStream::connect(server.addr()).await?;
    let raw_req = format!(
        "GET /__surface/app/%2e%2e/secret HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        server.addr()
    );
    tcp.write_all(raw_req.as_bytes()).await?;
    let mut raw_resp = Vec::new();
    tcp.read_to_end(&mut raw_resp).await?;
    let trav_text = String::from_utf8(raw_resp)?;
    assert!(trav_text.starts_with("HTTP/1.1 404"));
    assert!(
        trav_text
            .to_ascii_lowercase()
            .contains("content-security-policy:")
    );
    assert!(trav_text.contains("path_traversal"));

    // Invalid percent-encoding rejected with 400
    let bad_res = client
        .get(format!("{}/__surface/app/%ZZ", server.origin()))
        .send()
        .await?;
    assert_eq!(bad_res.status(), 400);

    server.shutdown().await?;
    Ok(())
}
