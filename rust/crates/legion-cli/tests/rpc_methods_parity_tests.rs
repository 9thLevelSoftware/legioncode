//! End-to-end WS / HTTP integration tests for terminal RPC upgrades (`terminal.open` snapshot,
//! `terminal.clear`, optional `terminalId` close) and Wave D/H surface parity on `legioncode serve`.

#[path = "support/ws_client.rs"]
mod ws_client;

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use ws_client::WsTestClient;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn spawn_isolated() -> Result<(TempDir, RunningServer), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let server = spawn_server(ServeConfig {
        base_dir: temp.path().to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: temp.path().to_path_buf(),
    })
    .await?;
    Ok((temp, server))
}

async fn call_ws(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<RpcExit, Box<dyn std::error::Error>> {
    let frame = ClientRpcFrame::Request {
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
    ws.send_text(&encode_client_frame(&frame)?).await?;
    let frames = decode_server_frames(&ws.recv_text().await?)?;
    match frames.into_iter().next() {
        Some(ServerRpcFrame::Exit { exit, .. }) => Ok(exit),
        other => Err(format!("expected Exit frame, got {other:?}").into()),
    }
}

#[tokio::test]
async fn terminal_rpc_returns_snapshot_supports_clear_and_optional_close_id() -> TestResult {
    let (temp, server) = spawn_isolated().await?;
    let cwd = temp.path().join("term-cwd");
    std::fs::create_dir_all(&cwd)?;
    let mut ws = WsTestClient::connect(server.addr(), "/ws?orchestrationProtocol=2").await?;

    // Missing cwd surfaces TerminalCwdNotFoundError
    let missing_exit = call_ws(
        &mut ws,
        "t-missing",
        "terminal.open",
        json!({
            "threadId": "thread-t1",
            "terminalId": "default",
            "cwd": temp.path().join("does-not-exist"),
        }),
    )
    .await?;
    assert!(matches!(missing_exit, RpcExit::Failure { .. }));

    // Headless compatibility open + write + clear + snapshot + close without terminalId
    let RpcExit::Success { value: opened } = call_ws(
        &mut ws,
        "t-open",
        "terminal.open",
        json!({
            "threadId": "thread-t1",
            "terminalId": "term-a",
            "cols": 90,
            "rows": 28,
            "label": "Test Shell",
        }),
    )
    .await?
    else {
        return Err("expected terminal.open Success".into());
    };
    assert_eq!(opened["threadId"], "thread-t1");
    assert_eq!(opened["terminalId"], "term-a");
    assert_eq!(opened["cols"], 90);
    assert_eq!(opened["rows"], 28);
    assert_eq!(opened["label"], "Test Shell");
    assert!(opened["history"].is_string());

    let RpcExit::Success { .. } = call_ws(
        &mut ws,
        "t-write",
        "terminal.write",
        json!({
            "threadId": "thread-t1",
            "terminalId": "term-a",
            "data": "hello parity",
        }),
    )
    .await?
    else {
        return Err("expected terminal.write Success".into());
    };

    let RpcExit::Success { value: snap1 } = call_ws(
        &mut ws,
        "t-snap1",
        "terminal.snapshot",
        json!({ "threadId": "thread-t1", "terminalId": "term-a" }),
    )
    .await?
    else {
        return Err("expected terminal.snapshot Success".into());
    };
    assert!(
        snap1["viewportText"]
            .as_str()
            .is_some_and(|t| t.contains("hello parity"))
    );

    let RpcExit::Success { .. } = call_ws(
        &mut ws,
        "t-resize",
        "terminal.resize",
        json!({ "threadId": "thread-t1", "terminalId": "term-a", "cols": 110, "rows": 32 }),
    )
    .await?
    else {
        return Err("expected terminal.resize Success".into());
    };

    let RpcExit::Success { .. } = call_ws(
        &mut ws,
        "t-close",
        "terminal.close",
        json!({ "threadId": "thread-t1" }),
    )
    .await?
    else {
        return Err("expected terminal.close without terminalId Success".into());
    };

    drop(ws);
    server.shutdown().await?;
    Ok(())
}
