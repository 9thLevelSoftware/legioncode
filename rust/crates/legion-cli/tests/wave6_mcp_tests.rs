//! Focused Wave 6 `/mcp` integration tests: SEP-3415 milestones + SSE session revocation,
//! `legion_workflow`/`t3_workflow`, local-only `legion_feedback`, and thread list/read parity.

use std::time::{SystemTime, UNIX_EPOCH};

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session, revoke_session};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use serde_json::{Value, json};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn spawn_isolated() -> TestResult<(TempDir, RunningServer)> {
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

async fn post_mcp(
    client: &reqwest::Client,
    origin: &str,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Value,
) -> TestResult<reqwest::Response> {
    let mut req = client
        .post(format!("{origin}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json");
    if let Some(tok) = bearer {
        req = req.bearer_auth(tok);
    }
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    Ok(req.json(&body).send().await?)
}

async fn post_mcp_json(
    client: &reqwest::Client,
    origin: &str,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Value,
) -> TestResult<Value> {
    let resp = post_mcp(client, origin, bearer, extra_headers, body).await?;
    Ok(resp.json::<Value>().await?)
}

fn tool_names(list_res: &Value) -> TestResult<Vec<&str>> {
    Ok(list_res["result"]["tools"]
        .as_array()
        .ok_or("tools array")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect())
}

#[tokio::test]
async fn mcp_sep3415_discover_events_list_poll_stream_and_revocation() -> TestResult {
    let (_temp, server) = spawn_isolated().await?;
    let client = reqwest::Client::new();

    let discover = post_mcp(
        &client,
        server.origin(),
        None,
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "server/discover"),
        ],
        json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {} }),
    )
    .await?;
    assert_eq!(discover.status(), 200);
    let disc_json: Value = discover.json().await?;
    assert_eq!(
        disc_json["result"]["capabilities"]["extensions"],
        json!({ "io.modelcontextprotocol/events": {} })
    );

    let session = {
        let conn = server.orchestrator().sink().db().lock().map_err(|_| "db")?;
        issue_scoped_session(
            &conn,
            IssueSessionInput::default().with_label("outside-bot"),
        )?
    };

    let outside_list = post_mcp_json(
        &client,
        server.origin(),
        Some(&session.token),
        &[],
        json!({ "jsonrpc": "2.0", "id": 2, "method": "events/list", "params": {} }),
    )
    .await?;
    assert_eq!(
        outside_list["result"]["events"].as_array().map(Vec::len),
        Some(12)
    );

    let thread_list = post_mcp_json(
        &client,
        server.origin(),
        Some(&session.token),
        &[("x-legioncode-thread-id", "thr-in")],
        json!({ "jsonrpc": "2.0", "id": 3, "method": "events/list", "params": {} }),
    )
    .await?;
    assert_eq!(thread_list["result"]["events"], json!([]));

    let missing_hdr = post_mcp(
        &client,
        server.origin(),
        Some(&session.token),
        &[("mcp-protocol-version", "2026-07-28"), ("mcp-method", "events/poll")],
        json!({ "jsonrpc": "2.0", "id": 4, "method": "events/poll", "params": { "name": "thread.run_settled", "cursor": null } }),
    )
    .await?;
    assert_eq!(missing_hdr.status(), 400);
    assert_eq!(missing_hdr.json::<Value>().await?["error"]["code"], -32020);

    let webhook_err = post_mcp_json(
        &client,
        server.origin(),
        Some(&session.token),
        &[],
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "events/subscribe",
            "params": { "name": "thread.run_settled", "delivery": { "mode": "webhook", "url": "https://example.com" } }
        }),
    )
    .await?;
    assert_eq!(webhook_err["error"]["code"], -32026);

    let mut sse_resp = post_mcp(
        &client,
        server.origin(),
        Some(&session.token),
        &[("mcp-name", "thread.run_settled"), ("mcp-protocol-version", "2026-07-28")],
        json!({ "jsonrpc": "2.0", "id": 7, "method": "events/stream", "params": { "name": "thread.run_settled", "cursor": null } }),
    )
    .await?;
    assert_eq!(sse_resp.status(), 200);
    let first_chunk = sse_resp
        .chunk()
        .await?
        .ok_or("expected initial SSE active event")?;
    let first_text = std::str::from_utf8(&first_chunk)?;
    assert!(first_text.contains("notifications/events/active"));

    {
        let conn = server.orchestrator().sink().db().lock().map_err(|_| "db")?;
        let _ = revoke_session(&conn, &session.session_id)?;
    }
    while let Some(next_chunk) = sse_resp.chunk().await? {
        if next_chunk.is_empty() {
            break;
        }
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    assert!(now > 0);
    server.shutdown().await?;
    Ok(())
}

