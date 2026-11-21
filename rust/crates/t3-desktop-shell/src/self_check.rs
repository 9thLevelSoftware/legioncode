//! Built-in `--self-check` suite exercising scheme security, window bounds clamping,
//! `BUG-28` update preservation, `BUG-09` env scrubbing, DB preflight, and boot-service manifests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use t3_surfaces::desktop::WindowBounds;

use crate::control_server::{
    DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION, DesktopControlPlatform, DesktopControlRequest,
    DesktopControlResponse, DesktopControlServer, DesktopControlStatus,
    resolve_desktop_app_control_address, send_loopback_request,
};
use crate::ipc::DesktopIpcRouter;
use crate::self_check_checks::{
    verify_bug28_update_preservation, verify_db_preflight, verify_scheme_routing,
    verify_service_manifests, verify_sidecar_env_sanitization, verify_window_bounds,
};
use crate::sidecar::{SidecarConfig, SidecarController};

static SELF_CHECK_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn create() -> Result<Self, String> {
        let id = SELF_CHECK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("t3-desktop-self-check-{}-{id}", std::process::id()));
        fs::create_dir_all(&path)
            .map_err(|err| format!("failed creating temp dir '{}': {err}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Summary printed by `t3-desktop --self-check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfCheckReport {
    /// Always `true` when every check passes.
    pub passed: bool,
    /// `t3-desktop-shell` package version.
    pub version: String,
    /// Custom scheme traversal & foreign-origin rejection status.
    pub scheme_traversal_rejected: bool,
    /// Clamped window bounds produced during bounds verification.
    pub clamped_bounds: WindowBounds,
    /// Preserved `V1` installer version after failed `V2` update check/download (`BUG-28`).
    pub retained_v1_after_v2_failure: String,
    /// Whether `BUG-09` env scrubbing removed watch/inspector variables.
    pub sidecar_env_sanitized: bool,
    /// DB preflight outcomes (`ready` on empty DB, `blocked` on protocol 2 and corrupt file).
    pub preflight_checks: PreflightCheckSummary,
    /// Rendered boot-service manifest verification across systemd, launchd, and Windows Task XML.
    pub service_manifests: ServiceManifestCheckSummary,
    /// Desktop app activation control server and address derivation verification.
    pub control_server: ControlServerCheckSummary,
}

/// Sub-summary for [`crate::control_server::DesktopControlServer`] self-checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlServerCheckSummary {
    /// Win32 pipe + Linux/Darwin `/tmp` `sun_path` fallback derivation matched `@t3tools/shared`.
    pub address_derivation_ok: bool,
    /// Loopback TCP server validated v1 activation + rejected invalid version requests.
    pub loopback_roundtrip: bool,
    /// Host-native Windows Named Pipe / Unix domain socket accepted and echoed activation.
    pub native_transport_roundtrip: bool,
}

/// Sub-summary for DB preflight self-checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightCheckSummary {
    /// Empty DB at protocol `4` passed as `ready`.
    pub clean_db_ready: bool,
    /// Unsupported protocol `2` returned `blocked`.
    pub unsupported_protocol_blocked: bool,
    /// Corrupt SQLite bytes returned `blocked`.
    pub corrupt_db_blocked: bool,
}

/// Sub-summary for boot-service and AUR manifest self-checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceManifestCheckSummary {
    /// Extracted `T3CODE_HOME` from `t3code.service` matched input `base_dir`.
    pub systemd_roundtrip: bool,
    /// Extracted `T3CODE_HOME` from `com.t3tools.t3code.service.plist` matched input `base_dir`.
    pub launchd_roundtrip: bool,
    /// `t3code-task.xml` starts with `0xFF 0xFE` BOM and roundtrips `base_dir`.
    pub windows_task_utf16le_bom: bool,
    /// Nightly AUR desktop entry ID (`com.t3tools.T3CodeNightly.desktop`) verifying `BUG-31`.
    pub aur_nightly_desktop_id: String,
    /// Total files described by `DesktopBundleManifest`.
    pub bundle_file_count: usize,
}

