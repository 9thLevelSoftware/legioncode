//! Wave 5 security HTTP tests: preview tunnel policy (`#39415`), workspace asset symlink target
//! previewability (`#39491`), and signed URL redaction in HTTP access logs & OTLP traces (`#40133`).

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::http::{Method, StatusCode};
use legion_contracts::policy::{ManagedPolicySummary, ManagedPolicySummaryLimits};
use legion_persistence::sqlite::SqliteStore;
use legion_remote::trace::{DEFAULT_TRACE_MAX_FILES, summarize_trace_files};
use legion_server_runtime::{
    RuntimeHttpState, format_redacted_access_log, resolve_previewable_workspace_asset,
    runtime_full_router,
};
use serde_json::json;
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

#[tokio::test]
async fn preview_tunnel_enforces_policy_headers_and_refuses_unbound_port() -> TestResult {
    let logs = TempDir::new()?;
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));

    let state_disabled = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), logs.path())
            .with_require_auth(false)
            .with_preview_tunnel_policy(false, None),
    );
    let (url_disabled, _h1) = spawn_state_server(state_disabled).await?;
    let client = reqwest::Client::new();
    let resp_disabled = client
        .get(format!("{url_disabled}/api/preview/tunnel?port=1234"))
        .send()
        .await?;
    assert_eq!(resp_disabled.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        resp_disabled
            .headers()
            .get("x-legioncode-tunnel-disabled")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );

    let policy_deny = ManagedPolicySummary {
        organization: "Acme Corp".to_owned(),
        support_url: None,
        locked_settings: Vec::new(),
        limits: Some(ManagedPolicySummaryLimits {
            preview_tunnel: Some(false),
            ..ManagedPolicySummaryLimits::default()
        }),
    };
    let state_policy_denied = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), logs.path())
            .with_require_auth(false)
            .with_preview_tunnel_policy(true, Some(policy_deny)),
    );
    let (url_policy, _h2) = spawn_state_server(state_policy_denied).await?;
    let resp_policy = client
        .get(format!("{url_policy}/api/preview/tunnel?port=1234"))
        .send()
        .await?;
    assert_eq!(resp_policy.status(), StatusCode::FORBIDDEN);

    let probe = std::net::TcpListener::bind("127.0.0.1:0")?;
    let unused_port = probe.local_addr()?.port();
    drop(probe);

    let state_enabled = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), logs.path())
            .with_require_auth(false)
            .with_preview_tunnel_policy(true, None),
    );
    let (url_enabled, _h3) = spawn_state_server(state_enabled).await?;
    let resp_refused = client
        .get(format!(
            "{url_enabled}/api/preview/tunnel?port={unused_port}"
        ))
        .send()
        .await?;
    assert_eq!(resp_refused.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        resp_refused
            .headers()
            .get("x-legioncode-tunnel-port-refused")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );

    let live_listener = TcpListener::bind("127.0.0.1:0").await?;
    let live_port = live_listener.local_addr()?.port();
    let _accept_task = tokio::spawn(async move {
        let _ = live_listener.accept().await;
    });
    let resp_ok = client
        .get(format!("{url_enabled}/api/preview/tunnel?port={live_port}"))
        .send()
        .await?;
    assert_eq!(resp_ok.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn workspace_asset_symlinks_only_serve_previewable_targets_and_logs_redact() -> TestResult {
    let temp = TempDir::new()?;
    let root = temp.path().join("ws");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(&outside)?;
    let real_pdf = root.join("real.pdf");
    let bad_xhtml = root.join("payload.xhtml");
    let outside_pdf = outside.join("secret.pdf");
    std::fs::write(&real_pdf, b"%PDF-1.7\n")?;
    std::fs::write(&bad_xhtml, b"<html xmlns='http://www.w3.org/1999/xhtml'/>")?;
    std::fs::write(&outside_pdf, b"%PDF-1.4\n")?;

    let allowed = vec![root.clone()];
    assert!(resolve_previewable_workspace_asset(&real_pdf, &allowed).is_some());
    assert!(resolve_previewable_workspace_asset(&bad_xhtml, &allowed).is_none());
    assert!(resolve_previewable_workspace_asset(&outside_pdf, &allowed).is_none());

    let logs = TempDir::new()?;
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let state = Arc::new(
        RuntimeHttpState::new(Arc::clone(&db), logs.path())
            .with_require_auth(false)
            .with_allowed_workspace_roots(allowed.clone()),
    );
    let (base_url, _h) = spawn_state_server(Arc::clone(&state)).await?;
    let client = reqwest::Client::new();

    let linked_pdf = root.join("report.pdf");
    if create_file_symlink(&real_pdf, &linked_pdf).is_ok() {
        assert!(resolve_previewable_workspace_asset(&linked_pdf, &allowed).is_some());
        let ok_resp = client
            .get(format!(
                "{base_url}/api/assets/workspace?path={}&token=super-secret-query",
                linked_pdf.to_string_lossy()
            ))
            .send()
            .await?;
        assert_eq!(ok_resp.status(), StatusCode::OK);

        let _ = std::fs::remove_file(&linked_pdf);
        create_file_symlink(&bad_xhtml, &linked_pdf)?;
        assert!(resolve_previewable_workspace_asset(&linked_pdf, &allowed).is_none());
        let bad_resp = client
            .get(format!(
                "{base_url}/api/assets/workspace?path={}",
                linked_pdf.to_string_lossy()
            ))
            .send()
            .await?;
        assert_eq!(bad_resp.status(), StatusCode::FORBIDDEN);
    }

    let line = format_redacted_access_log(
        &Method::POST,
        "/api/hooks/task-1/secret-webhook-token?sig=secret-sig",
        StatusCode::OK,
    );
    assert!(!line.contains("secret-webhook-token"));
    assert!(!line.contains("secret-sig"));
    assert!(line.contains("REDACTED"));

    let span_resp = client
        .post(format!("{base_url}/api/observability/v1/traces"))
        .json(&json!({
            "resourceSpans": [{
                "scopeSpans": [{
                    "spans": [{
                        "traceId": "t1",
                        "spanId": "s1",
                        "name": "GET /api/assets/sig-part/img.png?token=abc123",
                        "startTimeUnixNano": "1000000000",
                        "endTimeUnixNano": "2000000000",
                        "attributes": [
                            { "key": "http.url", "value": { "stringValue": "http://127.0.0.1/api/hooks/t1/my-hook-secret?access_token=tok999" } }
                        ]
                    }]
                }]
            }]
        }))
        .send()
        .await?;
    assert_eq!(span_resp.status(), StatusCode::OK);
    let trace_raw = std::fs::read_to_string(logs.path().join("traces.ndjson"))?;
    assert!(!trace_raw.contains("abc123"));
    assert!(!trace_raw.contains("my-hook-secret"));
    assert!(!trace_raw.contains("tok999"));
    assert!(!trace_raw.contains("sig-part"));
    assert!(
        state
            .access_logs()
            .iter()
            .all(|l| !l.contains("super-secret-query"))
    );
    let summary = summarize_trace_files(
        &logs.path().join("traces.ndjson"),
        DEFAULT_TRACE_MAX_FILES,
        None,
    )?;
    assert_eq!(summary.span_count, 1);
    Ok(())
}

fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
}
