//! Live HTTP integration tests for [`legion_server_runtime::static_assets_router`].

use std::sync::{Arc, Mutex};

use legion_persistence::sqlite::SqliteStore;
use legion_server_runtime::{RuntimeHttpState, runtime_full_router};
use reqwest::StatusCode;
use reqwest::header::{IF_NONE_MATCH, RANGE};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn spawn_static_server(
    static_dir: std::path::PathBuf,
) -> Result<(String, std::path::PathBuf, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let logs_dir = TempDir::new()?.keep();
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let state = Arc::new(
        RuntimeHttpState::new(db, &logs_dir)
            .with_static_dir(Some(static_dir))
            .with_require_auth(false),
    );
    let router = runtime_full_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), logs_dir, handle))
}

async fn verify_spa_and_csp(client: &reqwest::Client, base_url: &str) -> TestResult {
    for path in ["/", "/threads/non-existent"] {
        let resp = client.get(format!("{base_url}{path}")).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-frame-options")
                .and_then(|v| v.to_str().ok()),
            Some("DENY")
        );
        assert_eq!(
            resp.headers()
                .get("content-security-policy")
                .and_then(|v| v.to_str().ok()),
            Some("frame-ancestors 'none'")
        );
    }

    let html_resp = client.get(format!("{base_url}/embed.html")).send().await?;
    assert_eq!(
        html_resp
            .headers()
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok()),
        Some("sandbox allow-scripts allow-forms allow-popups allow-downloads")
    );
    let svg_resp = client.get(format!("{base_url}/logo.svg")).send().await?;
    assert_eq!(
        svg_resp
            .headers()
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok()),
        Some("default-src 'none'; style-src 'unsafe-inline'; sandbox")
    );
    Ok(())
}

async fn verify_etag_ranges_and_traversal(client: &reqwest::Client, base_url: &str) -> TestResult {
    let js_resp = client
        .get(format!("{base_url}/assets/app-deadbeef.js"))
        .send()
        .await?;
    assert_eq!(js_resp.status(), StatusCode::OK);
    assert_eq!(
        js_resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=31536000, immutable")
    );
    let etag = js_resp
        .headers()
        .get("etag")
        .ok_or("missing etag")?
        .to_str()?
        .to_owned();
    let not_modified = client
        .get(format!("{base_url}/assets/app-deadbeef.js"))
        .header(IF_NONE_MATCH, etag)
        .send()
        .await?;
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);

    let partial = client
        .get(format!("{base_url}/video.bin"))
        .header(RANGE, "bytes=2-5")
        .send()
        .await?;
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        partial
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some("bytes 2-5/10")
    );
    assert_eq!(partial.text().await?, "2345");

    let oob = client
        .get(format!("{base_url}/video.bin"))
        .header(RANGE, "bytes=999-")
        .send()
        .await?;
    assert_eq!(oob.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    let authority = base_url.trim_start_matches("http://");
    for bad in ["/%2e%2e/secret.txt", "/../secret.txt", "/foo%00bar"] {
        let mut stream = tokio::net::TcpStream::connect(authority).await?;
        let req = format!("GET {bad} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await?;
        let mut raw_resp = String::new();
        let _ = stream.read_to_string(&mut raw_resp).await?;
        assert!(
            raw_resp.starts_with("HTTP/1.1 400 "),
            "expected 400 for `{bad}`, got: {raw_resp}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn static_assets_router_handles_spa_csp_etag_ranges_and_traversal() -> TestResult {
    let dist_dir = TempDir::new()?;
    let root = dist_dir.path();
    std::fs::create_dir_all(root.join(".vite"))?;
    std::fs::create_dir_all(root.join("assets"))?;
    std::fs::write(
        root.join("index.html"),
        "<!doctype html><html><body>SPA</body></html>",
    )?;
    std::fs::write(root.join("embed.html"), "<html><body>Widget</body></html>")?;
    std::fs::write(
        root.join("logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
    )?;
    std::fs::write(
        root.join("assets").join("app-deadbeef.js"),
        "console.log('hi');",
    )?;
    std::fs::write(root.join("video.bin"), b"0123456789")?;
    std::fs::write(
        root.join(".vite").join("manifest.json"),
        r#"{"src/main.ts":{"file":"assets/app-deadbeef.js"}}"#,
    )?;

    let (base_url, logs_dir, _srv) = spawn_static_server(root.to_path_buf()).await?;
    let client = reqwest::Client::new();
    verify_spa_and_csp(&client, &base_url).await?;
    verify_etag_ranges_and_traversal(&client, &base_url).await?;
    let _ = std::fs::remove_dir_all(&logs_dir);
    Ok(())
}