/// Runs all self-check verifications cleanly and returns [`SelfCheckReport`].
pub fn run_self_check() -> Result<SelfCheckReport, String> {
    let temp = TempDirGuard::create()?;
    let controller = SidecarController::new(SidecarConfig {
        base_dir: temp.path().to_path_buf(),
        ..SidecarConfig::default()
    });
    let mut router = DesktopIpcRouter::new(controller.clone());

    let scheme_traversal_rejected = verify_scheme_routing(&mut router)?;
    let clamped_bounds = verify_window_bounds(&mut router)?;
    let retained_v1_after_v2_failure = verify_bug28_update_preservation(&mut router)?;
    let sidecar_env_sanitized = verify_sidecar_env_sanitization(&controller);
    let preflight_checks = verify_db_preflight(temp.path())?;
    let service_manifests = verify_service_manifests(&mut router)?;
    let control_server = verify_control_server_roundtrip(temp.path())?;

    Ok(SelfCheckReport {
        passed: true,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        scheme_traversal_rejected,
        clamped_bounds,
        retained_v1_after_v2_failure,
        sidecar_env_sanitized,
        preflight_checks,
        service_manifests,
        control_server,
    })
}

fn verify_control_server_roundtrip(state_dir: &Path) -> Result<ControlServerCheckSummary, String> {
    let win_addr = resolve_desktop_app_control_address(
        "/tmp/t3-state",
        DesktopControlPlatform::Win32,
        "/tmp",
        None,
    );
    let long_tmp = "/tmp/extremely-long-temp-directory-prefix-that-exceeds-sun-path-limits-across-unix-kernels";
    let linux_addr = resolve_desktop_app_control_address(
        "/tmp/t3-state",
        DesktopControlPlatform::Linux,
        long_tmp,
        Some(1000),
    );
    let darwin_addr = resolve_desktop_app_control_address(
        "/tmp/t3-state",
        DesktopControlPlatform::Darwin,
        long_tmp,
        Some(1000),
    );
    if !win_addr.address.starts_with(r"\\.\pipe\t3code-app-")
        || !linux_addr.address.starts_with("/tmp/t3code-1000/")
        || !darwin_addr.address.starts_with("/tmp/t3code-1000/")
    {
        return Err("desktop control address derivation failed parity check".to_owned());
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("failed creating tokio runtime for control check: {err}"))?;

    rt.block_on(async {
        let handler = Arc::new(|req: DesktopControlRequest| {
            DesktopControlResponse::activated(req.request_id, req.workspace_root)
        });
        let server = DesktopControlServer::bind_loopback(handler.clone())
            .await
            .map_err(|err| format!("loopback bind failed: {err}"))?;
        let addr = server
            .local_addr()
            .ok_or_else(|| "loopback server missing socket addr".to_owned())?;
        let ok_resp = send_loopback_request(
            addr,
            &DesktopControlRequest {
                version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
                request_id: "self-check-1".to_owned(),
                workspace_root: state_dir.display().to_string(),
                request_type: Some("open-workspace".to_owned()),
                platform: None,
            },
        )
        .await
        .map_err(|err| format!("loopback request failed: {err}"))?;
        let bad_resp = send_loopback_request(
            addr,
            &DesktopControlRequest {
                version: 99,
                request_id: "self-check-bad".to_owned(),
                workspace_root: state_dir.display().to_string(),
                request_type: None,
                platform: None,
            },
        )
        .await
        .map_err(|err| format!("invalid-version request failed: {err}"))?;
        server.shutdown().await;
        if ok_resp.status != DesktopControlStatus::Activated
            || bad_resp.status != DesktopControlStatus::Error
        {
            return Err("loopback control server roundtrip failed validation".to_owned());
        }

        let native_server = DesktopControlServer::bind_for_state_dir(state_dir, handler)
            .map_err(|err| format!("native control bind failed: {err}"))?;
        let req = DesktopControlRequest {
            version: DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION,
            request_id: "self-check-native".to_owned(),
            workspace_root: state_dir.display().to_string(),
            request_type: Some("open-workspace".to_owned()),
            platform: None,
        };
        #[cfg(windows)]
        let native_resp =
            crate::control_server::send_named_pipe_request(native_server.address(), &req)
                .await
                .map_err(|err| format!("named pipe request failed: {err}"))?;
        #[cfg(unix)]
        let native_resp = crate::control_server::send_unix_request(native_server.address(), &req)
            .await
            .map_err(|err| format!("unix socket request failed: {err}"))?;
        native_server.shutdown().await;
        if native_resp.status != DesktopControlStatus::Activated || !native_resp.ok {
            return Err("native control server failed to return activated".to_owned());
        }

        Ok(ControlServerCheckSummary {
            address_derivation_ok: true,
            loopback_roundtrip: true,
            native_transport_roundtrip: true,
        })
    })
}
