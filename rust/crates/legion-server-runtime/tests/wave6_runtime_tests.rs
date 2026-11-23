//! Wave 6 `legion-server-runtime` HTTP, CORS, webhook, and asset disposition tests.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_orchestrator::Orchestrator;
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::sqlite::SqliteStore;
use legion_remote::peer::links::{InsertPeerLinkInput, PeerLinksService};
use legion_server_runtime::{RuntimeHttpState, format_redacted_access_log, runtime_full_router};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::WorkflowsLocation;
use legion_workflows::store::WorkflowStore;
use legion_workflows::traits::{UnavailableAgentDriver, UnavailableForgeDriver};
use reqwest::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_DISPOSITION, ORIGIN, RANGE, VARY,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct HttpFx {
    base: String,
    bearer: String,
    state: Arc<RuntimeHttpState>,
    tmp: tempfile::TempDir,
    srv: tokio::task::JoinHandle<()>,
}

impl Drop for HttpFx {
    fn drop(&mut self) {
        self.srv.abort();
    }
}

async fn start_fx() -> Res<HttpFx> {
    let tmp = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let orch = Arc::new(Orchestrator::from_sqlite_store(store));
    let db = Arc::clone(orch.sink().db());
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let cred = issue_pairing_token_with_conn(
        &*db.lock().map_err(|_| "db poisoned")?,
        &IssuePairingTokenInput {
            scopes: vec!["*".to_owned()],
            ttl_ms: 600_000,
            label: Some("admin".to_owned()),
        },
        now_ms,
    )?
    .credential;
    let state =
        Arc::new(RuntimeHttpState::new(db, tmp.path().join("logs")).with_orchestrator(orch));
    let router = runtime_full_router(Arc::clone(&state));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let srv = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let base = format!("http://{addr}");
    let tok: Value = reqwest::Client::new()
        .post(format!("{base}/oauth/token"))
        .json(&json!({ "subject_token": cred }))
        .send()
        .await?
        .json()
        .await?;
    let bearer = tok["access_token"]
        .as_str()
        .ok_or("missing access_token")?
        .to_owned();
    Ok(HttpFx {
        base,
        bearer,
        state,
        tmp,
        srv,
    })
}

