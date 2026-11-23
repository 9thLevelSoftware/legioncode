//! Parity integration tests covering all `EnvironmentHttpApi` groups, CORS, raw assets,
//! attachment uploads, WebSocket tickets, cookies, and API 404 fallbacks.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use legion_contracts::orchestration::OrchestrationCommand;
use legion_orchestrator::Orchestrator;
use legion_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, IssuePairingTokenInput, issue_pairing_token_with_conn,
};
use legion_persistence::sqlite::SqliteStore;
use legion_server_runtime::{RuntimeHttpState, runtime_full_router};
use reqwest::header::{COOKIE, ORIGIN, RANGE};
use reqwest::{Client, Method, StatusCode, redirect};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;
type Db = Arc<Mutex<rusqlite::Connection>>;

struct Fx {
    base: String,
    client: Client,
    bearer: String,
    state: Arc<RuntimeHttpState>,
    tmp: tempfile::TempDir,
    srv: tokio::task::JoinHandle<()>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.srv.abort();
    }
}

impl Fx {
    async fn start(
        db: Option<Db>,
        static_dir: Option<std::path::PathBuf>,
        port: Option<u16>,
        dev: Option<&str>,
    ) -> Res<Self> {
        let tmp = tempfile::tempdir()?;
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs)?;
        let db = match db {
            Some(d) => d,
            None => Arc::new(Mutex::new(SqliteStore::open_in_memory()?.into_connection())),
        };
        let mut st = RuntimeHttpState::new(db, &logs)
            .with_static_dir(static_dir)
            .with_dev_url(dev.map(str::to_owned));
        if let Some(p) = port {
            st = st.with_port(p);
        }
        let state = Arc::new(st);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let app = runtime_full_router(Arc::clone(&state));
        let srv = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .build()?;
        let cred = mint_cred(&state, "Harness")?;
        let tok: Value = client
            .post(format!("{base}/oauth/token"))
            .json(&json!({ "subject_token": cred }))
            .send()
            .await?
            .json()
            .await?;
        let bearer = tok["access_token"].as_str().ok_or("no tok")?.to_owned();
        Ok(Self {
            base,
            client,
            bearer,
            state,
            tmp,
            srv,
        })
    }

    async fn get(&self, p: &str) -> Res<(StatusCode, Value)> {
        let rq = self
            .client
            .get(format!("{}{p}", self.base))
            .header("x-legioncode-orchestration-protocol", "2");
        let r = rq.bearer_auth(&self.bearer).send().await?;
        Ok((r.status(), r.json().await?))
    }

    async fn get_ck(&self, p: &str, ck: &str) -> Res<Value> {
        Ok(self
            .client
            .get(format!("{}{p}", self.base))
            .header(COOKIE, ck)
            .send()
            .await?
            .json()
            .await?)
    }

    async fn post(&self, p: &str, body: Value) -> Res<(StatusCode, Value)> {
        let r = self
            .client
            .post(format!("{}{p}", self.base))
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await?;
        let s = r.status();
        Ok((
            s,
            if s == StatusCode::NO_CONTENT {
                Value::Null
            } else {
                r.json().await?
            },
        ))
    }

    async fn chk(&self, p: &str, raw: &str, k: &str, want: Value) -> Res {
        let (s, v) = self.post(p, serde_json::from_str(raw)?).await?;
        assert_eq!((s, &v[k]), (StatusCode::OK, &want), "POST {p}");
        Ok(())
    }
}

fn mint_cred(state: &RuntimeHttpState, label: &str) -> Res<String> {
    let conn = state.db().lock().map_err(|_| "db lock")?;
    let scopes = ADMINISTRATIVE_CLIENT_SCOPES.iter().map(|s| (*s).to_owned());
    let input = IssuePairingTokenInput::default()
        .with_scopes(scopes)
        .with_label(label);
    Ok(issue_pairing_token_with_conn(&conn, &input, 4_000_000_000_000)?.credential)
}

