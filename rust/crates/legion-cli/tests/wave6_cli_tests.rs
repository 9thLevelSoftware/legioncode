//! Focused Wave 6 tests for `legioncode policy settings`, `legioncode thread` (`start`, `show` error,
//! `--output-schema`, `--wait --json` woken `output`), Windows `launchers` shims, and MCP aliases.

mod support;

use std::fs;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use legion_cli::cmd_update::repoint_launchers;
use legion_cli::serve::{ServeConfig, ServeMode, spawn_server};
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::sqlite::{SqliteOpenOptions, SqliteStore};
use serde_json::{Value, json};

use support::{TestResult, isolated_env, run_legioncode};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[derive(Default)]
struct MockMcpState {
    calls: Vec<(String, Value)>,
    wait_calls: usize,
}

async fn handle_mock_mcp(
    State(state): State<Arc<Mutex<MockMcpState>>>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let id = payload.get("id").cloned().unwrap_or(json!(1));
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => (
            StatusCode::OK,
            [("mcp-session-id", "mock-sess-1")],
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "protocolVersion": "2025-06-18", "capabilities": { "tools": {} } }
            })),
        )
            .into_response(),
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/call" => {
            let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
            let tool = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let Ok(mut guard) = state.lock() else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            guard.calls.push((tool.clone(), args.clone()));
            let structured = match tool.as_str() {
                "legion_project_list" => json!({
                    "projects": [{ "id": "proj-1", "title": "Proj", "workspaceRoot": "/repo" }],
                    "nextCursor": null
                }),
                "legion_thread_launch" => json!({ "threadId": "thr-1", "runId": "run-1" }),
                "legion_thread_send" => json!({
                    "threadId": "thr-1", "messageId": "msg-1", "runId": "run-1",
                    "status": "running", "delivery": "started"
                }),
                "legion_thread_wait" => {
                    guard.wait_calls += 1;
                    if guard.wait_calls == 1 {
                        json!({ "threadId": "thr-1", "runId": "run-1", "status": "completed", "timedOut": false })
                    } else {
                        json!({
                            "threadId": "thr-1", "runId": "run-wake", "status": "completed", "timedOut": false,
                            "resultRun": { "runId": "run-wake", "status": "completed" },
                            "output": { "verdict": "merge" }
                        })
                    }
                }
                "legion_thread_read" => {
                    if args.get("view").and_then(Value::as_str) == Some("activity") {
                        json!({
                            "thread": {
                                "threadId": "thr-fail", "projectId": "proj-1", "title": "Failed",
                                "status": "failed", "providerInstanceId": "codex", "model": "gpt-5",
                                "branch": null, "worktreePath": null, "latestRunId": "run-fail",
                                "pendingRequestCount": 0, "itemCount": 2
                            },
                            "recentRuns": [{ "runId": "run-fail", "ordinal": 1, "status": "failed" }],
                            "items": [
                                { "position": 0, "itemId": "i0", "runId": "run-0", "type": "error", "text": "older", "textTruncated": false },
                                { "position": 1, "itemId": "i1", "runId": "run-fail", "type": "error", "text": "Usage limit reached", "textTruncated": false }
                            ],
                            "nextPosition": null, "hasMore": false
                        })
                    } else {
                        json!({
                            "thread": {
                                "threadId": "thr-fail", "projectId": "proj-1", "title": "Failed",
                                "status": "failed", "providerInstanceId": "codex", "model": "gpt-5",
                                "branch": null, "worktreePath": null, "latestRunId": "run-fail",
                                "pendingRequestCount": 0, "itemCount": 1
                            },
                            "recentRuns": [
                                { "runId": "run-wake", "ordinal": 2, "status": "completed" },
                                { "runId": "run-1", "ordinal": 1, "status": "completed" }
                            ],
                            "items": [
                                { "position": 0, "itemId": "i-reply", "runId": "run-wake", "type": "assistant_message", "text": "All clean.", "textTruncated": false }
                            ],
                            "nextPosition": null, "hasMore": false
                        })
                    }
                }
                _ => json!({}),
            };
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{ "type": "text", "text": "{}" }], "structuredContent": structured, "isError": false }
            }))
            .into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn spawn_mock_mcp() -> Result<(String, Arc<Mutex<MockMcpState>>), Box<dyn std::error::Error>>
{
    let state = Arc::new(Mutex::new(MockMcpState::default()));
    let app = Router::new()
        .route(
            "/mcp",
            post(handle_mock_mcp).delete(|| async { StatusCode::NO_CONTENT }),
        )
        .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://{addr}"), state))
}

