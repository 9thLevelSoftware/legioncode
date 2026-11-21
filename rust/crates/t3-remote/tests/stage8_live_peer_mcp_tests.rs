#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use t3_contracts::ids::{EnvironmentId, NodeId, RunId, ThreadId};
use t3_contracts::orchestration::ProviderInteractionMode;
use t3_contracts::policy::RuntimeMode;
use t3_protocol::mcp::ModeCeiling;
use t3_remote::live_network::{LivePeerMcpOptions, LivePeerMcpSession, PeerMcpError};
use t3_remote::peer::delegation::{
    CallerCancelReason, FollowAction, RemoteChildRef, RemoteDelegationTracker, RemoteStopCommand,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

#[derive(Default)]
struct MockMcpState {
    mode_limits: Vec<String>,
    sender_threads: Vec<String>,
    interrupts: Vec<Value>,
    deleted_sessions: Vec<String>,
}

async fn read_http(
    stream: &mut TcpStream,
) -> Option<(String, String, Vec<(String, String)>, Vec<u8>)> {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 4096];
    let head_end;
    loop {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            head_end = pos + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&raw[..head_end - 4]);
    let mut lines = head.lines();
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    let mut content_len = 0_usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            if key == "content-length" {
                content_len = val.parse().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }
    while raw.len() - head_end < content_len {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    Some((
        method,
        path,
        headers,
        raw[head_end..head_end + content_len].to_vec(),
    ))
}

async fn write_http(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    extra: &[(&str, &str)],
    body: &str,
) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        let _ = write!(head, "{k}: {v}\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
}

async fn handle_mcp_conn(mut stream: TcpStream, state: Arc<Mutex<MockMcpState>>) {
    let Some((method, path, headers, body)) = read_http(&mut stream).await else {
        return;
    };
    let hdr = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    if path != "/mcp" || hdr("authorization").as_deref() != Some("Bearer tok-mcp-1") {
        write_http(&mut stream, "401 Unauthorized", "text/plain", &[], "denied").await;
        return;
    }
    if method == "DELETE" {
        if let Some(sid) = hdr("mcp-session-id")
            && let Ok(mut guard) = state.lock()
        {
            guard.deleted_sessions.push(sid);
        }
        write_http(&mut stream, "204 No Content", "text/plain", &[], "").await;
        return;
    }
    if method == "POST" {
        let req: Value = serde_json::from_slice(&body).unwrap_or_default();
        if let Ok(mut guard) = state.lock() {
            if let Some(m) = hdr("t3-mode-limit") {
                guard.mode_limits.push(m);
            }
            if let Some(s) = hdr("t3-sender-thread") {
                guard.sender_threads.push(s);
            }
        }
        dispatch_mcp_rpc(&mut stream, &state, &req).await;
    }
}

async fn dispatch_mcp_rpc(stream: &mut TcpStream, state: &Arc<Mutex<MockMcpState>>, req: &Value) {
    let rpc_method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let req_id = req.get("id").cloned().unwrap_or(Value::Null);
    match rpc_method {
        "initialize" => {
            let sse = format!(
                "data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/ping\"}}\n\n\
                 data: {{\"jsonrpc\":\"2.0\",\"id\":{req_id},\"result\":{{\"protocolVersion\":\"2025-06-18\"}}}}\n\n"
            );
            write_http(
                stream,
                "200 OK",
                "text/event-stream",
                &[
                    ("mcp-session-id", "mcp-sess-99"),
                    ("mcp-protocol-version", "2025-06-18"),
                ],
                &sse,
            )
            .await;
        }
        "notifications/initialized" => {
            write_http(stream, "202 Accepted", "text/plain", &[], "").await;
        }
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or("");
            let args = req["params"]["arguments"].clone();
            let payload = match name {
                "echo_tool" => json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "structuredContent": { "echoed": args["msg"], "ok": true },
                        "content": [{ "type": "text", "text": json!({ "echoed": args["msg"], "ok": true }).to_string() }]
                    }
                }),
                "t3_thread_wait" => json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "result": {
                        "isError": true,
                        "content": [{ "type": "text", "text": json!({ "code": "thread_not_found", "message": "child thread gone" }).to_string() }]
                    }
                }),
                "t3_thread_interrupt" => {
                    if let Ok(mut guard) = state.lock() {
                        guard.interrupts.push(args);
                    }
                    json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": { "structuredContent": { "status": "interrupt_requested" }, "content": [] }
                    })
                }
                _ => {
                    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32601, "message": "unknown tool" } })
                }
            };
            let sse = format!(
                "data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{}}}}\n\n\
                 data: {payload}\n\n"
            );
            write_http(stream, "200 OK", "text/event-stream", &[], &sse).await;
        }
        _ => write_http(stream, "404 Not Found", "text/plain", &[], "nope").await,
    }
}

#[tokio::test]
async fn live_peer_mcp_session_sse_bug06_and_delegation_bug07()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(Mutex::new(MockMcpState::default()));
    let (tx, mut rx) = watch::channel(false);
    let srv_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = rx.changed() => if *rx.borrow() { break; },
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break; };
                    tokio::spawn(handle_mcp_conn(stream, Arc::clone(&srv_state)));
                }
            }
        }
    });

    let opts = LivePeerMcpOptions {
        mode_limit: Some(ModeCeiling {
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Plan,
        }),
        sender_thread: Some((
            EnvironmentId::new("env-self")?,
            ThreadId::new("parent-1")?,
            "Parent Chat".to_owned(),
        )),
        ..LivePeerMcpOptions::default()
    };
    let mut session =
        LivePeerMcpSession::connect(&format!("http://127.0.0.1:{port}"), "tok-mcp-1", opts).await?;
    assert_eq!(session.session_id(), Some("mcp-sess-99"));

    let echoed = session
        .call_tool("echo_tool", json!({ "msg": "hi" }))
        .await?;
    assert_eq!(echoed["echoed"], "hi");

    let mut tracker = RemoteDelegationTracker::new(
        NodeId::new("task-1")?,
        ThreadId::new("parent-1")?,
        RemoteChildRef {
            environment_id: EnvironmentId::new("env-remote-peer")?,
            thread_id: ThreadId::new("child-1")?,
            label: "Remote Peer".to_owned(),
            title: Some("Subtask".to_owned()),
            run_id: Some(RunId::new("run-1")?),
            stopped_there: None,
            stop_count: None,
        },
    );
    let _ = tracker.on_caller_cancel(CallerCancelReason::ParentDeleted);
    let follow = session.wait_delegated_slice(&mut tracker).await?;
    assert!(matches!(
        follow,
        FollowAction::SendStop(RemoteStopCommand::StopThere { .. })
    ));

    let refuse = session.call_tool("t3_thread_wait", json!({})).await;
    assert!(matches!(refuse, Err(PeerMcpError::ToolRefused { .. })));

    session.close().await?;

    {
        let guard = state.lock().map_err(|_| "lock poisoned")?;
        assert!(
            guard
                .mode_limits
                .iter()
                .any(|m| m == "approval-required/plan")
        );
        assert!(
            guard
                .sender_threads
                .iter()
                .any(|s| s == "env-self/parent-1/Parent%20Chat")
        );
        assert_eq!(guard.interrupts.len(), 1);
        assert_eq!(guard.interrupts[0]["stop"], true);
        assert_eq!(guard.deleted_sessions, vec!["mcp-sess-99".to_owned()]);
    }

    let _ = tx.send(true);
    Ok(())
}
