//! Live HTTP integration tests: pairing auth, `BUG-03`, projects, and OTLP traces.

use std::path::Path;
use std::sync::{Arc, Mutex};

use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::sqlite::SqliteStore;
use legion_remote::trace::{DEFAULT_TRACE_MAX_FILES, summarize_trace_files};
use legion_server_runtime::{RuntimeHttpState, runtime_full_router};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn spawn_test_server(
    static_dir: Option<std::path::PathBuf>,
) -> Result<
    (
        String,
        Arc<Mutex<rusqlite::Connection>>,
        std::path::PathBuf,
        tokio::task::JoinHandle<()>,
    ),
    Box<dyn std::error::Error>,
> {
    let logs_dir = TempDir::new()?.keep();
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let state = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), &logs_dir)
            .with_static_dir(static_dir)
            .with_require_auth(true),
    );
    let router = runtime_full_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), db, logs_dir, handle))
}

async fn verify_pairing_and_bug03(
    client: &reqwest::Client,
    base_url: &str,
    db: &Arc<Mutex<rusqlite::Connection>>,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(i64::MAX);
    let (pair_code_1, pair_code_2) = {
        let conn = db.lock().map_err(|_| "db mutex")?;
        let p1 = issue_pairing_token_with_conn(
            &conn,
            &IssuePairingTokenInput {
                scopes: vec!["client:read".to_owned(), "client:write".to_owned()],
                ttl_ms: 600_000,
                label: Some("test-1".to_owned()),
            },
            now_ms,
        )?;
        let p2 = issue_pairing_token_with_conn(
            &conn,
            &IssuePairingTokenInput {
                scopes: vec!["client:read".to_owned()],
                ttl_ms: 600_000,
                label: Some("test-2".to_owned()),
            },
            now_ms,
        )?;
        (p1.credential, p2.credential)
    };

    let no_static = client.get(format!("{base_url}/")).send().await?;
    assert_eq!(no_static.status(), StatusCode::SERVICE_UNAVAILABLE);

    let pair_resp = client
        .post(format!("{base_url}/api/auth/pair"))
        .json(&json!({ "token": pair_code_1, "clientLabel": "test-browser" }))
        .send()
        .await?;
    assert_eq!(pair_resp.status(), StatusCode::OK);
    let cookie_hdr = pair_resp
        .headers()
        .get(SET_COOKIE)
        .ok_or("missing Set-Cookie header")?
        .to_str()?
        .to_owned();
    assert!(cookie_hdr.contains("legioncode_session="));
    assert!(cookie_hdr.contains("HttpOnly"));
    let cookie_kv = cookie_hdr
        .split(';')
        .next()
        .ok_or("invalid cookie")?
        .to_owned();
    let pair_body: Value = pair_resp.json().await?;
    let bearer = pair_body["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned();

    let reuse_resp = client
        .post(format!("{base_url}/api/auth/pair"))
        .json(&json!({ "token": pair_code_1 }))
        .send()
        .await?;
    assert_eq!(reuse_resp.status(), StatusCode::UNAUTHORIZED);

    let oauth_resp = client
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("subject_token", pair_code_2.as_str()),
        ])
        .send()
        .await?;
    assert_eq!(oauth_resp.status(), StatusCode::OK);
    let oauth_json: Value = oauth_resp.json().await?;
    assert_eq!(oauth_json["token_type"], "Bearer");
    assert!(oauth_json["access_token"].as_str().is_some());

    // BUG-03: Invalid Authorization header MUST NOT fall back to a valid cookie!
    let bug03_resp = client
        .get(format!("{base_url}/api/projects"))
        .header(COOKIE, &cookie_kv)
        .header(AUTHORIZATION, "Bearer completely-invalid-token")
        .send()
        .await?;
    assert_eq!(bug03_resp.status(), StatusCode::UNAUTHORIZED);
    Ok((bearer, cookie_kv))
}

