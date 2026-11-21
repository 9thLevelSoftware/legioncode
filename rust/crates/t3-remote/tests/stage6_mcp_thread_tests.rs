//! Live loopback HTTP/SSE tests for `acp_mcp_bridge` (`BUG-06`) and `thread_client`.

use std::fs;
use std::sync::Arc;

use serde_json::{Value, json};
use t3_remote::acp_mcp_bridge::{
    MCP_PROTOCOL_VERSION, McpHttpClient, call_mcp_tool, run_mcp_stdio_bridge,
};
use t3_remote::thread_client::{
    THREAD_WAIT_SLICE_MS, ThreadClientError, ThreadListRequest, ThreadMcpClient, ThreadReadRequest,
    ThreadSendDelivery, ThreadSendMode, ThreadSendRequest, ThreadWaitOutcome,
    discover_live_server_runtime, format_thread_list_line, format_thread_send_ack,
    format_title_from_message,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct MockState {
    methods: Vec<String>,
    deleted: bool,
    ping_replied: bool,
    use_sse_with_ping: bool,
}

async fn read_http_req(sock: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_len = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_len {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = buf[header_end..(header_end + content_len).min(buf.len())].to_vec();
    Some((head, body))
}

async fn write_http_resp(sock: &mut TcpStream, status: &str, ctype: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nMcp-Session-Id: sess-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
}

async fn spawn_mock_mcp_server(
    use_sse_with_ping: bool,
) -> Result<(String, Arc<Mutex<MockState>>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let state = Arc::new(Mutex::new(MockState {
        use_sse_with_ping,
        ..MockState::default()
    }));
    let state_srv = Arc::clone(&state);

    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let st = Arc::clone(&state_srv);
            tokio::spawn(async move {
                let Some((head, body)) = read_http_req(&mut sock).await else {
                    return;
                };
                if head.starts_with("DELETE ") {
                    st.lock().await.deleted = true;
                    write_http_resp(&mut sock, "204 No Content", "text/plain", "").await;
                    return;
                }
                let Ok(req_json) = serde_json::from_slice::<Value>(&body) else {
                    return;
                };
                let id = req_json.get("id").cloned();
                let method = req_json
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let mut guard = st.lock().await;
                if id.as_ref().and_then(Value::as_str) == Some("srv-ping-1")
                    && req_json.get("result").is_some()
                {
                    guard.ping_replied = true;
                    write_http_resp(&mut sock, "202 Accepted", "application/json", "").await;
                    return;
                }
                if !method.is_empty() {
                    guard.methods.push(method.clone());
                }
                if method == "notifications/initialized" {
                    write_http_resp(&mut sock, "202 Accepted", "application/json", "").await;
                    return;
                }
                let use_sse = guard.use_sse_with_ping;
                drop(guard);

                if method == "tools/call" && use_sse {
                    let payload =
                        mock_tool_result(req_json["params"]["name"].as_str().unwrap_or(""));
                    let sse = format!(
                        "data: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\n\n\
                         data: {{\"jsonrpc\":\"2.0\",\"id\":\"srv-ping-1\",\"method\":\"ping\"}}\n\n\
                         data: {}\n\n",
                        json!({"jsonrpc": "2.0", "id": id, "result": payload})
                    );
                    write_http_resp(&mut sock, "200 OK", "text/event-stream", &sse).await;
                    return;
                }
                let result = match method.as_str() {
                    "initialize" => {
                        json!({"protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {}, "serverInfo": {"name": "t3-mock", "version": "0.1.10"}})
                    }
                    "tools/call" => {
                        mock_tool_result(req_json["params"]["name"].as_str().unwrap_or(""))
                    }
                    _ => json!({}),
                };
                let json_body = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
                write_http_resp(&mut sock, "200 OK", "application/json", &json_body).await;
            });
        }
    });
    Ok((format!("http://{addr}"), state))
}

fn mock_tool_result(tool: &str) -> Value {
    match tool {
        "t3_thread_list" => json!({
            "structuredContent": {
                "projectId": "proj-1", "nextCursor": null, "total": 1,
                "threads": [{
                    "threadId": "th-1", "projectId": "proj-1", "title": "Fix bug",
                    "status": "running", "latestRunId": "run-1", "providerInstanceId": "codex",
                    "model": "gpt-5", "itemCount": 2,
                    "createdAt": "2026-11-20T00:00:00Z", "updatedAt": "2026-11-20T00:00:00Z"
                }]
            }
        }),
        "t3_thread_send" => json!({
            "structuredContent": {
                "threadId": "th-1", "messageId": "m-1", "runId": "run-1",
                "status": "running", "delivery": "queued"
            }
        }),
        "t3_thread_wait" => json!({
            "structuredContent": {"threadId": "th-1", "runId": "run-1", "status": "running", "timedOut": true}
        }),
        "t3_thread_read" => json!({
            "structuredContent": {
                "thread": {
                    "threadId": "th-1", "projectId": "proj-1", "title": "Fix bug",
                    "status": "running", "providerInstanceId": "codex", "model": "gpt-5",
                    "branch": null, "worktreePath": null, "pendingRequestCount": 1, "itemCount": 2
                },
                "recentRuns": [{"runId": "run-1", "ordinal": 1, "status": "running"}],
                "items": [], "nextPosition": null, "hasMore": false
            }
        }),
        _ => {
            let sc = json!({ "echo": "hello" });
            json!({
                "structuredContent": sc,
                "content": [
                    { "type": "text", "text": sc.to_string() },
                    { "type": "text", "text": "Keep readable summary." }
                ]
            })
        }
    }
}

#[tokio::test]
async fn acp_mcp_client_sse_ping_and_delete_on_close_bug06() -> TestResult {
    let (origin, state) = spawn_mock_mcp_server(true).await?;
    let client =
        McpHttpClient::connect(format!("{origin}/mcp"), "Bearer tok", "test-client").await?;
    assert_eq!(client.session_id().await.as_deref(), Some("sess-1"));
    assert_eq!(
        client.protocol_version().await.as_deref(),
        Some(MCP_PROTOCOL_VERSION)
    );

    let res = client.call_tool("echo_tool", &json!({"x": 1})).await?;
    assert_eq!(res["structuredContent"]["echo"], "hello");
    client.close().await;

    let st = state.lock().await;
    assert!(st.ping_replied);
    assert!(st.deleted);
    assert_eq!(
        st.methods,
        vec!["initialize", "notifications/initialized", "tools/call"]
    );
    Ok(())
}

#[tokio::test]
async fn acp_mcp_call_and_stdio_bridge_barrier() -> TestResult {
    let (origin, state) = spawn_mock_mcp_server(false).await?;
    let endpoint = format!("{origin}/mcp");

    let called = call_mcp_tool(&endpoint, "Bearer tok", "echo_tool", "{}").await?;
    let parts = called["content"].as_array().ok_or("expected content")?;
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "Keep readable summary.");
    assert!(state.lock().await.deleted);
    assert!(
        call_mcp_tool(&endpoint, "Bearer tok", "echo_tool", "[]")
            .await
            .is_err()
    );

    let input = concat!(
        "{bad json\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo_tool","arguments":{}}}"#,
        "\n"
    );
    let (client_out, server_out) = tokio::io::duplex(8192);
    run_mcp_stdio_bridge(
        &endpoint,
        "Bearer tok",
        BufReader::new(input.as_bytes()),
        server_out,
    )
    .await?;
    let mut lines = tokio::io::BufReader::new(client_out).lines();
    let mut responses = Vec::new();
    while let Some(line) = lines.next_line().await? {
        responses.push(serde_json::from_str::<Value>(&line)?);
    }
    assert!(responses.iter().any(|v| v["error"]["code"] == -32700));
    assert!(
        responses
            .iter()
            .any(|v| v["id"] == 1 && v["result"]["serverInfo"]["name"] == "t3-mock")
    );
    assert!(
        responses
            .iter()
            .any(|v| v["id"] == 2 && v["result"]["structuredContent"]["echo"] == "hello")
    );
    Ok(())
}

