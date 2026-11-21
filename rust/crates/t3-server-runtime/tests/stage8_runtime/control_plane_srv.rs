//! Mock upstream forge/peer/main HTTP servers and sample git repository for Stage 8 tests.

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode as AxumStatus};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};
use t3_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use t3_contracts::ids::EnvironmentId;
use t3_persistence::sqlite::SqliteStore;
use t3_server_runtime::{RuntimeHttpState, runtime_full_router};
use tempfile::TempDir;
use tokio::net::TcpListener;

use super::TestResult;

pub struct MainHttpServer {
    pub base_url: String,
    pub db: Arc<Mutex<rusqlite::Connection>>,
    pub handle: tokio::task::JoinHandle<()>,
}

pub async fn spawn_main_http_server(
    dir: &TempDir,
) -> Result<MainHttpServer, Box<dyn std::error::Error>> {
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let state = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), dir.path().join("logs"))
            .with_secrets_dir(dir.path().join("secrets"))
            .with_require_auth(true),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, runtime_full_router(state)).await;
    });
    Ok(MainHttpServer {
        base_url: format!("http://{addr}"),
        db,
        handle,
    })
}

pub async fn spawn_mock_upstream_forge_and_peer(
    peer_pair_token: String,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let app = mock_upstream_router(Arc::new(peer_pair_token));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://{addr}"), handle))
}

fn mock_upstream_router(peer_tok: Arc<String>) -> Router {
    Router::new()
        .route(
            "/repos/acme/demo/pulls/7",
            get(|| async {
                Json(json!({
                    "number": 7,
                    "title": "Existing PR",
                    "html_url": "https://github.com/acme/demo/pull/7",
                    "state": "open",
                    "draft": false,
                    "head": { "ref": "feat/live", "sha": "abc123" },
                    "base": { "ref": "main" },
                    "mergeable_state": "clean"
                }))
            }),
        )
        .route(
            "/repos/acme/demo/pulls",
            post(|Json(body): Json<Value>| async move {
                (
                    AxumStatus::CREATED,
                    Json(json!({
                        "number": 8,
                        "title": body["title"],
                        "html_url": "https://github.com/acme/demo/pull/8",
                        "state": "open",
                        "draft": body["draft"],
                        "head": { "ref": body["head"], "sha": "def456" },
                        "base": { "ref": body["base"] }
                    })),
                )
            }),
        )
        .route(
            "/repos/acme/demo/pulls/7/merge",
            put(|| async {
                Json(json!({
                    "merged": true,
                    "sha": "mrg789",
                    "message": "Pull Request successfully merged"
                }))
            }),
        )
        .route("/graphql", post(handle_graphql_mock))
        .route(
            "/v1/models",
            get(|| async { Json(json!({ "data": [{ "id": "grok-4" }] })) }),
        )
        .route(
            "/.well-known/t3/environment",
            get(|| async {
                let Ok(env_id) = EnvironmentId::new("env-peer-1") else {
                    return AxumStatus::INTERNAL_SERVER_ERROR.into_response();
                };
                let mut desc = ExecutionEnvironmentDescriptor::builder(
                    env_id,
                    "Remote Workstation",
                    PlatformInfo::new(PlatformOs::Darwin, PlatformArch::Arm64),
                    "0.1.10",
                )
                .build();
                desc.capabilities.mcp_mode_limit_header = Some(true);
                desc.capabilities.link_fence = Some(true);
                Json(desc).into_response()
            }),
        )
        .route(
            "/api/auth/pair",
            post(move |Json(body): Json<Value>| {
                let expected = Arc::clone(&peer_tok);
                async move {
                    if body["token"].as_str() != Some(expected.as_str()) {
                        return (
                            AxumStatus::UNAUTHORIZED,
                            Json(json!({ "error": "bad token" })),
                        )
                            .into_response();
                    }
                    Json(json!({
                        "accessToken": "t3s_peer_session_secret",
                        "expiresIn": 3600
                    }))
                    .into_response()
                }
            }),
        )
}

async fn handle_graphql_mock(headers: HeaderMap) -> axum::response::Response {
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth.starts_with("Bearer ") {
        return (
            AxumStatus::UNAUTHORIZED,
            Json(json!({ "error": "expected raw Linear key" })),
        )
            .into_response();
    }
    Json(json!({
        "data": {
            "issue": {
                "id": "lin-id-1",
                "identifier": "ENG-42",
                "title": "Finish Stage 8",
                "state": { "name": "In Progress" },
                "url": "https://linear.app/acme/issue/ENG-42"
            }
        }
    }))
    .into_response()
}

pub fn init_sample_git_repo(dir: &Path) -> TestResult {
    let run = |args: &[&str]| -> TestResult {
        let out = Command::new("git").args(args).current_dir(dir).output()?;
        if !out.status.success() {
            return Err(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            )
            .into());
        }
        Ok(())
    };
    run(&["init"])?;
    run(&["config", "user.name", "Test"])?;
    run(&["config", "user.email", "test@example.com"])?;
    run(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/demo.git",
    ])?;
    std::fs::write(dir.join("lib.rs"), "pub fn v() -> i32 { 1 }\n")?;
    run(&["add", "."])?;
    run(&["commit", "-m", "init"])?;
    std::fs::write(dir.join("lib.rs"), "pub fn v() -> i32 { 2 }\n")?;
    Ok(())
}
