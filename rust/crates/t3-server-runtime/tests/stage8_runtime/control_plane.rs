//! Stage 8 tests for `/api/auth/*`, `BUG-04` scope enforcement, `/api/scm/*`, `/api/peers*`, and `/api/providers/probe`.

use std::sync::{Arc, Mutex};

use reqwest::StatusCode;
use serde_json::{Value, json};
use t3_persistence::auth_connect_devices::connect_device_verification_code;
use t3_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use tempfile::TempDir;

use super::control_plane_srv::{
    init_sample_git_repo, spawn_main_http_server, spawn_mock_upstream_forge_and_peer,
};
use super::{TestResult, now_unix_ms};

pub async fn run_control_plane_tests() -> TestResult {
    let dir = TempDir::new()?;
    let main_srv = spawn_main_http_server(&dir).await?;
    let (mock_url, mock_srv) =
        spawn_mock_upstream_forge_and_peer("pair-peer-123".to_owned()).await?;
    let client = reqwest::Client::new();

    let admin = verify_auth_admin_and_bug04(&client, &main_srv.base_url, &main_srv.db).await?;
    verify_connect_devices(&client, &main_srv.base_url, &main_srv.db, &admin).await?;
    verify_scm_and_diff(&client, &main_srv.base_url, &mock_url, &admin, &dir).await?;
    verify_peers_and_probes(&client, &main_srv.base_url, &mock_url, &admin).await?;

    main_srv.handle.abort();
    mock_srv.abort();
    Ok(())
}

