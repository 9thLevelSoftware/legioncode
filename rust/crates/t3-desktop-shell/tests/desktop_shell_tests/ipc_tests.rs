//! Tests for `DesktopIpcRouter` across all 6 typed Tauri v2 IPC JSON envelopes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use t3_desktop_shell::ipc::{
    ClampWindowBoundsResult, DesktopIpcRequest, DesktopIpcRouter, PreflightDbResult,
    RenderServiceUnitsResult, UpdateStepResult,
};
use t3_desktop_shell::sidecar::{SidecarConfig, SidecarController};
use t3_edge::boot_service_units::{
    BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_TASK_FILE, BOOT_SERVICE_UNIT_FILE,
    extract_service_unit_base_dir,
};
use t3_edge::desktop_updates::{UpdateErrorContext, UpdateStatus, compute_sha512_base64};
use t3_surfaces::desktop::{
    DEFAULT_MAIN_WINDOW_HEIGHT, DEFAULT_MAIN_WINDOW_WIDTH, InitialWindowBounds, WindowBounds,
};

static IPC_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct IpcTempDir {
    path: PathBuf,
}

impl IpcTempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        let idx = IPC_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{idx}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IpcTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn dispatch(router: &mut DesktopIpcRouter, method: &str, params: Value) -> Result<Value, String> {
    let resp = router.dispatch(DesktopIpcRequest {
        id: Some(json!("req-1")),
        method: method.to_owned(),
        params,
    });
    assert_eq!(resp.id, Some(json!("req-1")));
    if !resp.ok {
        return Err(format!("IPC error: {:?}", resp.error));
    }
    resp.result.ok_or_else(|| "missing IPC result".to_owned())
}

#[test]
fn ipc_bootstrap_combines_surfaces_policy_paths_and_sidecar() -> Result<(), String> {
    let dir = IpcTempDir::new("t3-ipc-bootstrap")?;
    let mut router = DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
        base_dir: dir.path().to_path_buf(),
        ..SidecarConfig::default()
    }));

    let val = dispatch(
        &mut router,
        "desktop.bootstrap",
        json!({
            "appVersion": "0.1.10-nightly.1",
            "clerkFrontendApiHostname": "clerk.t3.codes",
            "baseDir": dir.path().to_string_lossy(),
            "persistedBounds": { "x": -1, "y": -1, "width": 1922, "height": 1082 },
            "displays": [{ "x": 0, "y": 0, "width": 1920, "height": 1080 }],
            "managedPolicy": {
                "schemaVersion": 1,
                "organization": "Example Corp",
                "limits": {
                    "updates": {
                        "mode": "auto",
                        "allowNightly": false
                    }
                }
            }
        }),
    )?;

    assert_eq!(val["identifier"], "com.t3tools.t3code");
    assert_eq!(val["scheme"], "t3code");
    assert_eq!(val["desktopUrl"], "t3code://app/");
    assert_eq!(val["branding"]["stageLabel"], "Nightly");
    assert!(
        val["contentSecurityPolicy"]
            .as_str()
            .is_some_and(|csp| csp.contains("https://clerk.t3.codes"))
    );
    assert_eq!(val["preflight"]["status"], "ready");
    assert_eq!(val["updatesEnabled"], false);
    assert_eq!(val["updateStatus"], "disabled");
    Ok(())
}

