//! Helper lifecycle utilities for [`crate::window_host`]: isolated temp data directories,
//! smoke watchdog timer, background activation control server thread, and report builder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use legion_surfaces::desktop::WindowBounds;
use tao::event_loop::EventLoopProxy;

use crate::control_server::{ActivationHandler, DesktopControlResponse, DesktopControlServer};
use crate::window_host_protocol::CustomProtocolMetrics;
use crate::window_host_types::{
    DesktopExitReason, DesktopHostError, DesktopLaunchConfig, DesktopRunReport, DesktopUserEvent,
    WebviewSmokeChecks,
};

static WEB_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
pub(crate) struct SmokeSignalState {
    pub js_smoke_ok: bool,
    pub js_traversal_rejected: bool,
    pub smoke_completed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct RuntimeTracker {
    pub protocol_metrics: CustomProtocolMetrics,
    pub webview_loaded: bool,
    pub smoke_timed_out: bool,
    pub smoke_signal: SmokeSignalState,
    pub ipc_round_trips: usize,
    pub js_fetch_traversal_status: u16,
    pub activated_workspaces: Vec<String>,
}

pub(crate) struct SmokeTempDir {
    pub path: PathBuf,
    cleanup: bool,
}

impl SmokeTempDir {
    pub fn new(explicit: Option<&Path>, is_smoke: bool) -> Result<Self, DesktopHostError> {
        if let Some(dir) = explicit {
            fs::create_dir_all(dir).map_err(|source| DesktopHostError::TempDirIo {
                path: dir.to_path_buf(),
                source,
            })?;
            return Ok(Self {
                path: dir.to_path_buf(),
                cleanup: false,
            });
        }
        let seq = WEB_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = if is_smoke {
            "legioncode-desktop-webview-smoke"
        } else {
            "legioncode-desktop-webview"
        };
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{seq}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|source| DesktopHostError::TempDirIo {
            path: dir.clone(),
            source,
        })?;
        Ok(Self {
            path: dir,
            cleanup: is_smoke,
        })
    }
}

impl Drop for SmokeTempDir {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub(crate) fn spawn_smoke_watchdog(
    timeout_ms: u64,
    proxy: EventLoopProxy<DesktopUserEvent>,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        if matches!(
            rx.recv_timeout(Duration::from_millis(timeout_ms)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let _ = proxy.send_event(DesktopUserEvent::SmokeTimeout);
        }
    });
    (tx, handle)
}

pub(crate) fn spawn_control_server_thread(
    base_dir: &Path,
    proxy: EventLoopProxy<DesktopUserEvent>,
) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let state_dir = base_dir.join("userdata");
    let handle = std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let handler: ActivationHandler = Arc::new(move |req| {
                let req_id = req.request_id.clone();
                let workspace = req.workspace_root.clone();
                let _ = proxy.send_event(DesktopUserEvent::ActivateWorkspace {
                    request_id: req_id.clone(),
                    workspace_root: workspace.clone(),
                });
                DesktopControlResponse::activated(req_id, workspace)
            });
            let Ok(server) = DesktopControlServer::bind_for_state_dir(&state_dir, handler) else {
                return;
            };
            while matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            server.shutdown().await;
        });
    });
    (tx, handle)
}

pub(crate) fn finalize_report(
    config: &DesktopLaunchConfig,
    clamped_bounds: WindowBounds,
    snapshot: &RuntimeTracker,
) -> DesktopRunReport {
    let traversal_rejected = snapshot.protocol_metrics.traversal_rejected
        || snapshot.smoke_signal.js_traversal_rejected
        || snapshot.js_fetch_traversal_status == 404;

    let checks = WebviewSmokeChecks {
        webview_loaded: snapshot.webview_loaded,
        custom_protocol_served: snapshot.protocol_metrics.served_ok,
        scheme_traversal_rejected_in_webview: traversal_rejected,
    };

    if !config.headless_smoke {
        return DesktopRunReport {
            passed: true,
            checks,
            ipc_round_trips: snapshot.ipc_round_trips,
            clamped_bounds,
            exit_reason: DesktopExitReason::WindowClosed,
            activated_workspaces: snapshot.activated_workspaces.clone(),
            failure_reason: None,
        };
    }

    let smoke_ok = !snapshot.smoke_timed_out
        && snapshot.smoke_signal.smoke_completed
        && snapshot.smoke_signal.js_smoke_ok
        && checks.webview_loaded
        && checks.custom_protocol_served
        && checks.scheme_traversal_rejected_in_webview
        && snapshot.ipc_round_trips >= 3;

    let failure_reason = if smoke_ok {
        None
    } else if snapshot.smoke_timed_out {
        Some(format!(
            "headless webview smoke timed out after {}ms (loaded={}, served={}, ipc={})",
            config.smoke_timeout_ms,
            checks.webview_loaded,
            checks.custom_protocol_served,
            snapshot.ipc_round_trips
        ))
    } else {
        Some(format!(
            "headless webview smoke incomplete (completed={}, js_ok={}, loaded={}, served={}, traversal={}, ipc={})",
            snapshot.smoke_signal.smoke_completed,
            snapshot.smoke_signal.js_smoke_ok,
            checks.webview_loaded,
            checks.custom_protocol_served,
            checks.scheme_traversal_rejected_in_webview,
            snapshot.ipc_round_trips
        ))
    };

    DesktopRunReport {
        passed: smoke_ok,
        checks,
        ipc_round_trips: snapshot.ipc_round_trips,
        clamped_bounds,
        exit_reason: if snapshot.smoke_timed_out {
            DesktopExitReason::SmokeTimeout
        } else {
            DesktopExitReason::SmokeCompleted
        },
        activated_workspaces: snapshot.activated_workspaces.clone(),
        failure_reason,
    }
}
