//! Individual self-check probes invoked by [`crate::self_check::run_self_check`].

use std::fs;
use std::path::Path;

use t3_edge::boot_service_units::{decode_service_unit_bytes, extract_service_unit_base_dir};
use t3_edge::desktop_updates::{
    DesktopUpdateChannel, UpdateStatus, compute_sha512_base64, render_aur_launcher,
};
use t3_persistence::service_preflight::{ServicePreflightResult, run_service_preflight};
use t3_surfaces::desktop::{
    DEFAULT_MAIN_WINDOW_HEIGHT, DEFAULT_MAIN_WINDOW_WIDTH, DisplayRect, InitialWindowBounds,
    WindowBounds,
};

use crate::bundle::DesktopBundleManifest;
use crate::ipc::{DesktopIpcRequest, DesktopIpcRouter, RenderServiceUnitsResult, UpdateStepResult};
use crate::self_check::{PreflightCheckSummary, ServiceManifestCheckSummary};
use crate::sidecar::SidecarController;

pub(crate) fn dispatch_typed<T: serde::de::DeserializeOwned>(
    router: &mut DesktopIpcRouter,
    method: &str,
    params: serde_json::Value,
) -> Result<T, String> {
    let resp = router.dispatch(DesktopIpcRequest {
        id: None,
        method: method.to_owned(),
        params,
    });
    if !resp.ok {
        return Err(format!(
            "{method} failed: {:?}",
            resp.error.map(|e| e.message)
        ));
    }
    let val = resp
        .result
        .ok_or_else(|| format!("{method} returned empty result"))?;
    serde_json::from_value(val).map_err(|err| format!("{method} decode error: {err}"))
}

pub(crate) fn verify_scheme_routing(router: &mut DesktopIpcRouter) -> Result<bool, String> {
    let valid: serde_json::Value = dispatch_typed(
        router,
        "desktop.resolveScheme",
        serde_json::json!({
            "method": "GET",
            "url": "t3code://app/assets/main.js",
            "assets": ["index.html", "assets/main.js"]
        }),
    )?;
    if valid["status"].as_u64() != Some(200)
        || valid["allowed"].as_bool() != Some(true)
        || valid["decision"]["serveAsset"]["relative_path"].as_str() != Some("assets/main.js")
    {
        return Err("expected valid t3code://app asset to resolve with 200".to_owned());
    }

    for traversal_url in [
        "t3code://app/%2e%2e%2fetc/passwd",
        "t3code://app/..\\secret.txt",
    ] {
        let rejected: serde_json::Value = dispatch_typed(
            router,
            "desktop.resolveScheme",
            serde_json::json!({ "method": "GET", "url": traversal_url }),
        )?;
        let is_traversal = rejected["status"].as_u64() == Some(404)
            && rejected["decision"]["rejected"]["reason"].as_str() == Some("path_traversal");
        if !is_traversal {
            return Err(format!("expected PathTraversal 404 for {traversal_url}"));
        }
    }

    let foreign: serde_json::Value = dispatch_typed(
        router,
        "desktop.resolveScheme",
        serde_json::json!({ "method": "GET", "url": "t3code://evil.test/index.html" }),
    )?;
    if foreign["status"].as_u64() != Some(404)
        || foreign["decision"]["rejected"]["reason"].as_str() != Some("foreign_origin")
    {
        return Err("expected ForeignOrigin 404 for foreign authority".to_owned());
    }
    Ok(true)
}

pub(crate) fn verify_window_bounds(router: &mut DesktopIpcRouter) -> Result<WindowBounds, String> {
    let display = DisplayRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let result: serde_json::Value = dispatch_typed(
        router,
        "desktop.clampWindowBounds",
        serde_json::json!({
            "bounds": { "x": 9000, "y": -500, "width": 3000, "height": 200 },
            "persistedBounds": { "x": -2, "y": 0, "width": 1000, "height": 700 },
            "displays": [display]
        }),
    )?;
    let clamped: WindowBounds = serde_json::from_value(result["clamped"].clone())
        .map_err(|err| format!("clamped decode error: {err}"))?;
    let expected = WindowBounds {
        x: 0,
        y: 0,
        width: 1920,
        height: 360,
    };
    if clamped != expected {
        return Err(format!("expected {expected:?}, got {clamped:?}"));
    }

    let initial: InitialWindowBounds = serde_json::from_value(result["initial"].clone())
        .map_err(|err| format!("initial decode error: {err}"))?;
    if initial
        != (InitialWindowBounds::DefaultSize {
            width: DEFAULT_MAIN_WINDOW_WIDTH,
            height: DEFAULT_MAIN_WINDOW_HEIGHT,
        })
    {
        return Err(format!(
            "expected DefaultSize fallback for out-of-bounds persisted bounds, got {initial:?}"
        ));
    }
    Ok(clamped)
}