#[tokio::test]
async fn mcp_oauth_revoke_metadata_and_bearer_only_cleanup() -> Res {
    let fx = start_fx().await?;
    let client = reqwest::Client::new();

    let meta: Value = client
        .get(format!(
            "{}/.well-known/oauth-authorization-server",
            fx.base
        ))
        .send()
        .await?
        .json()
        .await?;
    assert!(
        meta["revocation_endpoint"]
            .as_str()
            .is_some_and(|u| u.ends_with("/oauth/mcp/revoke"))
    );

    let (oauth_id, oauth_tok, admin_id, admin_tok) = {
        let conn = fx.state.db().lock().map_err(|_| "db poisoned")?;
        let s1 = issue_scoped_session(
            &conn,
            IssueSessionInput {
                scopes: vec!["orchestration:read".to_owned()],
                ttl_ms: Some(60_000),
                label: Some("mcp".to_owned()),
                subject: Some("mcp-oauth".to_owned()),
            },
        )?;
        let s2 = issue_scoped_session(
            &conn,
            IssueSessionInput {
                scopes: vec!["*".to_owned()],
                ttl_ms: Some(60_000),
                label: Some("browser".to_owned()),
                subject: Some("paired-admin".to_owned()),
            },
        )?;
        (s1.session_id, s1.token, s2.session_id, s2.token)
    };

    let r1 = client
        .post(format!("{}/oauth/mcp/revoke", fx.base))
        .form(&[("token", oauth_tok.as_str())])
        .send()
        .await?;
    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(
        r1.headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let r2 = client
        .post(format!("{}/oauth/mcp/revoke", fx.base))
        .form(&[("token", admin_tok.as_str())])
        .send()
        .await?;
    assert_eq!(r2.status(), StatusCode::OK);

    let conn = fx.state.db().lock().map_err(|_| "db poisoned")?;
    let oauth_revoked: Option<String> = conn.query_row(
        "SELECT revoked_at FROM auth_sessions WHERE session_id = ?1",
        [&oauth_id],
        |row| row.get(0),
    )?;
    let admin_revoked: Option<String> = conn.query_row(
        "SELECT revoked_at FROM auth_sessions WHERE session_id = ?1",
        [&admin_id],
        |row| row.get(0),
    )?;
    assert!(oauth_revoked.is_some());
    assert!(admin_revoked.is_none());
    Ok(())
}

#[tokio::test]
async fn peer_client_pairing_and_delete_ended_there() -> Res {
    let fx = start_fx().await?;
    let client = reqwest::Client::new();
    let pairing_url = format!("{}/api/peer-link/client-pairing", fx.base);
    let body = json!({ "scopes": ["orchestration:read"], "clientLabel": "Peer B" });
    let admin = client
        .post(&pairing_url)
        .bearer_auth(&fx.bearer)
        .json(&body)
        .send()
        .await?;
    assert_eq!(admin.status(), StatusCode::FORBIDDEN);
    let peer_tok = issue_scoped_session(
        &*fx.state.db().lock().map_err(|_| "db")?,
        IssueSessionInput::default()
            .with_subject("peer-link:full-access")
            .with_label("LegionCode · Laptop")
            .with_scopes(["*"]),
    )?
    .token;
    let paired = client
        .post(&pairing_url)
        .bearer_auth(&peer_tok)
        .json(&body)
        .send()
        .await?;
    assert_eq!(paired.status(), StatusCode::OK);
    let paired_body: Value = paired.json().await?;
    let cred = paired_body["credential"].as_str().ok_or("missing cred")?;
    assert!(
        !cred.is_empty()
            && paired_body["expiresAt"]
                .as_str()
                .is_some_and(|s| s.ends_with('Z'))
    );
    let via: Option<String> = fx.state.db().lock().map_err(|_| "db")?.query_row(
        "SELECT label FROM auth_pairing_links WHERE credential = ?1",
        [cred],
        |r| r.get(0),
    )?;
    assert_eq!(via.as_deref(), Some("Peer B · via Laptop"));

    let svc = PeerLinksService::new(legion_remote::secrets::FileSecretStore::new(
        &fx.state.secrets_dir,
    )?);
    {
        let conn = fx.state.db().lock().map_err(|_| "db poisoned")?;
        svc.insert(
            &conn,
            InsertPeerLinkInput {
                environment_id: EnvironmentId::new("env-remote-down")?,
                label: "Down Box".to_owned(),
                urls: vec!["http://127.0.0.1:9".to_owned()],
                access: McpClientAccess::FullAccess,
                access_token: "tok-down-1".to_owned(),
                linked_at: "2026-11-20T00:00:00Z".to_owned(),
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
            },
        )?;
    }

    let del = client
        .delete(format!("{}/api/peers/env-remote-down", fx.base))
        .bearer_auth(&fx.bearer)
        .send()
        .await?;
    assert_eq!(del.status(), StatusCode::OK);
    let del_body: Value = del.json().await?;
    assert_eq!(del_body["removed"], true);
    assert_eq!(del_body["endedThere"], false);
    Ok(())
}

#[tokio::test]
async fn webhook_dispatch_and_access_log_redaction() -> Res {
    let fx = start_fx().await?;
    let db = Arc::clone(fx.state.db());
    let wf_store = {
        let conn = db.lock().map_err(|_| "db poisoned")?;
        legion_workflows::schema::ensure_workflow_and_feedback_schema(&conn)?;
        drop(conn);
        WorkflowStore::new(Arc::clone(&db))?
    };
    let mgr = WorkflowDefinitionsManager::open(
        wf_store,
        Arc::new(UnavailableAgentDriver),
        Arc::new(UnavailableForgeDriver),
        WorkflowsLocation {
            directory: fx.tmp.path().join("workflows"),
            single: None,
        },
        true,
    )?;
    mgr.save(
        "hook.ts",
        r#"
Legion.workflow({
  name: "wh_hook",
  version: 1,
  triggers: [Legion.webhook({ name: "gh" })],
  run: () => Effect.gen(function* () { return { ok: true }; })
});
"#,
    )?;
    let hooks = mgr.engine().hooks()?;
    let hook = hooks.first().ok_or("missing registered hook")?;
    let parts: Vec<&str> = hook
        .path
        .trim_start_matches("/api/hooks/")
        .split('/')
        .collect();
    assert_eq!(parts.len(), 2);

    let orch = fx.state.orchestrator.clone().ok_or("missing orch")?;
    let state2 = Arc::new(
        RuntimeHttpState::new(db, fx.tmp.path().join("logs2"))
            .with_orchestrator(orch)
            .with_workflow_manager(mgr),
    );
    let router = runtime_full_router(Arc::clone(&state2));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base2 = format!("http://{}", listener.local_addr()?);
    let srv2 = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let client = reqwest::Client::new();

    let hook_res = client
        .post(format!("{base2}/webhooks/{}/{}", parts[0], parts[1]))
        .body("{}")
        .send()
        .await?;
    assert_eq!(hook_res.status(), StatusCode::ACCEPTED);
    let hook_json: Value = hook_res.json().await?;
    assert!(hook_json["runId"].as_str().is_some());

    let log = format_redacted_access_log(
        &Method::POST,
        "/webhooks/wf-wh_hook.gh/secret123?foo=bar",
        StatusCode::ACCEPTED,
    );
    assert!(log.contains("/webhooks/wf-wh_hook.gh/REDACTED?foo=bar"));
    assert!(!log.contains("secret123"));
    srv2.abort();
    Ok(())
}

#[tokio::test]
async fn attachment_disposition_and_proxy_cors() -> Res {
    let fx = start_fx().await?;
    let orch = fx.state.orchestrator.clone().ok_or("missing orch")?;
    let state2 = Arc::new(
        RuntimeHttpState::new(Arc::clone(fx.state.db()), fx.tmp.path().join("logs2"))
            .with_orchestrator(orch)
            .with_dev_url(Some("http://127.0.0.1:5733".to_owned())),
    );
    let router = runtime_full_router(Arc::clone(&state2));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base2 = format!("http://{}", listener.local_addr()?);
    let srv2 = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let client = reqwest::Client::new();

    let asset_path = fx.tmp.path().join("report.txt");
    std::fs::write(&asset_path, "hello-attachment-body")?;
    let signed = state2.create_signed_asset_url_with_disposition(
        &asset_path,
        Some("report.txt"),
        Some(60_000),
        true,
    )?;
    let full_get = client
        .get(format!("{base2}{}", signed.relative_url))
        .send()
        .await?;
    assert_eq!(full_get.status(), StatusCode::OK);
    assert_eq!(
        full_get
            .headers()
            .get(CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok()),
        Some("attachment; filename=\"report.txt\"")
    );
    let range_get = client
        .get(format!("{base2}{}", signed.relative_url))
        .header(RANGE, "bytes=0-4")
        .send()
        .await?;
    assert_eq!(range_get.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        range_get
            .headers()
            .get(CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok()),
        Some("attachment; filename=\"report.txt\"")
    );

    let originless_cors = client
        .get(format!("{base2}/api/auth/session"))
        .bearer_auth(&fx.bearer)
        .send()
        .await?;
    assert_eq!(originless_cors.status(), StatusCode::OK);
    assert_eq!(
        originless_cors
            .headers()
            .get(ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
    assert!(
        originless_cors
            .headers()
            .get(VARY)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("Origin"))
    );
    let disallowed_cors = client
        .get(format!("{base2}/api/auth/session"))
        .header(ORIGIN, "https://evil.example")
        .bearer_auth(&fx.bearer)
        .send()
        .await?;
    assert_eq!(
        disallowed_cors.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
        None
    );
    srv2.abort();
    Ok(())
}