#[test]
fn ipc_resolve_scheme_and_clamp_window_bounds() -> Result<(), String> {
    let mut router = DesktopIpcRouter::default();

    let ok_asset = dispatch(
        &mut router,
        "desktop.resolveScheme",
        json!({
            "method": "GET",
            "url": "t3code://app/assets/app.js",
            "assets": ["index.html", "assets/app.js"]
        }),
    )?;
    assert_eq!(ok_asset["status"], 200);
    assert_eq!(ok_asset["allowed"], true);
    assert_eq!(
        ok_asset["decision"]["serveAsset"]["relative_path"],
        "assets/app.js"
    );

    let traversal = dispatch(
        &mut router,
        "desktop.resolveScheme",
        json!({
            "method": "GET",
            "url": "t3code://app/%2e%2e%2fetc/passwd"
        }),
    )?;
    assert_eq!(traversal["status"], 404);
    assert_eq!(traversal["allowed"], false);
    assert_eq!(
        traversal["decision"]["rejected"]["reason"],
        "path_traversal"
    );

    let bounds: ClampWindowBoundsResult = serde_json::from_value(dispatch(
        &mut router,
        "desktop.clampWindowBounds",
        json!({
            "bounds": { "x": 9000, "y": -500, "width": 3000, "height": 200 },
            "persistedBounds": { "x": -2, "y": 0, "width": 1000, "height": 700 },
            "normalBounds": { "x": 100, "y": 100, "width": 900, "height": 600 },
            "displays": [
                { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                { "x": 1920, "y": 0, "width": 1440, "height": 900 }
            ]
        }),
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(
        bounds.clamped,
        WindowBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 360
        }
    );
    assert_eq!(
        bounds.initial,
        InitialWindowBounds::DefaultSize {
            width: DEFAULT_MAIN_WINDOW_WIDTH,
            height: DEFAULT_MAIN_WINDOW_HEIGHT
        }
    );
    assert!(!bounds.fits_within_any_display);
    assert!(!bounds.meets_minimum_size);
    assert!(bounds.normal_on_window_display.is_some());

    let missing_bounds = router.dispatch(DesktopIpcRequest {
        id: None,
        method: "desktop.clampWindowBounds".to_owned(),
        params: json!({ "displays": [] }),
    });
    assert!(!missing_bounds.ok);
    assert_eq!(
        missing_bounds.error.as_ref().map(|e| e.code.as_str()),
        Some("InvalidParams")
    );
    Ok(())
}

#[test]
fn ipc_update_step_preserves_v1_on_v2_failure_and_validates_digests() -> Result<(), String> {
    let mut router = DesktopIpcRouter::default();
    let payload = "t3code-v1-installer";
    let sha512 = compute_sha512_base64(payload.as_bytes());

    // Invalid payload digest is rejected cleanly.
    let bad_digest = router.dispatch(DesktopIpcRequest {
        id: None,
        method: "desktop.updateStep".to_owned(),
        params: json!({
            "action": "downloadComplete",
            "version": "1.1.0",
            "sha512": sha512,
            "sizeBytes": payload.len(),
            "payloadUtf8": "tampered"
        }),
    });
    assert!(!bad_digest.ok);
    assert_eq!(
        bad_digest.error.as_ref().map(|e| e.code.as_str()),
        Some("DigestVerificationError")
    );

    // Valid V1 installable download followed by V2 check + failed V2 download preserves V1 (BUG-28).
    let step1: UpdateStepResult = serde_json::from_value(dispatch(
        &mut router,
        "desktop.updateStep",
        json!({
            "action": "downloadComplete",
            "version": "1.1.0",
            "sha512": sha512,
            "sizeBytes": payload.len(),
            "payloadUtf8": payload
        }),
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(step1.status, UpdateStatus::Downloaded);

    let _ = dispatch(
        &mut router,
        "desktop.updateStep",
        json!({ "action": "beginCheck", "checkedAt": "2026-11-20T00:00:00Z" }),
    )?;
    let _ = dispatch(
        &mut router,
        "desktop.updateStep",
        json!({
            "action": "updateAvailable",
            "version": "1.2.0",
            "checkedAt": "2026-11-20T00:00:01Z"
        }),
    )?;
    let _ = dispatch(
        &mut router,
        "desktop.updateStep",
        json!({ "action": "beginDownload" }),
    )?;
    let after_fail: UpdateStepResult = serde_json::from_value(dispatch(
        &mut router,
        "desktop.updateStep",
        json!({ "action": "downloadFailure", "message": "net::ERR_TIMED_OUT" }),
    )?)
    .map_err(|e| e.to_string())?;

    assert_eq!(after_fail.status, UpdateStatus::Downloaded);
    assert_eq!(after_fail.error_context, Some(UpdateErrorContext::Download));
    assert_eq!(after_fail.pending_check, None);
    assert_eq!(
        after_fail
            .installable_artifact
            .as_ref()
            .map(|a| a.version.as_str()),
        Some("1.1.0")
    );
    Ok(())
}

#[test]
fn ipc_preflight_db_render_service_units_and_unknown_method() -> Result<(), String> {
    let dir = IpcTempDir::new("t3-ipc-db-units")?;
    let mut router = DesktopIpcRouter::default();

    let corrupt_path = dir.path().join("corrupt.sqlite");
    fs::write(&corrupt_path, b"invalid-sqlite").map_err(|e| e.to_string())?;
    let preflight: PreflightDbResult = serde_json::from_value(dispatch(
        &mut router,
        "desktop.preflightDb",
        json!({
            "databasePath": corrupt_path.to_string_lossy(),
            "launcherProtocol": 4,
            "version": "0.1.10"
        }),
    )?)
    .map_err(|e| e.to_string())?;
    assert!(!preflight.ready);

    let units: RenderServiceUnitsResult = serde_json::from_value(dispatch(
        &mut router,
        "desktop.renderServiceUnits",
        json!({ "baseDir": "/home/theo/.t3" }),
    )?)
    .map_err(|e| e.to_string())?;
    assert_eq!(units.file_names.systemd, BOOT_SERVICE_UNIT_FILE);
    assert_eq!(units.file_names.launchd, BOOT_SERVICE_PLIST_FILE);
    assert_eq!(units.file_names.windows_task, BOOT_SERVICE_TASK_FILE);
    assert_eq!(
        extract_service_unit_base_dir(&units.systemd_unit).as_deref(),
        Some("/home/theo/.t3")
    );
    assert_eq!(
        extract_service_unit_base_dir(&units.launchd_plist).as_deref(),
        Some("/home/theo/.t3")
    );
    assert_eq!(
        extract_service_unit_base_dir(&units.windows_task_xml).as_deref(),
        Some("/home/theo/.t3")
    );
    assert!(units.windows_task_utf16le_bom_bytes > units.windows_task_xml.len() * 2);

    let unknown = router.dispatch_json(r#"{"id":"x","method":"desktop.nope","params":{}}"#);
    let resp = unknown.map_err(|e| e.to_string())?;
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("UnknownMethod")
    );
    Ok(())
}
