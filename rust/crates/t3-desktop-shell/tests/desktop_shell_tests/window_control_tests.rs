//! Integration tests for `DesktopControlServer`, custom-protocol request routing, and live
//! `--smoke-webview` execution via the `t3-desktop` binary.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use http::{Method, Request, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use t3_desktop_shell::control_server::{
    DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION, DesktopControlPlatform, DesktopControlRequest,
    DesktopControlResponse, DesktopControlServer, DesktopControlStatus, handle_activation_line,
    resolve_desktop_app_control_address, send_loopback_request,
};
use t3_desktop_shell::window_host_protocol::{
    CustomProtocolMetrics, build_smoke_html, normalize_custom_protocol_url,
    resolve_custom_protocol_request,
};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ControlTempDir {
    path: PathBuf,
}

impl ControlTempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        let idx = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{idx}", std::process::id()));
        fs::create_dir_all(&path).map_err(|err| err.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn resolve_control_address_matches_shared_ts_parity() {
    let state_dir = "/Users/alice/.t3/userdata";
    let expected_hash = hex::encode(&Sha256::digest(state_dir.as_bytes())[..12]);

    let res = |plat, tmp, uid| resolve_desktop_app_control_address(state_dir, plat, tmp, uid);

    let win = res(DesktopControlPlatform::Win32, r"C:\Temp", None);
    assert_eq!(win.address, format!(r"\\.\pipe\t3code-app-{expected_hash}"));
    assert_eq!(win.directory, None);

    let linux_short = res(DesktopControlPlatform::Linux, "/tmp", Some(501));
    assert_eq!(
        linux_short.address,
        format!("/tmp/t3code-501/{expected_hash}.sock")
    );
    assert_eq!(linux_short.directory.as_deref(), Some("/tmp/t3code-501"));

    let fallback_hash_user = &expected_hash[..12];
    let long_tmp =
        "/tmp/extremely-long-xdg-runtime-directory-exceeding-unix-sun-path-byte-bounds-here";
    let linux_fallback = res(DesktopControlPlatform::Linux, long_tmp, None);
    assert_eq!(
        linux_fallback.address,
        format!("/tmp/t3code-{fallback_hash_user}/{expected_hash}.sock")
    );

    let border_103 = "/tmp/12345678901234567890123456789012345678901234567890123456789";
    let candidate_103 = format!("{border_103}/t3code-9/{expected_hash}.sock");
    assert_eq!(candidate_103.len(), 103);
    let darwin_exact = res(DesktopControlPlatform::Darwin, border_103, Some(9));
    assert_eq!(darwin_exact.address, candidate_103);

    let border_104 = "/tmp/x12345678901234567890123456789012345678901234567890123456789";
    let candidate_104 = format!("{border_104}/t3code-9/{expected_hash}.sock");
    assert_eq!(candidate_104.len(), 104);
    let darwin_fallback = res(DesktopControlPlatform::Darwin, border_104, Some(9));
    assert_eq!(
        darwin_fallback.address,
        format!("/tmp/t3code-9/{expected_hash}.sock")
    );
    let linux_104 = res(DesktopControlPlatform::Linux, border_104, Some(9));
    assert_eq!(linux_104.address, candidate_104);
}

#[test]
fn control_server_loopback_and_native_transport_roundtrip() -> Result<(), String> {
    let temp = ControlTempDir::new("t3-control-test")?;
    let recorded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_ref = Arc::clone(&recorded);
    let handler: t3_desktop_shell::control_server::ActivationHandler =
        Arc::new(move |req: DesktopControlRequest| {
            if let Ok(mut guard) = recorded_ref.lock() {
                guard.push(req.workspace_root.clone());
            }
            DesktopControlResponse::activated(req.request_id, req.workspace_root)
        });

    let malformed = handle_activation_line("{not-json", &handler);
    assert_eq!(malformed.status, DesktopControlStatus::Error);
    assert_eq!(malformed.code.as_deref(), Some("invalid-request"));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;

    rt.block_on(async {
        let server = DesktopControlServer::bind_loopback(handler.clone())
            .await
            .map_err(|err| err.to_string())?;
        let addr = server
            .local_addr()
            .ok_or_else(|| "missing loopback addr".to_owned())?;

        let ok = send_loopback_request(
            addr,
            &DesktopControlRequest {
                version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
                request_id: "req-1".to_owned(),
                workspace_root: "/workspace/demo".to_owned(),
                request_type: Some("open-workspace".to_owned()),
                platform: Some("win32".to_owned()),
            },
        )
        .await
        .map_err(|err| err.to_string())?;
        assert_eq!(ok.status, DesktopControlStatus::Activated);
        assert!(ok.ok);
        assert_eq!(ok.request_id, "req-1");
        assert_eq!(ok.workspace_root.as_deref(), Some("/workspace/demo"));

        let bad_ver = send_loopback_request(
            addr,
            &DesktopControlRequest {
                version: 42,
                request_id: "req-bad-ver".to_owned(),
                workspace_root: "/workspace/demo".to_owned(),
                request_type: None,
                platform: None,
            },
        )
        .await
        .map_err(|err| err.to_string())?;
        assert_eq!(bad_ver.status, DesktopControlStatus::Error);
        assert_eq!(bad_ver.code.as_deref(), Some("invalid-request"));
        assert_eq!(bad_ver.request_id, "req-bad-ver");

        let bad_kind = send_loopback_request(
            addr,
            &DesktopControlRequest {
                version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
                request_id: "req-bad-kind".to_owned(),
                workspace_root: "/workspace/demo".to_owned(),
                request_type: Some("shutdown".to_owned()),
                platform: None,
            },
        )
        .await
        .map_err(|err| err.to_string())?;
        assert_eq!(bad_kind.status, DesktopControlStatus::Error);
        server.shutdown().await;

        let native = DesktopControlServer::bind_for_state_dir(temp.path(), handler)
            .map_err(|err| err.to_string())?;
        let native_req = DesktopControlRequest {
            version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
            request_id: "req-native".to_owned(),
            workspace_root: temp.path().display().to_string(),
            request_type: Some("open-workspace".to_owned()),
            platform: None,
        };
        #[cfg(windows)]
        let native_resp = t3_desktop_shell::control_server::send_named_pipe_request(
            native.address(),
            &native_req,
        )
        .await
        .map_err(|err| err.to_string())?;
        #[cfg(unix)]
        let native_resp =
            t3_desktop_shell::control_server::send_unix_request(native.address(), &native_req)
                .await
                .map_err(|err| err.to_string())?;
        native.shutdown().await;

        assert_eq!(native_resp.status, DesktopControlStatus::Activated);
        assert!(native_resp.ok);
        Ok::<(), String>(())
    })?;

    let captured = recorded.lock().map_err(|err| err.to_string())?;
    assert_eq!(captured.len(), 2);
    Ok(())
}

#[test]
fn custom_protocol_router_handles_webview2_localhost_and_rejects_traversal_and_foreign_origin()
-> Result<(), String> {
    assert_eq!(
        normalize_custom_protocol_url("t3code", "http://t3code.localhost/assets/smoke.js"),
        "t3code://app/assets/smoke.js"
    );
    assert_eq!(
        normalize_custom_protocol_url("t3code", "t3code://localhost/index.html"),
        "t3code://app/index.html"
    );
    assert_eq!(
        normalize_custom_protocol_url("t3code", "https://evil.example/assets/smoke.js"),
        "https://evil.example/assets/smoke.js"
    );

    let smoke_html = build_smoke_html();
    let mut metrics = CustomProtocolMetrics::default();

    let asset_req = Request::builder()
        .method(Method::GET)
        .uri("http://t3code.localhost/assets/smoke.js")
        .body(Vec::new())
        .map_err(|err| err.to_string())?;
    let asset_resp = resolve_custom_protocol_request(
        "t3code",
        None,
        None,
        Some(smoke_html),
        &asset_req,
        Some(&mut metrics),
    );
    assert_eq!(asset_resp.status(), StatusCode::OK);
    assert_eq!(
        asset_resp
            .headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert!(
        asset_resp
            .headers()
            .get("content-security-policy")
            .is_some()
    );
    assert!(
        std::str::from_utf8(&Cow::into_owned(asset_resp.into_body()))
            .map_err(|err| err.to_string())?
            .contains("__t3SmokeAssetLoaded")
    );

    for bad_uri in [
        "t3code://app/%2e%2e%2fetc/passwd",
        "http://t3code.localhost/assets/..\\secrets.json",
        "https://evil.example/assets/smoke.js",
    ] {
        let bad_req = Request::builder()
            .method(Method::GET)
            .uri(bad_uri)
            .body(Vec::new())
            .map_err(|err| err.to_string())?;
        let bad_resp = resolve_custom_protocol_request(
            "t3code",
            None,
            None,
            Some(smoke_html),
            &bad_req,
            Some(&mut metrics),
        );
        assert_eq!(bad_resp.status(), StatusCode::NOT_FOUND, "uri={bad_uri}");
        assert_eq!(
            bad_resp
                .headers()
                .get("x-content-type-options")
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
    }

    assert!(metrics.served_ok);
    assert!(metrics.traversal_rejected);
    Ok(())
}

#[test]
fn cli_smoke_webview_executes_live_webview2_ipc_and_scheme_checks() -> Result<(), String> {
    let bin = env!("CARGO_BIN_EXE_t3-desktop");
    let out = Command::new(bin)
        .arg("--smoke-webview")
        .output()
        .map_err(|err| err.to_string())?;
    assert!(
        out.status.success(),
        "--smoke-webview failed (status {:?})\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let report: Value = serde_json::from_slice(&out.stdout).map_err(|err| err.to_string())?;
    assert_eq!(report["passed"], true);
    assert_eq!(report["webviewLoaded"], true);
    assert_eq!(report["customProtocolServed"], true);
    assert_eq!(report["schemeTraversalRejectedInWebview"], true);
    assert_eq!(report["exitReason"], "smokeCompleted");
    let round_trips = report["ipcRoundTrips"]
        .as_u64()
        .ok_or_else(|| "missing ipcRoundTrips".to_owned())?;
    assert!(
        round_trips >= 3,
        "expected >= 3 round trips, got {round_trips}"
    );
    assert!(report["clampedBounds"]["width"].as_u64().unwrap_or(0) >= 400);
    assert!(report["clampedBounds"]["height"].as_u64().unwrap_or(0) >= 360);
    Ok(())
}
