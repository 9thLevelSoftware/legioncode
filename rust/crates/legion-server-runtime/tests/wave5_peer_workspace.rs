//! Wave 5 peer thread relay and transitive pairing integration tests (`#39406`, `#40036`).

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, PlatformArch, PlatformInfo, PlatformOs,
};
use legion_contracts::ids::{EnvironmentId, ThreadId, TurnItemId};
use legion_contracts::orchestration::McpClientAccess;
use legion_contracts::peer_links::PeerLinkThreadHistoryInput;
use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::sqlite::SqliteStore;
use legion_remote::peer::links::{InsertPeerLinkInput, PeerLinksService};
use legion_server_runtime::{
    RuntimeHttpState, relay_peer_thread_history, relay_peer_thread_item, relay_peer_thread_stream,
    runtime_full_router,
};
use reqwest::header::ORIGIN;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn spawn_state_server(
    state: Arc<RuntimeHttpState>,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let router = runtime_full_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), handle))
}

async fn setup_peer_box_b(
    logs_b: &TempDir,
) -> Result<(String, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let store_b = SqliteStore::open_in_memory()?;
    {
        let conn = store_b.connection();
        conn.execute(
            "INSERT INTO projection_projects (project_id, title, workspace_root, scripts_json, created_at, updated_at)
             VALUES ('p1', 'Proj 1', '/tmp/p1', '[]', '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z')",
            [],
        )?;
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads (thread_id, project_id, title, default_provider, runtime_mode, interaction_mode, payload_json, created_at, updated_at)
             VALUES ('thr-1', 'p1', 'Thread One', 'codex', 'full-access', 'default', '{\"modelSelection\":{\"model\":\"gpt-5\"}}', '2026-11-20T00:00:00Z', '2026-11-20T00:01:00Z')",
            [],
        )?;
        conn.execute(
            "INSERT INTO orchestration_v2_projection_messages (message_id, thread_id, run_id, role, streaming, created_at, updated_at, payload_json)
             VALUES ('msg-1', 'thr-1', 'run-1', 'assistant', 0, '2026-11-20T00:01:00Z', '2026-11-20T00:01:00Z', '{\"text\":\"hello peer\"}')",
            [],
        )?;
        let _ = issue_pairing_token_with_conn(
            conn,
            &IssuePairingTokenInput {
                scopes: vec!["*".to_owned()],
                ttl_ms: 600_000,
                label: Some("peer-link".to_owned()),
            },
            1_700_000_000_000,
        )?;
    }
    let db_b = Arc::new(Mutex::new(store_b.into_connection()));
    let state_b = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db_b), logs_b.path())
            .with_require_auth(false)
            .with_linked_environments_enabled(true),
    );
    spawn_state_server(state_b).await
}

async fn verify_peer_relay_client(url_b: &str, bearer_token: &str) -> TestResult {
    let logs_a = TempDir::new()?;
    let store_a = SqliteStore::open_in_memory()?;
    let db_a = Arc::new(Mutex::new(store_a.into_connection()));
    let state_a = Arc::new(RuntimeHttpState::new(Arc::clone(&db_a), logs_a.path()));
    {
        let conn = db_a.lock().map_err(|_| "db lock")?;
        PeerLinksService::new(state_a.secrets_store()?).insert(
            &conn,
            InsertPeerLinkInput {
                environment_id: EnvironmentId::new("env-b")?,
                label: "Box B".to_owned(),
                urls: vec![url_b.to_owned()],
                access: McpClientAccess::FullAccess,
                access_token: bearer_token.to_owned(),
                linked_at: "2026-11-20T00:00:00Z".to_owned(),
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
            },
        )?;
    }

    let mut desc = ExecutionEnvironmentDescriptor::builder(
        EnvironmentId::new("env-b")?,
        "Box B",
        PlatformInfo::new(PlatformOs::Linux, PlatformArch::X64),
        "0.1.10",
    )
    .build();
    desc.capabilities.linked_thread_read = Some(true);
    let env_b = EnvironmentId::new("env-b")?;
    let thr_1 = ThreadId::new("thr-1")?;
    let item_1 = TurnItemId::new("msg-1")?;

    let stream = relay_peer_thread_stream(&state_a, &env_b, &thr_1, &desc)
        .await
        .map_err(|e| e.message)?;
    assert_ne!(stream, [] as [Value; 0]);
    assert_eq!(stream[0]["thread"]["title"], "Thread One");

    let history = relay_peer_thread_history(
        &state_a,
        &PeerLinkThreadHistoryInput {
            environment_id: env_b.clone(),
            thread_id: thr_1.clone(),
            cursor: "0".to_owned(),
        },
        &desc,
    )
    .await
    .map_err(|e| e.message)?;
    assert_eq!(history["items"][0]["itemId"], "msg-1");

    let item = relay_peer_thread_item(&state_a, &env_b, &thr_1, &item_1, &desc)
        .await
        .map_err(|e| e.message)?;
    assert_eq!(item["item"]["payload"]["text"], "hello peer");
    Ok(())
}

#[tokio::test]
async fn peer_threads_relay_and_transitive_pairing_roundtrip() -> TestResult {
    let logs_b = TempDir::new()?;
    let (url_b, _hb) = setup_peer_box_b(&logs_b).await?;
    let client = reqwest::Client::new();

    let origin_refused = client
        .get(format!("{url_b}/api/links/thread/stream?thread=thr-1"))
        .header(ORIGIN, "https://evil.example")
        .send()
        .await?;
    assert_eq!(origin_refused.status(), StatusCode::FORBIDDEN);

    let list_threads: Value = client
        .get(format!("{url_b}/api/peer/threads"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(list_threads["threads"][0]["threadId"], "thr-1");

    let missing_resp = client
        .get(format!(
            "{url_b}/api/links/thread/item?thread=missing&item=x"
        ))
        .send()
        .await?;
    assert_eq!(missing_resp.status(), StatusCode::NOT_FOUND);

    let pair_resp: Value = client
        .post(format!("{url_b}/api/links/client-pairing"))
        .json(&json!({
            "scopes": ["orchestration:read", "access:write"],
            "clientLabel": "Link · via Phone"
        }))
        .send()
        .await?
        .json()
        .await?;
    let pairing_code = pair_resp["credential"]
        .as_str()
        .ok_or("missing transitive pairing credential")?;
    let exchanged: Value = client
        .post(format!("{url_b}/api/auth/pair"))
        .json(&json!({ "token": pairing_code, "clientLabel": "Box A" }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(exchanged["scopes"], json!(["orchestration:read"]));
    let bearer_token = exchanged["accessToken"]
        .as_str()
        .ok_or("missing exchanged accessToken")?;

    verify_peer_relay_client(&url_b, bearer_token).await
}