async fn assert_cors_and_browser_cookie(fx: &Fx, dev: &str) -> Res<String> {
    let opt = fx
        .client
        .request(Method::OPTIONS, format!("{}/api/auth/session", fx.base))
        .header(ORIGIN, dev)
        .send()
        .await?;
    let h = |k: &str| opt.headers().get(k).and_then(|v| v.to_str().ok());
    assert_eq!(
        (
            opt.status(),
            h("access-control-allow-origin"),
            h("access-control-allow-credentials")
        ),
        (StatusCode::NO_CONTENT, Some(dev), Some("true"))
    );

    let anon: Value = fx
        .client
        .get(format!("{}/api/auth/session", fx.base))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(anon["authenticated"], false);
    assert!(
        anon["auth"]["sessionCookieName"]
            .as_str()
            .unwrap_or("")
            .starts_with("legioncode_session_3773_")
    );

    let cred1 = mint_cred(&fx.state, "Browser")?;
    let login_req = fx
        .client
        .post(format!("{}/api/auth/browser-session", fx.base))
        .header(ORIGIN, "http://localhost:5173");
    let login_resp = login_req
        .json(&json!({ "credential": cred1 }))
        .send()
        .await?;
    assert_eq!(login_resp.status(), StatusCode::OK);
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .ok_or("no set-cookie")?
        .split(';')
        .next()
        .ok_or("bad cookie")?
        .to_owned();
    let bj: Value = login_resp.json().await?;
    assert_eq!(
        (&bj["authenticated"], &bj["sessionMethod"]),
        (&json!(true), &json!("browser-session-cookie"))
    );
    assert_eq!(
        (
            bj["scopes"].as_array().map(Vec::len),
            bj["permissions"].as_array().map(Vec::len)
        ),
        (Some(7), Some(17))
    );
    Ok(cookie)
}