async fn verify_auth_admin_and_bug04(
    client: &reqwest::Client,
    base: &str,
    db: &Arc<Mutex<rusqlite::Connection>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let pair_code = {
        let conn = db.lock().map_err(|_| "db lock")?;
        issue_pairing_token_with_conn(&conn, &IssuePairingTokenInput::default(), now_unix_ms())?
            .credential
    };
    let pair_resp: Value = client
        .post(format!("{base}/api/auth/pair"))
        .json(&json!({ "token": pair_code, "clientLabel": "admin" }))
        .send()
        .await?
        .json()
        .await?;
    let admin = pair_resp["accessToken"]
        .as_str()
        .ok_or("admin token")?
        .to_owned();

    let ro_resp: Value = client
        .post(format!("{base}/api/auth/sessions"))
        .bearer_auth(&admin)
        .json(&json!({ "scopes": ["orchestration:read", "access:read", "relay:read"], "label": "ro" }))
        .send()
        .await?
        .json()
        .await?;
    let ro_tok = ro_resp["token"].as_str().ok_or("ro token")?.to_owned();
    let ro_id = ro_resp["sessionId"]
        .as_str()
        .ok_or("ro sessionId")?
        .to_owned();

    for path in [
        "/api/auth/pairing",
        "/api/auth/sessions",
        "/api/peers",
        "/api/providers/probe",
        "/api/preview/action",
    ] {
        let r = client
            .post(format!("{base}{path}"))
            .bearer_auth(&ro_tok)
            .json(&json!({}))
            .send()
            .await?;
        assert_eq!(
            r.status(),
            StatusCode::FORBIDDEN,
            "expected 403 for read-only token on {path}"
        );
    }
    let ro_pr_create = client
        .post(format!("{base}/api/scm/pr"))
        .bearer_auth(&ro_tok)
        .json(&json!({ "action": "create", "repo": "acme/demo" }))
        .send()
        .await?;
    assert_eq!(ro_pr_create.status(), StatusCode::FORBIDDEN);

    let new_pair: Value = client
        .post(format!("{base}/api/auth/pairing"))
        .bearer_auth(&admin)
        .json(&json!({ "label": "ephemeral", "ttl": "10m" }))
        .send()
        .await?
        .json()
        .await?;
    let pid = new_pair["id"].as_str().ok_or("pairing id")?;
    assert_eq!(
        client
            .delete(format!("{base}/api/auth/pairing/{pid}"))
            .bearer_auth(&admin)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .delete(format!("{base}/api/auth/sessions/{ro_id}"))
            .bearer_auth(&admin)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .get(format!("{base}/api/auth/sessions"))
            .bearer_auth(&ro_tok)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(admin)
}

async fn verify_connect_devices(
    client: &reqwest::Client,
    base: &str,
    db: &Arc<Mutex<rusqlite::Connection>>,
    admin: &str,
) -> TestResult {
    let thumb = "AbCdEf123456";
    let code = connect_device_verification_code(thumb);
    {
        let conn = db.lock().map_err(|_| "db lock")?;
        conn.execute(
            "INSERT INTO auth_pairing_links (id, credential, method, scopes, subject, label, created_at, expires_at, requires_connect_approval, connect_denied, proof_key_thumbprint) VALUES ('cd-1', 'c1', 't3-connect', '[\"orchestration:read\"]', 'connect', 'Phone', '2026-11-20T00:00:00Z', '2099-01-01T00:00:00Z', 1, 0, ?1)",
            [thumb],
        )?;
    }
    let devices: Value = client
        .get(format!("{base}/api/auth/connect-devices"))
        .bearer_auth(admin)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(devices["waiting"][0]["verificationCode"], code);
    assert_eq!(
        client
            .post(format!("{base}/api/auth/connect-devices/approve"))
            .bearer_auth(admin)
            .json(&json!({ "code": code }))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{base}/api/auth/connect-devices/revoke"))
            .bearer_auth(admin)
            .json(&json!({ "code": code }))
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

async fn verify_scm_and_diff(
    client: &reqwest::Client,
    base: &str,
    mock_url: &str,
    admin: &str,
    dir: &TempDir,
) -> TestResult {
    let pr_fetch: Value = client
        .post(format!("{base}/api/scm/pr"))
        .bearer_auth(admin)
        .json(&json!({ "action": "fetch", "apiBaseUrl": mock_url, "token": "ghp-1", "repo": "acme/demo", "number": 7 }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(pr_fetch["pullRequest"]["title"], "Existing PR");
    let pr_create = client
        .post(format!("{base}/api/scm/pr"))
        .bearer_auth(admin)
        .json(&json!({ "action": "create", "apiBaseUrl": mock_url, "token": "ghp-1", "repo": "acme/demo", "title": "New", "head": "f", "base": "main" }))
        .send()
        .await?;
    assert_eq!(pr_create.status(), StatusCode::CREATED);
    let lin: Value = client
        .post(format!("{base}/api/scm/pr"))
        .bearer_auth(admin)
        .json(&json!({ "action": "linear", "linearEndpoint": format!("{mock_url}/graphql"), "token": "lin_api_key", "issue": "ENG-42" }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(lin["issue"]["identifier"], "ENG-42");

    let repo_dir = dir.path().join("git_repo");
    std::fs::create_dir_all(&repo_dir)?;
    init_sample_git_repo(&repo_dir)?;
    let diff: Value = client
        .post(format!("{base}/api/scm/diff"))
        .bearer_auth(admin)
        .json(&json!({ "repoDir": repo_dir.to_string_lossy() }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(diff["filesChanged"], 1);
    assert_eq!(diff["remote"]["fullName"], "acme/demo");
    Ok(())
}

async fn verify_peers_and_probes(
    client: &reqwest::Client,
    base: &str,
    mock_url: &str,
    admin: &str,
) -> TestResult {
    let linked = client
        .post(format!("{base}/api/peers"))
        .bearer_auth(admin)
        .json(&json!({ "url": mock_url, "pairingCode": "pair-peer-123" }))
        .send()
        .await?;
    assert_eq!(linked.status(), StatusCode::CREATED);
    let listed: Value = client
        .get(format!("{base}/api/peers"))
        .bearer_auth(admin)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(listed["peers"][0]["status"], "reachable");
    assert_eq!(
        client
            .delete(format!("{base}/api/peers/env-peer-1"))
            .bearer_auth(admin)
            .send()
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .delete(format!("{base}/api/peers/env-peer-1"))
            .bearer_auth(admin)
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );

    let cli_probe: Value = client
        .post(format!("{base}/api/providers/probe"))
        .bearer_auth(admin)
        .json(&json!({ "kind": "cli", "executable": "git", "args": ["--version"] }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(cli_probe["status"], "ready");
    let grok_probe: Value = client
        .post(format!("{base}/api/providers/probe"))
        .bearer_auth(admin)
        .json(&json!({ "kind": "grokHttp", "endpointUrl": mock_url, "apiKey": "k" }))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(grok_probe["status"], "ready");
    let speech_probe = client
        .post(format!("{base}/api/providers/probe"))
        .bearer_auth(admin)
        .json(&json!({ "kind": "speech" }))
        .send()
        .await?;
    assert_eq!(speech_probe.status(), StatusCode::OK);
    Ok(())
}
