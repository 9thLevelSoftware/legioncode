//! Integration tests for `DesktopIpcRouter` `window.desktopBridge` handlers:
//! settings/catalog persistence, loopback OAuth bearer exchange, URL/media guards,
//! updater transitions, and snapshot/preview/exposure/WSL controls.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use std::process::Command;
use std::time::Duration;

use legion_desktop_shell::http_loopback::{base64_decode, is_valid_external_url};
use legion_desktop_shell::ipc::{DesktopIpcRequest, DesktopIpcRouter};
use legion_desktop_shell::native_dialogs::to_local_backend_path;
use legion_desktop_shell::sidecar::{SidecarConfig, SidecarController};
use legion_desktop_shell::sidecar_process::wait_until_environment_ready;
use serde_json::{Value, json};

static BRIDGE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct BridgeTempDir {
    path: PathBuf,
}

impl BridgeTempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        let idx = BRIDGE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{idx}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BridgeTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn dispatch(router: &mut DesktopIpcRouter, method: &str, params: Value) -> Result<Value, String> {
    let resp = router.dispatch(DesktopIpcRequest {
        id: Some(json!(1)),
        method: method.to_owned(),
        params,
    });
    if resp.ok {
        Ok(resp.result.unwrap_or(Value::Null))
    } else {
        Err(resp
            .error
            .map_or_else(|| "unknown error".to_owned(), |e| e.message))
    }
}

fn spawn_mock_auth_media_server() -> Result<(u16, thread::JoinHandle<()>), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let srv = thread::spawn(move || {
        for idx in 0..3 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let resp = if req.contains("POST /oauth/token") {
                let body = format!(r#"{{"access_token":"bearer-tok-{idx}","expires_in":3600}}"#);
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                let body = "PNGDATA";
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    Ok((port, srv))
}

#[test]
fn desktop_ipc_router_persists_settings_catalog_and_exchanges_tokens() -> Result<(), String> {
    let tmp = BridgeTempDir::new("legion-bridge-ipc")?;
    let (port, srv) = spawn_mock_auth_media_server()?;

    let mut router = DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
        base_dir: tmp.path().to_path_buf(),
        base_dir_is_explicit: true,
        app_version: "0.1.11".to_owned(),
        ..SidecarConfig::default()
    }))
    .with_local_bootstrap(
        Some(format!("http://127.0.0.1:{port}")),
        Some(format!("ws://127.0.0.1:{port}/ws")),
        Some("mock-bootstrap-secret".to_owned()),
    );

    assert_eq!(
        dispatch(&mut router, "desktop.bridge.getClientSettings", Value::Null)?,
        Value::Null
    );
    dispatch(
        &mut router,
        "desktop.bridge.setClientSettings",
        json!({ "uiScale": 1.1 }),
    )?;
    assert_eq!(
        dispatch(&mut router, "desktop.bridge.getClientSettings", Value::Null)?["uiScale"],
        1.1
    );
    assert!(tmp.path().join("userdata/client-settings.json").is_file());

    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.getConnectionCatalog",
            Value::Null
        )?,
        Value::Null
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.setConnectionCatalog",
            json!("not-json")
        )?,
        false
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.setConnectionCatalog",
            json!(r#"{"targets":[]}"#)
        )?,
        true
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.getConnectionCatalog",
            Value::Null
        )?,
        r#"{"targets":[]}"#
    );
    assert!(
        tmp.path()
            .join("userdata/connection-catalog.json")
            .is_file()
    );

    let tok1 = dispatch(
        &mut router,
        "desktop.bridge.getLocalEnvironmentBearerToken",
        Value::Null,
    )?;
    assert_eq!(tok1, "bearer-tok-0");
    let cached = dispatch(
        &mut router,
        "desktop.bridge.getLocalEnvironmentBearerToken",
        json!({ "forceRefresh": false }),
    )?;
    assert_eq!(cached, "bearer-tok-0");
    let refreshed = dispatch(
        &mut router,
        "desktop.bridge.getLocalEnvironmentBearerToken",
        json!({ "forceRefresh": true }),
    )?;
    assert_eq!(refreshed, "bearer-tok-1");

    let media = dispatch(
        &mut router,
        "desktop.bridge.readMedia",
        json!({ "url": format!("http://127.0.0.1:{port}/a.png") }),
    )?;
    assert_eq!(media["status"], 200);
    assert_eq!(
        base64_decode(media["dataBase64"].as_str().unwrap_or_default())?,
        b"PNGDATA"
    );
    let _ = srv.join();
    Ok(())
}