pub(crate) fn verify_bug28_update_preservation(
    router: &mut DesktopIpcRouter,
) -> Result<String, String> {
    let payload = "t3code-v1-verified-installer";
    let digest = compute_sha512_base64(payload.as_bytes());
    let _: UpdateStepResult = dispatch_typed(
        router,
        "desktop.updateStep",
        serde_json::json!({
            "action": "downloadComplete",
            "version": "1.1.0",
            "sha512": digest,
            "sizeBytes": payload.len(),
            "payloadUtf8": payload
        }),
    )?;
    let _: UpdateStepResult = dispatch_typed(
        router,
        "desktop.updateStep",
        serde_json::json!({ "action": "beginCheck", "checkedAt": "2026-11-20T00:00:00Z" }),
    )?;
    let _: UpdateStepResult = dispatch_typed(
        router,
        "desktop.updateStep",
        serde_json::json!({
            "action": "updateAvailable",
            "version": "1.2.0",
            "checkedAt": "2026-11-20T00:00:01Z"
        }),
    )?;
    let _: UpdateStepResult = dispatch_typed(
        router,
        "desktop.updateStep",
        serde_json::json!({ "action": "beginDownload" }),
    )?;
    let after_fail: UpdateStepResult = dispatch_typed(
        router,
        "desktop.updateStep",
        serde_json::json!({ "action": "downloadFailure", "message": "net::ERR_TIMED_OUT" }),
    )?;
    if after_fail.status != UpdateStatus::Downloaded {
        return Err(format!(
            "BUG-28 regression: expected Downloaded, got {:?}",
            after_fail.status
        ));
    }
    let retained_version = after_fail
        .installable_artifact
        .map(|a| a.version)
        .ok_or("BUG-28 regression: missing installable V1 artifact")?;
    if retained_version != "1.1.0" {
        return Err(format!(
            "BUG-28 regression: expected V1=1.1.0, got {retained_version}"
        ));
    }
    Ok(retained_version)
}

pub(crate) fn verify_sidecar_env_sanitization(controller: &SidecarController) -> bool {
    let sanitized = controller.sanitize_env_entries(
        [
            ("WATCH_REPORT_DEPENDENCIES", "1"),
            ("NODE_WATCH_CHANNEL", "fd3"),
            ("T3CODE_PORT", "9999"),
            (
                "NODE_OPTIONS",
                "--max-old-space-size=4096 --inspect-brk --inspect-port=9229",
            ),
            ("PATH", "/usr/bin"),
        ],
        false,
    );
    sanitized.get("WATCH_REPORT_DEPENDENCIES").is_none()
        && sanitized.get("NODE_WATCH_CHANNEL").is_none()
        && sanitized.get("T3CODE_PORT").is_none()
        && sanitized.get("NODE_OPTIONS") == Some("--max-old-space-size=4096")
        && sanitized.get("T3CODE_HOME").is_some()
}

pub(crate) fn verify_db_preflight(temp_dir: &Path) -> Result<PreflightCheckSummary, String> {
    let missing_db = temp_dir.join("missing.sqlite");
    let clean = run_service_preflight(&missing_db, 4, env!("CARGO_PKG_VERSION"));
    let clean_db_ready = clean.is_ready();
    let old_proto = run_service_preflight(&missing_db, 2, env!("CARGO_PKG_VERSION"));
    let unsupported_protocol_blocked = matches!(old_proto, ServicePreflightResult::Blocked { .. });

    let corrupt_db = temp_dir.join("corrupt.sqlite");
    fs::write(&corrupt_db, b"not a sqlite file").map_err(|err| err.to_string())?;
    let corrupt = run_service_preflight(&corrupt_db, 4, env!("CARGO_PKG_VERSION"));
    let corrupt_db_blocked = matches!(corrupt, ServicePreflightResult::Blocked { .. });

    if !(clean_db_ready && unsupported_protocol_blocked && corrupt_db_blocked) {
        return Err("DB preflight self-check assertion failed".to_owned());
    }

    Ok(PreflightCheckSummary {
        clean_db_ready,
        unsupported_protocol_blocked,
        corrupt_db_blocked,
    })
}

pub(crate) fn verify_service_manifests(
    router: &mut DesktopIpcRouter,
) -> Result<ServiceManifestCheckSummary, String> {
    let base_dir = "/home/t3/.t3";
    let rendered: RenderServiceUnitsResult = dispatch_typed(
        router,
        "desktop.renderServiceUnits",
        serde_json::json!({ "baseDir": base_dir }),
    )?;

    let systemd_roundtrip =
        extract_service_unit_base_dir(&rendered.systemd_unit).as_deref() == Some(base_dir);
    let launchd_roundtrip =
        extract_service_unit_base_dir(&rendered.launchd_plist).as_deref() == Some(base_dir);

    let (manifest, artifacts) =
        DesktopBundleManifest::default_bundle().map_err(|e| e.to_string())?;
    let bom_ok = artifacts
        .windows_task_utf16le_bom
        .starts_with(&[0xff, 0xfe])
        && decode_service_unit_bytes(&artifacts.windows_task_utf16le_bom)
            == artifacts.windows_task_xml
        && extract_service_unit_base_dir(&artifacts.windows_task_xml).as_deref() == Some(base_dir);

    let nightly = render_aur_launcher(DesktopUpdateChannel::Nightly);
    let aur_nightly_isolated = nightly.desktop_entry_id == "com.t3tools.T3CodeNightly.desktop"
        && nightly
            .launcher_script
            .contains("T3CODE_DESKTOP_LAUNCHER_PATH=/usr/bin/t3code-nightly");

    if !(systemd_roundtrip && launchd_roundtrip && bom_ok && aur_nightly_isolated) {
        return Err("boot-service manifest verification failed".to_owned());
    }

    Ok(ServiceManifestCheckSummary {
        systemd_roundtrip,
        launchd_roundtrip,
        windows_task_utf16le_bom: bom_ok,
        aur_nightly_desktop_id: nightly.desktop_entry_id.clone(),
        bundle_file_count: manifest.files.len(),
    })
}