#[tokio::test]
async fn auth_session_cookies_oauth_tickets_revokes_and_cors() -> Res {
    let dev = "https://vite.example.ts.net:5173";
    let mut fx = Fx::start(None, None, Some(3773), Some(dev)).await?;
    let cookie = assert_cors_and_browser_cookie(&fx, dev).await?;

    let p_res: Value = fx
        .client
        .post(format!("{}/api/auth/pairing-token", fx.base))
        .header(COOKIE, &cookie)
        .json(&json!({ "label": "OAuth", "role": "owner" }))
        .send()
        .await?
        .json()
        .await?;
    let c2 = p_res["credential"].as_str().ok_or("no cred")?;
    let form = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange&subject_token={c2}&subject_token_type=urn%3Alegioncode%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap&requested_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token"
    );
    let oa: Value = fx
        .client
        .post(format!("{}/oauth/token", fx.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("dpop", "proof")
        .body(form)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(oa["token_type"], "DPoP");
    fx.bearer = oa["access_token"].as_str().ok_or("no tok")?.to_owned();

    assert_eq!(
        fx.get_ck("/api/auth/session", &cookie).await?["authenticated"],
        true
    );
    let rpc = fx
        .post("/api/auth/websocket-ticket/rpc", json!({}))
        .await?
        .1["ticket"]
        .as_str()
        .ok_or("no ticket")?
        .to_owned();
    assert!(fx.state.consume_ws_ticket(&rpc, Some("media")).is_none());
    assert!(
        fx.state.consume_ws_ticket(&rpc, Some("rpc")).is_some()
            && fx.state.consume_ws_ticket(&rpc, Some("rpc")).is_none()
    );

    assert!(
        fx.post("/api/auth/clients/revoke-others", json!({}))
            .await?
            .1["revokedCount"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );
    assert_eq!(
        fx.get_ck("/api/auth/session", &cookie).await?["authenticated"],
        false
    );

    let lid = fx
        .post("/api/auth/pairing-token", json!({ "label": "Tmp" }))
        .await?
        .1["id"]
        .as_str()
        .ok_or("no id")?
        .to_owned();
    fx.chk(
        "/api/auth/pairing-links/revoke",
        &format!(r#"{{"id":"{lid}"}}"#),
        "revoked",
        json!(true),
    )
    .await?;

    fx.state.db().lock().map_err(|_| "db lock")?.execute(
        "INSERT INTO auth_pairing_links (id, credential, method, scopes, subject, label, created_at, expires_at, requires_connect_approval, connect_denied, proof_key_thumbprint) VALUES ('cd-link-1', 'c-link-1', 'legion-connect', '[\"orchestration:read\"]', 'connect', 'Phone', '2026-11-20T00:00:00Z', '2099-01-01T00:00:00Z', 1, 0, 'ThumbKey123')",
        [],
    )?;
    fx.chk(
        "/api/auth/connect-devices/approve",
        r#"{"pairingLinkId":"cd-link-1"}"#,
        "approved",
        json!(true),
    )
    .await?;
    fx.chk(
        "/api/auth/connect-devices/revoke",
        r#"{"proofKeyThumbprint":"ThumbKey123"}"#,
        "revoked",
        json!(true),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn orchestration_projects_and_pr_diff_parity() -> Res {
    let orch = Orchestrator::from_sqlite_store(SqliteStore::open_in_memory()?);
    let c1: OrchestrationCommand = serde_json::from_value(json!({
        "type": "thread.create", "createdBy": "user", "creationSource": "web", "commandId": "cmd-th-1",
        "threadId": "th-parity-1", "projectId": "proj-parity-1", "title": "Parity Thread",
        "modelSelection": { "instanceId": "codex", "model": "gpt-5" },
        "runtimeMode": "full-access", "interactionMode": "default", "branch": null, "worktreePath": null
    }))?;
    orch.dispatch_command(&c1, "2026-11-20T00:01:00.000Z")
        .await?;
    let c2: OrchestrationCommand = serde_json::from_value(json!({
        "type": "message.dispatch", "createdBy": "user", "creationSource": "web", "commandId": "cmd-msg-1",
        "threadId": "th-parity-1", "messageId": "msg-1", "text": "Hello Parity",
        "attachments": [], "dispatchMode": { "type": "start_immediately" }
    }))?;
    orch.dispatch_command(&c2, "2026-11-20T00:01:01.000Z")
        .await?;

    let fx = Fx::start(Some(Arc::clone(orch.sink().db())), None, None, None).await?;
    let proj_dir = fx.tmp.path().join("repo-alpha");
    std::fs::create_dir_all(&proj_dir)?;

    let p_body = json!({ "type": "project.create", "commandId": "cmd-p-1", "projectId": "proj-parity-1", "title": "Alpha", "workspaceRoot": proj_dir.to_string_lossy() });
    let (_, proj) = fx.post("/api/projects/mutate", p_body).await?;
    assert_eq!(proj["id"], "proj-parity-1");
    assert!(proj.get("repositoryIdentity").is_some());

    let (_, shell) = fx.get("/api/orchestration/shell").await?;
    assert_eq!(
        (&shell["schemaVersion"], &shell["threads"][0]["id"]),
        (&json!(2), &json!("th-parity-1"))
    );
    assert_eq!(
        fx.get("/api/orchestration/shell?resumeFrom=0").await?.1["resumable"],
        true
    );
    for p in ["", "/bounded", "/history", "/transcript"] {
        assert_eq!(
            fx.get(&format!("/api/orchestration/threads/th-parity-1{p}"))
                .await?
                .0,
            StatusCode::OK
        );
    }

    let (nf_s, nf_v) = fx.get("/api/orchestration/threads/missing").await?;
    assert_eq!(
        (nf_s, &nf_v["_tag"]),
        (
            StatusCode::NOT_FOUND,
            &json!("EnvironmentResourceNotFoundError")
        )
    );

    let pr_b = json!({ "projectId": "missing", "repository": "a/b", "number": 1 });
    let (pr_s, pr_v) = fx.post("/api/pull-requests/diff", pr_b).await?;
    assert_eq!(
        (pr_s, &pr_v["_tag"]),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            &json!("PullRequestUnavailableError")
        )
    );
    Ok(())
}

async fn assert_voice_and_connect(fx: &Fx) -> Res {
    let status_cases = [
        (
            "/api/voice/cloud/openai/key",
            r#"{"key":"sk-1"}"#,
            StatusCode::NO_CONTENT,
        ),
        ("/api/voice/models/prepare", "{}", StatusCode::NO_CONTENT),
        (
            "/api/voice/models/download",
            r#"{"modelId":"parakeet-tdt-0.6b-v3"}"#,
            StatusCode::OK,
        ),
        (
            "/api/voice/models/select",
            r#"{"modelId":"parakeet-tdt-0.6b-v3"}"#,
            StatusCode::OK,
        ),
    ];
    for (p, raw, want) in status_cases {
        assert_eq!(
            fx.post(p, serde_json::from_str(raw)?).await?.0,
            want,
            "POST {p}"
        );
    }
    assert_eq!(fx.get("/api/voice/cloud/keys").await?.1["openai"], true);
    assert_eq!(fx.get("/api/voice/status").await?.1["supported"], true);
    assert_eq!(fx.get("/api/connect/link-state").await?.1["linked"], false);
    assert!(
        fx.post("/api/connect/link-proof", json!({ "challenge": "ch-1" }))
            .await?
            .1
            .is_string()
    );
    assert!(
        fx.post(
            "/api/legion-connect/mint-credential",
            json!({ "requestNonce": "n-2" })
        )
        .await?
        .1["credential"]
            .is_string()
    );

    let post_cases = [
        (
            "/api/voice/acceleration",
            r#"{"acceleration":"cpu"}"#,
            "acceleration",
            json!("cpu"),
        ),
        (
            "/api/voice/model-unload-timeout",
            r#"{"timeout":"min_10"}"#,
            "modelUnloadTimeout",
            json!("min_10"),
        ),
        (
            "/api/voice/post-process",
            r#"{"text":"hello   world"}"#,
            "text",
            json!("hello world"),
        ),
        (
            "/api/connect/relay-config",
            r#"{"cloudUserId":"u-1","relayUrl":"https://relay.test","relayIssuer":"https://iss.test"}"#,
            "ok",
            json!(true),
        ),
        (
            "/api/connect/preferences",
            r#"{"publishAgentActivity":true}"#,
            "publishAgentActivity",
            json!(true),
        ),
        (
            "/api/connect/live-activity-preference",
            r#"{"liveActivitiesEnabled":true}"#,
            "ok",
            json!(true),
        ),
        (
            "/api/connect/reachability",
            "{}",
            "status",
            json!("unreachable"),
        ),
        (
            "/api/legion-connect/health",
            r#"{"requestNonce":"n-1"}"#,
            "status",
            json!("online"),
        ),
        ("/api/connect/unlink", "{}", "ok", json!(true)),
    ];
    for (p, raw, key, want) in post_cases {
        fx.chk(p, raw, key, want).await?;
    }
    Ok(())
}

async fn assert_mcp_oauth_pkce(fx: &Fx) -> Res {
    for wk in ["oauth-protected-resource", "oauth-authorization-server"] {
        assert_eq!(
            fx.get(&format!("/.well-known/{wk}")).await?.0,
            StatusCode::OK
        );
    }
    let reg_b = json!({ "client_name": "Agent", "redirect_uris": ["http://127.0.0.1:49999/cb"] });
    let cid = fx.post("/oauth/mcp/register", reg_b).await?.1["client_id"]
        .as_str()
        .ok_or("no cid")?
        .to_owned();
    let verifier = "pkce-verifier-secret-0123456789abcdef";
    let chal = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let auth_req = json!({ "response_type": "code", "client_id": cid, "redirect_uri": "http://127.0.0.1:49999/cb", "code_challenge": chal, "code_challenge_method": "S256" });
    let csrf = fx.post("/oauth/mcp/approval", auth_req.clone()).await?.1["csrfToken"]
        .as_str()
        .ok_or("no csrf")?
        .to_owned();
    let dec = json!({ "authorization": auth_req, "decision": { "_tag": "browser-session", "access": "full-access", "csrfToken": csrf } });
    let redir = fx.post("/oauth/mcp/decision", dec).await?.1["redirectTo"]
        .as_str()
        .ok_or("no redir")?
        .to_owned();
    let code = redir
        .split("code=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .ok_or("no code")?;
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", "http://127.0.0.1:49999/cb"),
        ("client_id", &cid),
        ("code_verifier", verifier),
    ];
    let mcp_tok: Value = fx
        .client
        .post(format!("{}/oauth/mcp/token", fx.base))
        .form(&form)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(
        (&mcp_tok["token_type"], mcp_tok["access_token"].is_string()),
        (&json!("Bearer"), true)
    );
    Ok(())
}

#[tokio::test]
async fn voice_connect_mcp_oauth_webhooks_assets_and_api_404() -> Res {
    let static_tmp = tempfile::tempdir()?;
    std::fs::write(static_tmp.path().join("index.html"), "<html>SPA</html>")?;
    let fx = Fx::start(None, Some(static_tmp.path().to_path_buf()), None, None).await?;

    assert_voice_and_connect(&fx).await?;
    assert_mcp_oauth_pkce(&fx).await?;

    for m in [Method::GET, Method::POST, Method::PUT, Method::PATCH] {
        let wh = fx
            .client
            .request(m, format!("{}/api/hooks/hook-99/tok-99", fx.base))
            .body("{\"ok\":true}")
            .send()
            .await?;
        assert_eq!(wh.status(), StatusCode::ACCEPTED);
        assert!(wh.json::<Value>().await?["deliveryId"].as_str().is_some());
    }

    let fp = fx.tmp.path().join("sample.bin");
    std::fs::write(&fp, b"0123456789")?;
    let asset_url = fx
        .state
        .create_signed_asset_url(&fp, None, None)?
        .relative_url;
    let ranged = fx
        .client
        .get(format!("{}{asset_url}", fx.base))
        .header(RANGE, "bytes=2-5")
        .send()
        .await?;
    assert_eq!(
        (ranged.status(), ranged.text().await?.as_str()),
        (StatusCode::PARTIAL_CONTENT, "2345")
    );

    let up = fx
        .state
        .create_attachment_upload_url("att-1.txt", Some(1024), None)?;
    let dl = fx
        .client
        .post(format!("{}{}", fx.base, up.relative_url))
        .body("hello-attachment")
        .send()
        .await?
        .json::<Value>()
        .await?["relativeUrl"]
        .as_str()
        .ok_or("no url")?
        .to_owned();
    assert_eq!(
        fx.client
            .get(format!("{}{dl}", fx.base))
            .send()
            .await?
            .text()
            .await?,
        "hello-attachment"
    );

    let (nf_s, nf_v) = fx.get("/api/no-such-endpoint").await?;
    assert_eq!(
        (nf_s, &nf_v["_tag"]),
        (
            StatusCode::NOT_FOUND,
            &json!("EnvironmentResourceNotFoundError")
        )
    );
    Ok(())
}