#[test]
fn policy_settings_lists_defaults_and_rejects_extra_args() -> TestResult {
    let temp = tempfile::tempdir()?;
    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);

    let out = run_legioncode(&["policy", "settings"], &env)?;
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8(out.stdout)?;
    assert!(stdout.contains("backgroundActivity.baseProfile = "));
    assert!(stdout.contains("storageCleanup.logsAfterDays = "));
    assert!(stdout.contains("usageModelFilter = (no default)"));

    let help = run_legioncode(&["policy", "--help"], &env)?;
    assert!(String::from_utf8(help.stdout)?.contains("settings"));

    let bad = run_legioncode(&["policy", "settings", "--unknown"], &env)?;
    assert_eq!(bad.status.code(), Some(1));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_show_start_and_woken_output_via_remote_mcp()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let (origin, state) = spawn_mock_mcp().await?;
    let schema_file = temp.path().join("schema.json");
    fs::write(
        &schema_file,
        r#"{"type":"object","properties":{"verdict":{"type":"string"}},"required":["verdict"]}"#,
    )?;
    let schema_str = schema_file.to_string_lossy().into_owned();
    let env = isolated_env(temp.path(), &[("LEGIONCODE_BIN_TS", NONEXISTENT_BIN_TS)]);

    // 1. thread show --json includes latest failed run `error`.
    let show_json = run_legioncode(
        &[
            "thread",
            "show",
            "--remote-url",
            &origin,
            "--token",
            "tok-1",
            "--json",
            "thr-fail",
        ],
        &env,
    )?;
    assert_eq!(show_json.status.code(), Some(0));
    let parsed: Value = serde_json::from_slice(&show_json.stdout)?;
    assert_eq!(parsed["error"], "Usage limit reached");

    let show_txt = run_legioncode(
        &[
            "thread",
            "show",
            "--remote-url",
            &origin,
            "--token",
            "tok-1",
            "thr-fail",
        ],
        &env,
    )?;
    assert_eq!(show_txt.status.code(), Some(0));
    assert!(String::from_utf8(show_txt.stdout)?.contains("Error: Usage limit reached"));

    // 2. thread start --generate-title --output-schema passes fields to legion_thread_launch.
    let started = run_legioncode(
        &[
            "thread",
            "start",
            "--remote-url",
            &origin,
            "--token",
            "tok-1",
            "--generate-title",
            "--output-schema",
            &schema_str,
            "--json",
            "proj-1",
            "Review PR",
        ],
        &env,
    )?;
    assert_eq!(started.status.code(), Some(0));
    let started_json: Value = serde_json::from_slice(&started.stdout)?;
    assert_eq!(started_json["threadId"], "thr-1");

    // 3. thread send --output-schema --wait --json preserves woken run `output`.
    let sent = run_legioncode(
        &[
            "thread",
            "send",
            "--remote-url",
            &origin,
            "--token",
            "tok-1",
            "--output-schema",
            &schema_str,
            "--wait",
            "--json",
            "thr-1",
            "Return verdict",
        ],
        &env,
    )?;
    assert_eq!(sent.status.code(), Some(0));
    let sent_json: Value = serde_json::from_slice(&sent.stdout)?;
    assert_eq!(sent_json["output"]["verdict"], "merge");

    let calls = state
        .lock()
        .map_or_else(|_| Vec::new(), |g| g.calls.clone());
    assert!(
        calls
            .iter()
            .any(|(t, a)| t == "legion_thread_launch" && a["generateTitle"] == true)
    );
    Ok(())
}

#[test]
fn repoints_recorded_windows_launcher_outside_path() -> TestResult {
    let temp = tempfile::tempdir()?;
    let runtime_dir = temp.path().join("home").join("runtime");
    let versions_dir = runtime_dir.join("versions");
    let custom_bin = temp.path().join("custom bin");
    fs::create_dir_all(&versions_dir)?;
    fs::create_dir_all(&custom_bin)?;

    let launcher_name = if cfg!(windows) {
        "legioncode.cmd"
    } else {
        "legioncode"
    };
    let launcher = custom_bin.join(launcher_name);
    let old_exe = versions_dir.join("0.1.10").join("legioncode.exe");
    let new_exe = versions_dir.join("0.1.11").join("legioncode.exe");
    if cfg!(windows) {
        fs::write(
            &launcher,
            format!("@echo off\r\n\"{}\" %*", old_exe.display()),
        )?;
    } else {
        fs::write(
            &launcher,
            format!("#!/bin/sh\nexec '{}' \"$@\"\n", old_exe.display()),
        )?;
    }
    fs::write(
        runtime_dir.join("launchers"),
        format!("{}\r\n", launcher.display()),
    )?;

    repoint_launchers(&versions_dir, &new_exe);
    let after = fs::read_to_string(&launcher)?;
    assert!(
        after.contains(&new_exe.display().to_string()),
        "got: {after}"
    );
    Ok(())
}

#[tokio::test]
async fn live_mcp_dispatches_legion_tools_and_launch_and_submit()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let handle = spawn_server(ServeConfig {
        mode: ServeMode::Web,
        host: Some("127.0.0.1".to_owned()),
        port: 0,
        base_dir: temp.path().to_path_buf(),
        base_dir_is_explicit: true,
        cwd: temp.path().to_path_buf(),
        static_dir: None,
        no_browser: true,
        lan: false,
    })
    .await?;

    let db_path = handle.paths().db_path.clone();
    let store = SqliteStore::open_with_options(
        &db_path,
        SqliteOpenOptions {
            ensure_core_schema: false,
        },
    )?;
    let sess = issue_scoped_session(store.connection(), IssueSessionInput::default())?;
    let _ = store.close();

    let http = reqwest::Client::new();
    let mcp_url = format!("{}/mcp", handle.origin());
    let call = |name: &str, args: Value| {
        let http = http.clone();
        let mcp_url = mcp_url.clone();
        let tok = sess.token.clone();
        let name = name.to_owned();
        async move {
            let resp = http
                .post(&mcp_url)
                .header("Authorization", format!("Bearer {tok}"))
                .header("Accept", "application/json, text/event-stream")
                .header("Content-Type", "application/json")
                .json(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": { "name": name, "arguments": args }
                }))
                .send()
                .await?;
            resp.json::<Value>().await
        }
    };

    let list_res = call("legion_project_list", json!({})).await?;
    assert_eq!(list_res["result"]["isError"], false);

    let bad_title = call(
        "mcp__legion_code__legion_thread_launch",
        json!({ "projectId": "p1", "title": "Fix", "generateTitle": true }),
    )
    .await?;
    assert_eq!(bad_title["result"]["isError"], true);
    assert!(
        bad_title["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Title generation requires a message or attachments")
    );

    handle.shutdown().await?;
    Ok(())
}
