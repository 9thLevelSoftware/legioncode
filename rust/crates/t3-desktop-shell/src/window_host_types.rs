//! Configuration, report, and error types for [`crate::window_host::launch_desktop_window`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use t3_surfaces::desktop::{
    DEFAULT_MAIN_WINDOW_HEIGHT, DEFAULT_MAIN_WINDOW_WIDTH, DisplayRect, WindowBounds,
};
use thiserror::Error;

use crate::sidecar::SidecarConfig;
use crate::window_host_protocol::desktop_scheme_name;

/// Custom events delivered into the `tao` main event loop via `EventLoopProxy`.
#[derive(Debug, Clone)]
pub enum DesktopUserEvent {
    /// Serialized [`crate::ipc::DesktopIpcResponse`] dispatched to `window.__t3ReceiveIpc`.
    IpcReply(String),
    /// Workspace activation received from [`crate::control_server::DesktopControlServer`].
    ActivateWorkspace {
        /// Request correlation identifier.
        request_id: String,
        /// Workspace directory path.
        workspace_root: String,
    },
    /// Headless smoke verification completed inside the live `WebView`.
    SmokeComplete,
    /// Headless smoke watchdog timer fired before JS signaled completion.
    SmokeTimeout,
}

/// Errors emitted while creating or running the native `tao` + `wry` desktop window.
#[derive(Debug, Error)]
pub enum DesktopHostError {
    /// Native window creation failed.
    #[error("failed creating desktop window: {0}")]
    WindowCreate(String),
    /// Native `WebView` (`wry` / `WebView2`) creation failed.
    #[error("failed creating desktop webview: {0}")]
    WebViewCreate(String),
    /// Creating an isolated `WebView` data directory failed.
    #[error("failed preparing isolated webview data directory '{}': {source}", path.display())]
    TempDirIo {
        /// Directory path.
        path: PathBuf,
        /// Underlying OS I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Configuration for [`crate::window_host::launch_desktop_window`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopLaunchConfig {
    /// Window title (`"T3 Code"`).
    pub title: String,
    /// Initial URL loaded in the `WebView` (`"t3code://app/"` or `http://127.0.0.1:...`).
    pub initial_url: String,
    /// Optional directory serving static assets on `t3code://app/*`.
    pub static_dir: Option<PathBuf>,
    /// T3 home directory passed to [`crate::sidecar::SidecarController`].
    pub base_dir: PathBuf,
    /// Optional isolated `WebView` user-data directory (auto-temp in smoke mode).
    pub web_data_dir: Option<PathBuf>,
    /// Requested window bounds (clamped against connected displays before window creation).
    pub bounds: WindowBounds,
    /// Fallback display rects when `available_monitors()` yields none.
    pub displays: Vec<DisplayRect>,
    /// When `true`, hides window, loads smoke HTML, verifies live IPC + traversal, and exits.
    pub headless_smoke: bool,
    /// Configures `with_any_thread(true)` on supported `tao` event loops (Windows / Linux).
    pub allow_any_thread: bool,
    /// Watchdog timeout in milliseconds for `headless_smoke` mode (`10_000` default).
    pub smoke_timeout_ms: u64,
    /// Application version forwarded to [`crate::sidecar::SidecarController`].
    pub app_version: String,
    /// Optional Clerk frontend API hostname appended to CSP `script-src`.
    pub clerk_frontend_api_hostname: Option<String>,
    /// Whether to start [`crate::control_server::DesktopControlServer`] alongside the window.
    pub enable_control_server: bool,
}

impl Default for DesktopLaunchConfig {
    fn default() -> Self {
        Self {
            title: "T3 Code".to_owned(),
            initial_url: format!("{}://app/", desktop_scheme_name()),
            static_dir: None,
            base_dir: SidecarConfig::default().base_dir,
            web_data_dir: None,
            bounds: WindowBounds {
                x: 80,
                y: 80,
                width: DEFAULT_MAIN_WINDOW_WIDTH,
                height: DEFAULT_MAIN_WINDOW_HEIGHT,
            },
            displays: vec![DisplayRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
            headless_smoke: false,
            allow_any_thread: false,
            smoke_timeout_ms: 10_000,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            clerk_frontend_api_hostname: None,
            enable_control_server: false,
        }
    }
}

/// Terminal reason explaining why the desktop event loop exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopExitReason {
    /// Headless smoke completed all JS/IPC assertions cleanly.
    SmokeCompleted,
    /// Headless smoke hit the watchdog timeout before completion.
    SmokeTimeout,
    /// Interactive window closed by user or OS.
    WindowClosed,
}

/// `WebView` smoke verification flags serialized flat inside [`DesktopRunReport`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebviewSmokeChecks {
    /// `true` once the `WebView` completed initial navigation (`PageLoadEvent::Finished`).
    pub webview_loaded: bool,
    /// `true` once `t3code://app` served a `200` document/asset response.
    pub custom_protocol_served: bool,
    /// `true` once traversal/foreign origin returned `404` inside `WebView` / IPC checks.
    pub scheme_traversal_rejected_in_webview: bool,
}

/// Result returned by [`crate::window_host::launch_desktop_window`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRunReport {
    /// `true` when interactive launch exited cleanly or all headless smoke checks passed.
    pub passed: bool,
    /// `WebView` page load, custom protocol, and traversal checks.
    #[serde(flatten)]
    pub checks: WebviewSmokeChecks,
    /// Number of `window.ipc.postMessage` round-trips dispatched via [`crate::ipc::DesktopIpcRouter`].
    pub ipc_round_trips: usize,
    /// Display-clamped window bounds applied at launch.
    pub clamped_bounds: WindowBounds,
    /// Reason the `tao` event loop exited.
    pub exit_reason: DesktopExitReason,
    /// Workspace roots activated via [`crate::control_server::DesktopControlServer`].
    pub activated_workspaces: Vec<String>,
    /// Diagnostic failure detail when `passed == false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}
