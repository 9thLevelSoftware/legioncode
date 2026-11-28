//! Integration tests for M1.1 Rust WS Scope Authorization & Client-Only Guards (`R5`).

#[path = "support/ws_auth_helpers.rs"]
mod helpers;

use helpers::{
    TestResult, assert_auth_denied, call_unary, connect_ws, issue_token, pair_standard_client,
    spawn_srv,
};
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

#[tokio::test]
async fn ws_rejects_mcp_client_upgrade_and_enforces_narrow_scopes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-scope-narrow")).await?;
    let mcp_client_tok = issue_token(&server, "mcp-client", &["orchestration:read"])?;
    let read_tok = issue_token(&server, "one-time-token", &["orchestration:read"])?;
    let operate_tok = issue_token(&server, "one-time-token", &["orchestration:operate"])?;

    let mut stream = TcpStream::connect(server.addr()).await?;
    let req = format!(
        "GET /ws?orchestrationProtocol=2&wsTicket={mcp_client_tok} HTTP/1.1\r\nHost: {}\r\n\
         Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        server.addr()
    );
    stream.write_all(req.as_bytes()).await?;
    let mut buf = [0_u8; 512];
    let n = stream.read(&mut buf).await?;
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 401"));

    let mut ws_read = connect_ws(&server, &read_tok).await?;
    assert!(matches!(
        call_unary(&mut ws_read, "r1", "server.probe", json!({})).await?,
        RpcExit::Success { .. }
    ));
    assert_auth_denied(
        call_unary(
            &mut ws_read,
            "r2",
            "orchestration.dispatchCommand",
            json!({ "type": "thread.stop", "commandId": "c1", "threadId": "t1" }),
        )
        .await?,
        "orchestration:operate",
        "orchestration:operate",
    );
    assert_auth_denied(
        call_unary(
            &mut ws_read,
            "r3",
            "orchestration.v2.launchThread",
            json!({}),
        )
        .await?,
        "orchestration:operate",
        "orchestration:operate",
    );
    assert_auth_denied(
        call_unary(
            &mut ws_read,
            "r4",
            "terminal.attach",
            json!({ "threadId": "t1", "terminalId": "term-1" }),
        )
        .await?,
        "terminal:operate",
        "terminal:operate",
    );

    let mut ws_op = connect_ws(&server, &operate_tok).await?;
    assert_auth_denied(
        call_unary(
            &mut ws_op,
            "o1",
            "server.updateSettings",
            json!({ "patch": { "defaultAutoPull": true } }),
        )
        .await?,
        "orchestration:operate",
        "settings:write",
    );
    assert_auth_denied(
        call_unary(
            &mut ws_op,
            "o2",
            "git.runStackedAction",
            json!({ "actionId": "a1", "cwd": "/repo", "action": "commit" }),
        )
        .await?,
        "orchestration:operate",
        "source-control:write",
    );

    server.shutdown().await?;
    Ok(())
}

fn build_operate_bypass_cases(
    root: &std::path::Path,
) -> Vec<(&'static str, &'static str, Value, &'static str)> {
    vec![
        (
            "o3",
            "projects.mutate",
            json!({
                "action": "project.create",
                "workspaceRoot": root.join("p4a").to_string_lossy(),
                "scripts": [{ "id": "s1", "name": "evil", "command": "echo pwned" }]
            }),
            "settings:write",
        ),
        (
            "o4",
            "projects.mutate",
            json!({
                "workspaceRoot": root.join("p4a-omitted").to_string_lossy(),
                "scripts": [{
                    "id": "s1",
                    "env": { "provider": "env", "reference": "SECRET_TOKEN" }
                }]
            }),
            "settings:write",
        ),
        (
            "o5",
            "orchestration.v2.dispatchCommand",
            json!({
                "type": "project.create",
                "commandId": "cmd-4b-1",
                "title": "Bypass4B",
                "workspaceRoot": root.join("p4b").to_string_lossy(),
                "scripts": [{
                    "id": "s1",
                    "env": { "provider": "env", "reference": "SECRET_TOKEN" }
                }]
            }),
            "settings:write",
        ),
        (
            "o6",
            "orchestration.v2.dispatchCommand",
            json!({
                "type": "project.update",
                "commandId": "cmd-4b-2",
                "projectId": "proj-1",
                "scripts": [{ "id": "s1", "command": "echo hi" }]
            }),
            "settings:write",
        ),
        (
            "o7",
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.turn.start",
                "commandId": "cmd-4c-1",
                "threadId": "thr-1",
                "message": { "messageId": "m1", "text": "/logout", "attachments": [] }
            }),
            "providers:manage",
        ),
    ]
}

#[tokio::test]
async fn ws_blocks_dynamic_alias_and_project_command_bypasses() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-scope-bypasses")).await?;
    let read_tok = issue_token(&server, "one-time-token", &["orchestration:read"])?;
    let operate_tok = issue_token(&server, "one-time-token", &["orchestration:operate"])?;

    let mut ws_read = connect_ws(&server, &read_tok).await?;
    let mut ws_op = connect_ws(&server, &operate_tok).await?;

    for (id, method, payload, expected_perm) in build_operate_bypass_cases(temp.path()) {
        assert_auth_denied(
            call_unary(&mut ws_op, id, method, payload).await?,
            "orchestration:operate",
            expected_perm,
        );
    }

    for (id, resource) in [
        (
            "r5",
            json!({ "kind": "workspace-file", "path": "secret.txt" }),
        ),
        (
            "r6",
            json!({ "_tag": null, "kind": "workspace-file", "path": "secret.txt" }),
        ),
        (
            "r7",
            json!({ "_tag": "native-app-icon", "app": { "_tag": null, "kind": "app-path" } }),
        ),
    ] {
        assert_auth_denied(
            call_unary(
                &mut ws_read,
                id,
                "assets.createUrl",
                json!({ "resource": resource }),
            )
            .await?,
            "orchestration:read",
            "filesystem:read",
        );
    }

    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_enforces_client_only_guards_against_agent_tokens() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-client-only")).await?;
    let thread_tok = issue_token(
        &server,
        "thread:thr-agent-1",
        &["orchestration:read", "orchestration:operate"],
    )?;
    let client_tok = pair_standard_client(&server).await?;

    let mut ws_thread = connect_ws(&server, &thread_tok).await?;
    assert_auth_denied(
        call_unary(
            &mut ws_thread,
            "t1",
            "workflowBoard.approveSpec",
            json!({ "cardId": "c1" }),
        )
        .await?,
        "orchestration:operate",
        "client:interactive",
    );

    let mut ws_client = connect_ws(&server, &client_tok).await?;
    match call_unary(
        &mut ws_client,
        "c1",
        "workflowBoard.approveSpec",
        json!({ "cardId": "c1", "expectedRevision": 1 }),
    )
    .await?
    {
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                panic!("expected Fail cause");
            };
            assert_ne!(error["_tag"], "EnvironmentAuthorizationError");
            assert_eq!(error["_tag"], "WorkflowBoardRequestError");
        }
        other @ RpcExit::Success { .. } => panic!("unexpected result: {other:?}"),
    }

    server.shutdown().await?;
    Ok(())
}

#[path = "ws_scope_auth/asset_cases.rs"]
mod asset_cases;
#[path = "ws_scope_auth/m2_cases.rs"]
mod m2_cases;
#[path = "ws_scope_auth/m3_cases.rs"]
mod m3_cases;
#[path = "ws_scope_auth/m4_cases.rs"]
mod m4_cases;
