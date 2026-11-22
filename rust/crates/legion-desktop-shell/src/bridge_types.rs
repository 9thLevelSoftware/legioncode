//! Wire-compatible data payloads and runtime host state for `window.desktopBridge`
//! matching `packages/contracts/src/ipc.ts`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use legion_edge::desktop_updates::{DesktopUpdateChannel, UpdateErrorContext, UpdateStatus};
use serde::{Deserialize, Serialize};

use crate::ipc_types::DesktopAppBranding;

/// Local environment bootstrap descriptor returned by `desktopBridge.getLocalEnvironmentBootstraps()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopEnvironmentBootstrap {
    /// Stable local backend identity (`"primary"`).
    pub id: String,
    /// Human-readable backend label (`"Local"`).
    pub label: String,
    /// Active WSL distro when running under WSL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_distro: Option<String>,
    /// Loopback HTTP base URL (e.g. `http://127.0.0.1:41234`).
    pub http_base_url: Option<String>,
    /// Loopback WebSocket base URL (e.g. `ws://127.0.0.1:41234/ws`).
    pub ws_base_url: Option<String>,
    /// Single-use or cached bootstrap credential for automatic authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_token: Option<String>,
}

/// Pre-hydrated synchronous values baked into the `window.desktopBridge` init script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBridgeInitConfig {
    /// Application branding returned by `getAppBranding()`.
    pub branding: DesktopAppBranding,
    /// Relay telemetry flag (`getRelayTelemetryEnabled()`).
    pub relay_telemetry_enabled: Option<bool>,
    /// Platform tag (`"win32" | "darwin" | "linux"`) returned by `getClientPlatform()`.
    pub client_platform: String,
    /// OS primary-selection middle-click paste enabled flag (`getPrimaryPasteEnabled()`).
    pub primary_paste_enabled: bool,
    /// Primary BCP-47 OS locale (`getSystemLocale()`).
    pub system_locale: Option<String>,
    /// Ordered BCP-47 OS languages (`getPreferredSystemLanguages()`).
    pub preferred_system_languages: Vec<String>,
    /// Local backend bootstrap targets (`getLocalEnvironmentBootstraps()`).
    pub local_environment_bootstraps: Vec<DesktopEnvironmentBootstrap>,
    /// Whether the bundled local backend is enabled (`getLocalEnvironmentEnabled()`).
    pub local_environment_enabled: bool,
    /// Initial fullscreen state (`getWindowFullscreenState()`).
    pub window_fullscreen_state: bool,
    /// Initial webview zoom factor (`getWindowZoomFactor()`).
    pub window_zoom_factor: f64,
}

/// Mutable desktop bridge state carried inside [`crate::ipc::DesktopIpcRouter`].
#[derive(Debug, Clone)]
pub struct DesktopBridgeRuntimeState {
    /// Directory where `client-settings.json` and `connection-catalog.json` reside (`<legionHome>/userdata`).
    pub userdata_dir: PathBuf,
    /// SQLite database path (`<userdata_dir>/statev2.sqlite`) for minting pairing tokens.
    pub db_path: PathBuf,
    /// Whether the bundled local environment is enabled.
    pub local_environment_enabled: bool,
    /// Active local backend HTTP base URL (e.g. `http://127.0.0.1:<port>`).
    pub local_http_base_url: Option<String>,
    /// Active local backend WS base URL (e.g. `ws://127.0.0.1:<port>/ws`).
    pub local_ws_base_url: Option<String>,
    /// Bootstrap pairing token hydrated for the local backend.
    pub local_bootstrap_token: Option<String>,
    /// Cached bearer token returned by `getLocalEnvironmentBearerToken()`.
    pub cached_bearer_token: Option<String>,
    /// Server exposure mode (`"local-only"` or `"network-accessible"`).
    pub exposure_mode: String,
    /// Whether Tailscale Serve is enabled.
    pub tailscale_serve_enabled: bool,
    /// Port used by Tailscale Serve (`443` default).
    pub tailscale_serve_port: u16,
    /// Active WSL configuration state.
    pub wsl_state: DesktopWslStatePayload,
    /// Last applied desktop theme (`"system"`, `"light"`, or `"dark"`).
    pub theme: String,
    /// Last set notification badge count.
    pub notification_badge_count: u32,
    /// Readiness and shortcut-suppression flags.
    pub readiness: DesktopBridgeReadiness,
    /// Incremented whenever renderer invokes `revealWindow()`.
    pub reveal_requests: u64,
    /// Incremented whenever renderer invokes `pasteAsText()`.
    pub paste_as_text_requests: u64,
    /// Preview tab zoom/mute/color-scheme metadata cache.
    pub preview_tabs: BTreeMap<String, serde_json::Value>,
    /// Resolved host arch string (`"x64"`, `"arm64"`, or `"other"`).
    pub host_arch: String,
}

/// Renderer readiness and shortcut-suppression flags tracked across `window.desktopBridge`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DesktopBridgeReadiness {
    /// Whether `snapShot` global shortcut is suppressed while editing shortcuts.
    pub shortcut_suppressed: bool,
    /// Tracks renderer `webLinks.setReady` readiness.
    pub web_links_ready: bool,
    /// Tracks renderer `appActivation.setReady` readiness.
    pub app_activation_ready: bool,
}

