//! Edge-case authorization and transitive pairing checks for `POST /api/peer-link/client-pairing`.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use legion_orchestrator::Orchestrator;
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::sqlite::SqliteStore;
use legion_server_runtime::{RuntimeHttpState, runtime_full_router};
use reqwest::header::{COOKIE, ORIGIN};
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct PairingFx {
    url: String,
    db: Arc<Mutex<Connection>>,
    _tmp: tempfile::TempDir,
    srv: JoinHandle<()>,
}

impl Drop for PairingFx {
    fn drop(&mut self) {
        self.srv.abort();
    }
}

async fn start_pairing_fx() -> Res<PairingFx> {
    let tmp = tempfile::tempdir()?;
    let orch = Arc::new(Orchestrator::from_sqlite_store(
        SqliteStore::open_in_memory()?,
    ));
    let db = Arc::clone(orch.sink().db());
    let state = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), tmp.path().join("logs"))
            .with_orchestrator(orch)
            .with_require_auth(false)
            .with_dev_auth_token(Some("dev-secret-tok".to_owned())),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!(
        "http://{}/api/peer-link/client-pairing",
        listener.local_addr()?
    );
    let srv = tokio::spawn(async move {
        let _ = axum::serve(listener, runtime_full_router(state)).await;
    });
    Ok(PairingFx {
        url,
        db,
        _tmp: tmp,
        srv,
    })
}

fn issue_tok(db: &Mutex<Connection>, subject: &str, label: &str, scopes: &[&str]) -> Res<String> {
    let conn = db.lock().map_err(|_| "db poisoned")?;
    let input = IssueSessionInput::default()
        .with_subject(subject)
        .with_label(label)
        .with_scopes(scopes.iter().copied());
    Ok(issue_scoped_session(&conn, input)?.token)
}

#[tokio::test]
async fn peer_pairing_rejects_unauth_cookie_dev_and_non_full_access() -> Res {
    let fx = start_pairing_fx().await?;
    let ro = issue_tok(
        &fx.db,
        "peer-link:read-only",
        "LegionCode · ReadBox",
        &["orchestration:read"],
    )?;
    let auto = issue_tok(
        &fx.db,
        "peer-link:auto",
        "LegionCode · AutoBox",
        &["orchestration:read", "orchestration:operate"],
    )?;
    let outside = issue_tok(
        &fx.db,
        "mcp-client",
        "Outside agent",
        &[
            "orchestration:read",
            "orchestration:operate",
            "settings:write",
            "source-control:write",
        ],
    )?;

    let client = reqwest::Client::new();
    let body = json!({ "scopes": ["orchestration:read"], "clientLabel": "Phone" });

    assert_eq!(
        client.post(&fx.url).json(&body).send().await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(&fx.url)
            .header(COOKIE, "legioncode_dev_auth=dev-secret-tok")
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(&fx.url)
            .bearer_auth("dev-secret-tok")
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(&fx.url)
            .bearer_auth(&ro)
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(&fx.url)
            .bearer_auth(&auto)
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(&fx.url)
            .bearer_auth(&outside)
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[tokio::test]
async fn peer_pairing_rejects_origin_and_accepts_t3_legacy_link_prefix() -> Res {
    let fx = start_pairing_fx().await?;
    let t3_link = issue_tok(
        &fx.db,
        "mcp-client",
        "T3 Code · Legacy Box",
        &[
            "orchestration:read",
            "orchestration:operate",
            "settings:write",
            "source-control:write",
        ],
    )?;

    let client = reqwest::Client::new();
    let body = json!({ "scopes": ["orchestration:read"], "clientLabel": "Phone" });
    assert_eq!(
        client
            .post(&fx.url)
            .header(ORIGIN, "https://evil.example")
            .bearer_auth(&t3_link)
            .json(&body)
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );

    let resp = client
        .post(&fx.url)
        .bearer_auth(&t3_link)
        .json(&json!({ "scopes": [], "clientLabel": "Ignored", "link": { "access": "approval-required" } }))
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json_body: Value = resp.json().await?;
    let cred = json_body["credential"].as_str().ok_or("missing cred")?;
    let (label, scopes_raw): (Option<String>, String) =
        fx.db.lock().map_err(|_| "db poisoned")?.query_row(
            "SELECT label, scopes FROM auth_pairing_links WHERE credential = ?1",
            [cred],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    assert_eq!(label.as_deref(), Some("Link · via Legacy Box"));
    assert!(!scopes_raw.contains("access:write"));
    Ok(())
}
