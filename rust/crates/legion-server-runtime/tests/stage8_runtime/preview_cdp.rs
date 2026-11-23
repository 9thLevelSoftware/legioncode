//! Stage 8 tests for headless `PreviewBrowserRuntime` CDP WebSocket RPC & launch diagnostics.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use legion_persistence::pairing_tokens::{IssuePairingTokenInput, issue_pairing_token_with_conn};
use legion_persistence::sqlite::SqliteStore;
use legion_server_runtime::{
    PreviewLaunchDiagnostic, RuntimeHttpState, diagnose_browser_launch_failure, runtime_full_router,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

use super::{TestResult, now_unix_ms};

async fn handle_mock_cdp_socket(mut socket: WebSocket, methods_seen: Arc<Mutex<Vec<String>>>) {
    while let Some(Ok(msg)) = socket.recv().await {
        let Message::Text(text) = msg else {
            continue;
        };
        let Ok(req) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let id = req.get("id").and_then(Value::as_u64).unwrap_or(1);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        if let Ok(mut guard) = methods_seen.lock() {
            guard.push(method.to_owned());
        }
        let result = match method {
            "Target.createTarget" => json!({ "targetId": "target-1" }),
            "Page.navigate" => json!({ "frameId": "frame-1" }),
            "Runtime.evaluate" => json!({ "result": { "type": "number", "value": 2 } }),
            "Page.captureScreenshot" => json!({ "data": "iVBORw0KGgo=" }),
            _ => json!({}),
        };
        let reply = json!({ "id": id, "result": result }).to_string();
        if socket.send(Message::Text(reply.into())).await.is_err() {
            break;
        }
    }
}

async fn spawn_mock_cdp_ws() -> Result<
    (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>),
    Box<dyn std::error::Error>,
> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    let app = Router::new().route(
        "/devtools/page/1",
        get(move |ws: WebSocketUpgrade| {
            let log = Arc::clone(&seen_clone);
            async move { ws.on_upgrade(move |sock| handle_mock_cdp_socket(sock, log)) }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("ws://{addr}/devtools/page/1"), seen, handle))
}

fn verify_launch_diagnostics() {
    assert!(matches!(
        diagnose_browser_launch_failure(
            "chrome",
            "No usable sandbox!",
            "legioncode browser setup",
            true,
            true
        ),
        Some(PreviewLaunchDiagnostic::RootSandbox { .. })
    ));
    assert!(matches!(
        diagnose_browser_launch_failure(
            "chrome",
            "No usable sandbox!",
            "legioncode browser setup",
            false,
            true
        ),
        Some(PreviewLaunchDiagnostic::AppArmorSandbox { .. })
    ));
    assert!(matches!(
        diagnose_browser_launch_failure(
            "chrome",
            "error while loading shared libraries: libnss3.so: cannot open",
            "legioncode browser setup",
            false,
            true
        ),
        Some(PreviewLaunchDiagnostic::MissingLibraries { libraries, .. })
            if libraries == vec!["libnss3.so".to_owned()]
    ));
}

pub async fn run_preview_cdp_tests() -> TestResult {
    verify_launch_diagnostics();

    // 2. Drive live CDP actions over /api/preview/action against a local WS CDP server.
    let (ws_url, seen, ws_srv) = spawn_mock_cdp_ws().await?;
    let dir = TempDir::new()?;
    let store = SqliteStore::open_in_memory()?;
    let db = Arc::new(Mutex::new(store.into_connection()));
    let pair_code = {
        let conn = db.lock().map_err(|_| "db lock")?;
        issue_pairing_token_with_conn(&conn, &IssuePairingTokenInput::default(), now_unix_ms())?
            .credential
    };
    let state =
        Arc::new(RuntimeHttpState::new(Arc::clone(&db), dir.path()).with_require_auth(true));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base = format!("http://{addr}");
    let http_srv = tokio::spawn(async move {
        let _ = axum::serve(listener, runtime_full_router(state)).await;
    });

    let client = reqwest::Client::new();
    let pair_resp: Value = client
        .post(format!("{base}/api/auth/pair"))
        .json(&json!({ "token": pair_code }))
        .send()
        .await?
        .json()
        .await?;
    let bearer = pair_resp["accessToken"].as_str().ok_or("missing token")?;

    let post_action = |payload: Value| {
        let c = client.clone();
        let url = format!("{base}/api/preview/action");
        let tok = bearer.to_owned();
        async move { c.post(url).bearer_auth(tok).json(&payload).send().await }
    };

    let conn_res = post_action(
        json!({ "action": "connect", "wsUrl": ws_url, "targetUrl": "http://localhost:3000" }),
    )
    .await?;
    assert_eq!(conn_res.status(), StatusCode::OK);
    let nav_res =
        post_action(json!({ "action": "navigate", "url": "http://localhost:3000" })).await?;
    assert_eq!(nav_res.status(), StatusCode::OK);
    let eval_val: Value = post_action(json!({ "action": "evaluate", "expression": "1 + 1" }))
        .await?
        .json()
        .await?;
    assert_eq!(eval_val["result"]["value"], 2);
    let shot_val: Value = post_action(json!({ "action": "screenshot", "format": "png" }))
        .await?
        .json()
        .await?;
    assert_eq!(shot_val["data"], "iVBORw0KGgo=");
    assert_eq!(post_action(json!({ "action": "mouseEvent", "eventType": "mousePressed", "x": 10.0, "y": 20.0, "button": "left", "clickCount": 1 })).await?.status(), StatusCode::OK);
    assert_eq!(
        post_action(json!({ "action": "keyEvent", "eventType": "keyDown", "key": "Enter" }))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_action(json!({ "action": "setViewport", "width": 1280, "height": 720 }))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_action(json!({ "action": "close" })).await?.status(),
        StatusCode::OK
    );

    let methods = seen.lock().map_err(|_| "seen lock")?.clone();
    for expected in [
        "Page.enable",
        "Target.createTarget",
        "Page.navigate",
        "Runtime.evaluate",
        "Page.captureScreenshot",
        "Input.dispatchMouseEvent",
        "Input.dispatchKeyEvent",
        "Emulation.setDeviceMetricsOverride",
    ] {
        assert!(
            methods.iter().any(|m| m == expected),
            "missing CDP call {expected}"
        );
    }

    http_srv.abort();
    ws_srv.abort();
    Ok(())
}