impl DesktopBridgeRuntimeState {
    /// Initializes runtime bridge state anchored under `<base_dir>/userdata`.
    #[must_use]
    pub fn from_base_dir(base_dir: &std::path::Path) -> Self {
        let userdata_dir = base_dir.join("userdata");
        let db_path = userdata_dir.join("statev2.sqlite");
        Self {
            userdata_dir,
            db_path,
            local_environment_enabled: true,
            local_http_base_url: None,
            local_ws_base_url: None,
            local_bootstrap_token: None,
            cached_bearer_token: None,
            exposure_mode: "local-only".to_owned(),
            tailscale_serve_enabled: false,
            tailscale_serve_port: 443,
            wsl_state: DesktopWslStatePayload::default(),
            theme: "system".to_owned(),
            notification_badge_count: 0,
            readiness: DesktopBridgeReadiness::default(),
            reveal_requests: 0,
            paste_as_text_requests: 0,
            preview_tabs: BTreeMap::new(),
            host_arch: current_runtime_arch().to_owned(),
        }
    }
}

/// Returns `"arm64"`, `"x64"`, or `"other"` matching `DesktopRuntimeArchSchema`.
#[must_use]
pub const fn current_runtime_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x64"
    } else {
        "other"
    }
}

/// Release note block serialized inside [`DesktopUpdateStatePayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateReleaseNotePayload {
    /// Release version string.
    pub version: String,
    /// Formatted bullet note lines.
    pub items: Vec<String>,
    /// Total item count before truncation.
    pub total_items: usize,
}

/// Full updater payload matching `DesktopUpdateStateSchema` (`ipc.ts:320-339`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateStatePayload {
    /// Whether automatic updates are enabled.
    pub enabled: bool,
    /// Lifecycle status (`disabled`, `idle`, `checking`, etc.).
    pub status: UpdateStatus,
    /// Active release channel (`latest` or `nightly`).
    pub channel: DesktopUpdateChannel,
    /// Currently running app version.
    pub current_version: String,
    /// Host CPU architecture (`x64`, `arm64`, `other`).
    pub host_arch: String,
    /// Application binary architecture.
    pub app_arch: String,
    /// `true` when running under Rosetta / `WoW64` emulation.
    pub running_under_arm64_translation: bool,
    /// Candidate version discovered on feed if any.
    pub available_version: Option<String>,
    /// Verified artifact version retained on disk if any.
    pub downloaded_version: Option<String>,
    /// Structured release notes.
    pub release_notes: Vec<DesktopUpdateReleaseNotePayload>,
    /// Count of omitted older releases.
    pub omitted_release_count: usize,
    /// Download percentage `0..=100` while downloading.
    pub download_percent: Option<u8>,
    /// ISO-8601 timestamp of the most recent update check.
    pub checked_at: Option<String>,
    /// Error or status message.
    pub message: Option<String>,
    /// Phase of the latest error (`check`, `download`, or `install`).
    pub error_context: Option<UpdateErrorContext>,
    /// `true` when retry is actionable.
    pub can_retry: bool,
}

/// WSL configuration state matching `DesktopWslStateSchema` (`ipc.ts:579-586`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWslStatePayload {
    /// Whether the WSL backend is enabled.
    pub enabled: bool,
    /// Explicit WSL distro or `null` for default.
    pub distro: Option<String>,
    /// Whether WSL is available on this host.
    pub available: bool,
    /// Run WSL backend only without native Windows backend.
    pub wsl_only: bool,
    /// Installed WSL distros.
    pub distros: Vec<DesktopWslDistroPayload>,
    /// Preflight warning/error text if any.
    pub preflight_error: Option<String>,
}

/// WSL distribution descriptor matching `DesktopWslDistroSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWslDistroPayload {
    /// Distro name.
    pub name: String,
    /// `true` for the default WSL distro.
    pub is_default: bool,
    /// WSL architecture version (`1` or `2`).
    pub version: u8,
}

/// Server exposure configuration matching `DesktopServerExposureStateSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopServerExposureStatePayload {
    /// `"local-only"` or `"network-accessible"`.
    pub mode: String,
    /// Public/LAN HTTP endpoint when accessible.
    pub endpoint_url: Option<String>,
    /// Advertised LAN or Tailscale hostname.
    pub advertised_host: Option<String>,
    /// Whether Tailscale Serve proxying is enabled.
    pub tailscale_serve_enabled: bool,
    /// Port exposed by Tailscale Serve.
    pub tailscale_serve_port: u16,
}

/// CLI command installation state matching `DesktopCliCommandStateSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCliCommandStatePayload {
    /// Whether CLI command management is supported on this host.
    pub supported: bool,
    /// Installed `legioncode` symlink/shim path if present.
    pub installed_path: Option<String>,
    /// Whether `legioncode` resolves cleanly on `PATH`.
    pub on_path: bool,
    /// Bundled CLI binary path managed by the desktop host.
    pub launcher_path: Option<String>,
}