async fn verify_project_mutations(
    client: &reqwest::Client,
    base_url: &str,
    bearer: &str,
    cookie_kv: &str,
) -> TestResult {
    let ws_dir = TempDir::new()?;
    let ws_path = ws_dir.path().to_string_lossy().into_owned();
    let create_resp = client
        .post(format!("{base_url}/api/projects/mutate"))
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .json(&json!({
            "type": "project.create",
            "projectId": "proj-http-1",
            "title": "HTTP Project",
            "workspaceRoot": ws_path,
            "pinned": true
        }))
        .send()
        .await?;
    assert_eq!(create_resp.status(), StatusCode::OK);
    let create_val: Value = create_resp.json().await?;
    assert_eq!(create_val["project"]["id"], "proj-http-1");
    assert!(create_val["project"]["pinnedAt"].is_string());

    let update_resp = client
        .post(format!("{base_url}/api/projects/mutate"))
        .header(COOKIE, cookie_kv)
        .json(&json!({
            "type": "project.update",
            "projectId": "proj-http-1",
            "title": "Renamed HTTP Project",
            "autoPull": true
        }))
        .send()
        .await?;
    assert_eq!(update_resp.status(), StatusCode::OK);

    let list_resp = client
        .get(format!("{base_url}/api/projects"))
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .send()
        .await?;
    assert_eq!(list_resp.status(), StatusCode::OK);
    let list_val: Value = list_resp.json().await?;
    let projects = list_val["projects"]
        .as_array()
        .ok_or("missing projects array")?;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["title"], "Renamed HTTP Project");
    assert_eq!(projects[0]["autoPull"], true);

    let del_resp = client
        .post(format!("{base_url}/api/projects/mutate"))
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .json(&json!({ "type": "project.delete", "projectId": "proj-http-1" }))
        .send()
        .await?;
    assert_eq!(del_resp.status(), StatusCode::OK);
    let del_val: Value = del_resp.json().await?;
    assert_eq!(
        del_val["snapshot"]["projects"]
            .as_array()
            .ok_or("missing projects")?
            .len(),
        0
    );
    Ok(())
}

async fn verify_otlp_traces(
    client: &reqwest::Client,
    base_url: &str,
    logs_dir: &Path,
) -> TestResult {
    let otlp_payload = json!({
        "resourceSpans": [{
            "scopeSpans": [{
                "spans": [
                    {
                        "traceId": "aaa1",
                        "spanId": "bbb1",
                        "name": "rpc.dispatch",
                        "startTimeUnixNano": "1700000000000000000",
                        "endTimeUnixNano": "1700000000025000000",
                        "status": { "code": 1 },
                        "attributes": [{ "key": "http.route", "value": { "stringValue": "/ws" } }]
                    },
                    {
                        "traceId": "aaa2",
                        "spanId": "bbb2",
                        "name": "provider.turn",
                        "startTimeUnixNano": "1700000001000000000",
                        "endTimeUnixNano": "1700000001100000000",
                        "status": { "code": 2, "message": "boom" },
                        "attributes": []
                    }
                ]
            }]
        }]
    });
    let trace_resp = client
        .post(format!("{base_url}/api/observability/v1/traces"))
        .json(&otlp_payload)
        .send()
        .await?;
    assert_eq!(trace_resp.status(), StatusCode::OK);
    let trace_file = logs_dir.join("traces.ndjson");
    let summary = summarize_trace_files(&trace_file, DEFAULT_TRACE_MAX_FILES, None)?;
    assert_eq!(summary.span_count, 2);
    assert_eq!(summary.skipped_line_count, 0);
    let total_failures: usize = summary.spans.iter().map(|s| s.failure_count).sum();
    assert_eq!(total_failures, 1);
    Ok(())
}

#[tokio::test]
async fn pairing_auth_bug03_projects_and_otlp_traces_end_to_end() -> TestResult {
    let (base_url, db, logs_dir, _srv) = spawn_test_server(None).await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let (bearer, cookie_kv) = verify_pairing_and_bug03(&client, &base_url, &db).await?;
    verify_project_mutations(&client, &base_url, &bearer, &cookie_kv).await?;
    verify_otlp_traces(&client, &base_url, &logs_dir).await?;
    let _ = std::fs::remove_dir_all(&logs_dir);
    Ok(())
}