#[tokio::test]
async fn thread_client_discovery_exit2_and_cli_formatting() -> TestResult {
    assert_eq!(THREAD_WAIT_SLICE_MS, 15_000);
    let empty_dir = tempfile::tempdir()?;
    let Err(offline_err) = discover_live_server_runtime(empty_dir.path()) else {
        return Err("expected offline server error".into());
    };
    assert_eq!(offline_err.exit_code(), 1);
    assert_eq!(
        offline_err.to_string(),
        format!(
            "No running T3 Code server uses {}. Start T3 Code, then retry.",
            empty_dir.path().display()
        )
    );

    let (origin, state) = spawn_mock_mcp_server(false).await?;
    let dev_dir = empty_dir.path().join("dev");
    fs::create_dir_all(&dev_dir)?;
    let port: u16 = origin.rsplit_once(':').ok_or("port")?.1.parse()?;
    fs::write(
        dev_dir.join("server-runtime.json"),
        json!({"version": 1, "pid": std::process::id(), "port": port, "origin": origin, "startedAt": "2026-11-20T00:00:00.000Z"}).to_string(),
    )?;

    let client = ThreadMcpClient::connect_base_dir(empty_dir.path(), "tok-secret").await?;
    let listed = client.list_threads(&ThreadListRequest::default()).await?;
    assert_eq!(listed.threads.len(), 1);
    assert_eq!(
        format_thread_list_line(&listed.threads[0]),
        "th-1  running      Fix bug"
    );

    let sent = client
        .send_message(&ThreadSendRequest {
            thread_id: "th-1".to_owned(),
            message: "Check status".to_owned(),
            mode: Some(ThreadSendMode::Queue),
            ..ThreadSendRequest::default()
        })
        .await?;
    assert_eq!(sent.delivery, ThreadSendDelivery::Queued);
    assert_eq!(format_thread_send_ack(sent.delivery), "Queued.");

    let outcome = client.wait_for_run("th-1", "run-1", 100).await?;
    assert_eq!(outcome, ThreadWaitOutcome::NeedsInput);
    let Err(err) = client
        .finish_run_error_check("th-1", "run-1", 100, None)
        .await
    else {
        return Err("expected NeedsInput error".into());
    };
    assert_eq!(err.exit_code(), 2);
    assert!(matches!(err, ThreadClientError::NeedsInput { .. }));

    let _ = client
        .read_thread(&ThreadReadRequest {
            thread_id: "th-1".to_owned(),
            ..ThreadReadRequest::default()
        })
        .await?;
    client.close().await;
    assert!(state.lock().await.deleted);

    let long_title = format_title_from_message(&"a".repeat(90));
    assert_eq!(long_title.chars().count(), 80);
    assert!(long_title.ends_with('…'));
    Ok(())
}
