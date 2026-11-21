//! Stage 9 Wave V1 integration tests verifying `T3CODE_LEGACY_NODE=1` propagation on forced
//! fallback, default static bundle discovery in `t3 serve`, and `t3 thread` SQLite session
//! fallback against `/mcp`-only servers with `FutureMigration` rows.

#![forbid(unsafe_code)]

mod support;

use std::fs;
use std::path::Path;
use std::process::Stdio;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use axum::routing::any;
use serde_json::{Value, json};
use t3_cli::serve::config::resolve_default_static_dir_from;
use t3_persistence::ownership::{PersistedServerRuntimeState, persist_server_runtime_state};
use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::sqlite::SqliteStore;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;

use support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[test]
fn delegate_to_node_sets_t3code_legacy_node_env_on_forced_unit() -> TestResult {
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fake-bin.js");
    fs::write(
        &fake_bin,
        "process.stdout.write(JSON.stringify({ legacy: process.env.T3CODE_LEGACY_NODE ?? null, args: process.argv.slice(2) }));\n",
    )?;
    let fake_str = fake_bin.to_string_lossy().into_owned();
    let env = isolated_env(
        temp.path(),
        &[
            ("T3_BIN_TS", &fake_str),
            ("RUST_ANYTHING_UNIT_VERSION", "original"),
        ],
    );

    let out = run_t3(&["--version"], &env)?;
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(parsed["legacy"], "1");
    assert_eq!(parsed["args"], json!(["--version"]));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_auto_discovers_static_dirs_and_serves_client_index() -> TestResult {
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("cwd");
    let exe_dir = temp.path().join("bundle").join("bin");
    let exe_client = exe_dir.join("client");
    let sibling_webapp = temp.path().join("bundle").join("web-app");
    let cwd_client = cwd.join("client");
    fs::create_dir_all(&cwd_client)?;
    fs::write(cwd_client.join("index.html"), "cwd-client")?;
    assert_eq!(
        resolve_default_static_dir_from(Some(&exe_dir), &cwd),
        Some(cwd_client.clone())
    );

    fs::create_dir_all(&sibling_webapp)?;
    fs::write(sibling_webapp.join("index.html"), "sibling-webapp")?;
    assert_eq!(
        resolve_default_static_dir_from(Some(&exe_dir), &cwd),
        Some(sibling_webapp)
    );

    fs::create_dir_all(&exe_client)?;
    fs::write(
        exe_client.join("index.html"),
        "<!doctype html><html>stage9-client</html>",
    )?;
    assert_eq!(
        resolve_default_static_dir_from(Some(&exe_dir), &cwd),
        Some(exe_client)
    );

    let copied_bin = exe_dir.join(if cfg!(windows) { "t3.exe" } else { "t3" });
    fs::copy(env!("CARGO_BIN_EXE_t3"), &copied_bin)?;
    let home = temp.path().join("srv-home");
    let body = fetch_root_from_spawned_bin(&copied_bin, &home, &cwd, None).await?;
    assert!(body.contains("stage9-client"));

    let override_dir = temp.path().join("override-static");
    fs::create_dir_all(&override_dir)?;
    fs::write(override_dir.join("index.html"), "override-static-html")?;
    let override_body =
        fetch_root_from_spawned_bin(&copied_bin, &home, &cwd, Some(&override_dir)).await?;
    assert!(override_body.contains("override-static-html"));
    Ok(())
}

async fn fetch_root_from_spawned_bin(
    bin: &Path,
    home: &Path,
    cwd: &Path,
    static_override: Option<&Path>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(bin);
    cmd.args(["serve", "--port", "0", "--no-browser", "--base-dir"])
        .arg(home)
        .current_dir(cwd)
        .env_remove("T3CODE_STATIC_DIR")
        .env("T3_BIN_TS", NONEXISTENT_BIN_TS)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = static_override {
        cmd.arg("--static-dir").arg(dir);
    }

    let mut child = cmd.spawn()?;
    let mut reader = BufReader::new(child.stdout.take().ok_or("missing stdout")?);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let origin = line
        .trim()
        .strip_prefix("T3 Code server listening on ")
        .ok_or_else(|| format!("unexpected server banner: {line:?}"))?
        .to_owned();

    let body = reqwest::get(format!("{origin}/")).await?.text().await?;
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_list_json_with_future_migration_against_mcp_only_server() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("t3-cli-json");
    let paths = ServerDerivedPaths::derive(&root, false, true);
    let store = SqliteStore::open(&paths.db_path)?;
    store.connection().execute(
        "INSERT INTO effect_sql_migrations (migration_id, name) VALUES (?1, 'FutureMigration')",
        rusqlite::params![999_999_i64],
    )?;
    store.close()?;

    let root_str = root.to_string_lossy().into_owned();
    let (port, tx, handle) = spawn_mcp_only_server(root_str.clone()).await?;
    persist_server_runtime_state(
        &paths.server_runtime_state_path,
        &PersistedServerRuntimeState::for_current_process(port, Some("127.0.0.1")),
    )?;

    let temp_owned = temp.path().to_path_buf();
    let root_owned = root_str.clone();
    let out = tokio::task::spawn_blocking(move || {
        let env = isolated_env(
            &temp_owned,
            &[
                ("T3_BIN_TS", NONEXISTENT_BIN_TS),
                ("T3CODE_HOME", &root_owned),
            ],
        );
        run_t3(
            &[
                "thread",
                "list",
                "--project",
                "project-1",
                "--json",
                "--base-dir",
                &root_owned,
            ],
            &env,
        )
    })
    .await??;

    let _ = tx.send(());
    let _ = handle.await;

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stderr, [] as [u8; 0]);
    let stdout = String::from_utf8(out.stdout)?;
    let parsed: Value = serde_json::from_str(stdout.trim())?;
    assert_eq!(parsed, json!([]));

    let check_store = SqliteStore::open(&paths.db_path)?;
    let unrevoked: i64 = check_store.connection().query_row(
        "SELECT COUNT(*) FROM auth_sessions WHERE revoked_at IS NULL",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(unrevoked, 0);
    Ok(())
}

async fn spawn_mcp_only_server(
    workspace_root: String,
) -> Result<(u16, oneshot::Sender<()>, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = Router::new().fallback(any(move |method: Method, body: Bytes| {
        let ws = workspace_root.clone();
        async move { mcp_only_handler(&method, &body, &ws) }
    }));
    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    Ok((port, tx, handle))
}

fn mcp_only_handler(method: &Method, body: &[u8], workspace_root: &str) -> Response {
    if *method == Method::DELETE {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap_or_default();
    }
    let Ok(msg) = serde_json::from_slice::<Value>(body) else {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(axum::body::Body::empty())
            .unwrap_or_default();
    };
    let Some(id) = msg.get("id").cloned() else {
        return Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(axum::body::Body::empty())
            .unwrap_or_default();
    };
    let result = if msg.get("method").and_then(Value::as_str) == Some("initialize") {
        json!({ "protocolVersion": "2025-06-18" })
    } else if msg.pointer("/params/name").and_then(Value::as_str) == Some("t3_project_list") {
        json!({
            "structuredContent": {
                "projects": [{ "id": "project-1", "title": "Test", "workspaceRoot": workspace_root }],
                "nextCursor": null
            }
        })
    } else {
        json!({
            "structuredContent": {
                "projectId": "project-1",
                "currentThreadId": null,
                "threads": [],
                "nextCursor": null,
                "total": 0
            }
        })
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert("mcp-session-id", HeaderValue::from_static("cli-json-test"));
    let payload = serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        .unwrap_or_default();
    let mut resp = Response::new(axum::body::Body::from(payload));
    *resp.headers_mut() = headers;
    resp
}