#[test]
fn desktop_ipc_router_validates_urls_updates_snapshot_and_preview() -> Result<(), String> {
    let tmp = BridgeTempDir::new("legion-bridge-guards")?;
    let mut router = DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
        base_dir: tmp.path().to_path_buf(),
        base_dir_is_explicit: true,
        app_version: "0.1.11".to_owned(),
        ..SidecarConfig::default()
    }));

    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.readMedia",
            json!({ "url": "file:///etc/passwd" })
        )?,
        Value::Null
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.openExternal",
            json!({ "url": "javascript:alert(1)" })
        )?,
        false
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.openExternal",
            json!({ "url": "file:///C:/Windows/win.ini" })
        )?,
        false
    );
    assert!(is_valid_external_url("https://legion.codes/docs?a=1"));
    assert!(is_valid_external_url("mailto:support@legion.codes"));
    assert!(!is_valid_external_url("javascript:alert(1)"));

    assert_eq!(
        dispatch(&mut router, "desktop.bridge.getSnapShotState", Value::Null)?["mode"],
        "unavailable"
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.preview.getPreviewConfig",
            json!({ "environmentId": "primary" })
        )?["preloadUrl"],
        Value::Null
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.preview.sharesLoopback",
            json!({ "environmentId": "primary" })
        )?,
        true
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.getServerExposureState",
            Value::Null
        )?["mode"],
        "local-only"
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.setServerExposureMode",
            json!({ "mode": "network-accessible" })
        )?["mode"],
        "network-accessible"
    );
    assert_eq!(
        dispatch(
            &mut router,
            "desktop.bridge.setWslBackendEnabled",
            json!({ "enabled": true })
        )?["enabled"],
        true
    );
    assert_eq!(
        to_local_backend_path("wsl:Ubuntu", r"C:\Users\dev\repo"),
        Some("/mnt/c/Users/dev/repo".to_owned())
    );

    let update_check = dispatch(&mut router, "desktop.bridge.checkForUpdate", Value::Null)?;
    assert_eq!(update_check["checked"], true);
    assert_eq!(update_check["state"]["status"], "up-to-date");
    let nightly = dispatch(
        &mut router,
        "desktop.bridge.setUpdateChannel",
        json!({ "channel": "nightly" }),
    )?;
    assert_eq!(nightly["channel"], "nightly");
    Ok(())
}

#[test]
fn sidecar_environment_readiness_poll_retries_and_honors_early_exit() -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let srv = thread::spawn(move || {
        for attempt in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let resp = if attempt == 0 {
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            };
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });

    let base_url = format!("http://127.0.0.1:{port}");
    wait_until_environment_ready(&base_url, Duration::from_secs(3), || Ok(()))?;
    let _ = srv.join();

    let exited = wait_until_environment_ready(&base_url, Duration::from_secs(2), || {
        Err("sidecar exited prematurely".to_owned())
    });
    assert!(exited.is_err());

    let bin = env!("CARGO_BIN_EXE_legioncode-desktop");
    let smoke = Command::new(bin)
        .arg("--headless-smoke")
        .output()
        .map_err(|e| e.to_string())?;
    assert!(
        smoke.status.success(),
        "--headless-smoke failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&smoke.stdout),
        String::from_utf8_lossy(&smoke.stderr)
    );
    Ok(())
}
