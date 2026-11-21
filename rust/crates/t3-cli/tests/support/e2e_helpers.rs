//! Shared WebSocket RPC and git repo fixtures for `standalone_node_free_tests`.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use t3_protocol::rpc_codec::{
    ClientRpcFrame, RpcExit, RpcId, ServerRpcFrame, decode_server_frames, encode_client_frame,
};

use super::support::TestResult;
use super::ws_client::WsTestClient;

pub(super) async fn rpc_call(
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
    match decode_server_frames(&ws.recv_text().await?)?
        .into_iter()
        .next()
    {
        Some(ServerRpcFrame::Exit { exit, .. }) => Ok(exit),
        other => Err(format!("expected Exit, got {other:?}").into()),
    }
}

pub(super) fn init_git_repo(dir: &Path) -> TestResult {
    for args in [
        &["init"][..],
        &["config", "user.name", "Test User"][..],
        &["config", "user.email", "test@example.com"][..],
    ] {
        assert!(
            Command::new("git")
                .current_dir(dir)
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(args)
                .status()?
                .success()
        );
    }
    fs::write(dir.join("README.md"), "base\n")?;
    assert!(
        Command::new("git")
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["add", "README.md"])
            .status()?
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["commit", "-m", "init"])
            .status()?
            .success()
    );
    fs::write(dir.join("dirty.txt"), "dirty-content\n")?;
    Ok(())
}