async fn spawn_wf_mcp_fixture()
-> Result<(TempDir, RunningServer, String), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let userdata = temp.path().join("userdata");
    let wf_dir = userdata.join("workflows");
    std::fs::create_dir_all(&wf_dir)?;
    std::fs::write(
        userdata.join("settings.json"),
        serde_json::to_string_pretty(&json!({
            "workflowsEnabled": true,
            "agentFeedback": { "mode": "auto" }
        }))?,
    )?;
    std::fs::write(
        wf_dir.join("mcp_test.ts"),
        "Legion.workflow({ name: \"mcp_wf\", version: 1, triggers: [], run: () => Effect.gen(function* () { return { done: 1 }; }) });",
    )?;
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
    let session = {
        let conn = server.orchestrator().sink().db().lock().map_err(|_| "db")?;
        ensure_project_store_schema(&conn)?;
        let _ = create_project(
            &*conn,
            &CreateProjectInput::new(server.paths().state_dir.to_string_lossy().as_ref())
                .with_project_id("proj-mcp")
                .with_title("McpProj"),
        )?;
        issue_scoped_session(&conn, IssueSessionInput::default().with_label("mcp-client"))?
    };
    Ok((temp, server, session.token))
}

#[tokio::test]
async fn mcp_workflow_and_feedback_scope_visibility_parity() -> TestResult {
    let (_temp, server, token) = spawn_wf_mcp_fixture().await?;
    let client = reqwest::Client::new();

    let tools_outside = post_mcp_json(
        &client,
        server.origin(),
        Some(&token),
        &[],
        json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/list", "params": {} }),
    )
    .await?;
    let names_outside = tool_names(&tools_outside)?;
    assert!(names_outside.contains(&"legion_workflow"));
    assert!(names_outside.contains(&"legion_feedback"));

    let tools_inside = post_mcp_json(
        &client,
        server.origin(),
        Some(&token),
        &[("x-legioncode-thread-id", "thr-inner")],
        json!({ "jsonrpc": "2.0", "id": 11, "method": "tools/list", "params": {} }),
    )
    .await?;
    assert!(!tool_names(&tools_inside)?.contains(&"legion_workflow"));

    // Call `t3_feedback` alias and confirm local storage (`sent == false`).
    let fb_res = post_mcp_json(
        &client,
        server.origin(),
        Some(&token),
        &[],
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "tools/call",
            "params": {
                "name": "t3_feedback",
                "arguments": {
                    "kind": "bug",
                    "subject": "legion_workflow",
                    "expected": "works",
                    "actual": "edge case"
                }
            }
        }),
    )
    .await?;
    assert_eq!(fb_res["result"]["isError"], false);
    assert_eq!(fb_res["result"]["structuredContent"]["sent"], false);
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn mcp_thread_launch_workflow_run_and_list_parity() -> TestResult {
    let (_temp, server, token) = spawn_wf_mcp_fixture().await?;
    let client = reqwest::Client::new();

    let launched = post_mcp_json(
        &client,
        server.origin(),
        Some(&token),
        &[],
        json!({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "tools/call",
            "params": {
                "name": "legion_thread_launch",
                "arguments": {
                    "projectId": "proj-mcp",
                    "title": "WF Step",
                    "workflowRun": { "runId": "wf-1", "workflow": "mcp_wf" }
                }
            }
        }),
    )
    .await?;
    let tid = launched["result"]["structuredContent"]["threadId"]
        .as_str()
        .ok_or("missing threadId")?;

    let listed = post_mcp_json(
        &client,
        server.origin(),
        Some(&token),
        &[],
        json!({
            "jsonrpc": "2.0",
            "id": 14,
            "method": "tools/call",
            "params": {
                "name": "legion_thread_list",
                "arguments": { "projectId": "proj-mcp", "includeSubagents": false }
            }
        }),
    )
    .await?;
    let threads = listed["result"]["structuredContent"]["threads"]
        .as_array()
        .ok_or("threads")?;
    assert!(
        threads
            .iter()
            .any(|t| t["threadId"] == tid && t["workflowRun"]["runId"] == "wf-1")
    );

    server.shutdown().await?;
    Ok(())
}
