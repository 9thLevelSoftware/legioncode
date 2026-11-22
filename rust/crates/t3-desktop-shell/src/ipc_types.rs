//! Core JSON-RPC envelope, bootstrap, scheme, and window-bounds types for [`crate::ipc::DesktopIpcRouter`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use t3_contracts::policy::ManagedPolicyDocument;
use t3_edge::desktop_updates::{DesktopUpdateChannel, TrackMismatch, UpdateStatus};
use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::service_preflight::ServicePreflightResult;
use t3_surfaces::desktop::{DesktopRouteDecision, DisplayRect, InitialWindowBounds, WindowBounds};

/// Known Tauri v2 IPC method names routed by [`crate::ipc::DesktopIpcRouter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DesktopIpcMethod {
    /// `desktop.bootstrap`
    #[serde(rename = "desktop.bootstrap")]
    Bootstrap,
    /// `desktop.resolveScheme`
    #[serde(rename = "desktop.resolveScheme")]
    ResolveScheme,
    /// `desktop.clampWindowBounds`
    #[serde(rename = "desktop.clampWindowBounds")]
    ClampWindowBounds,
    /// `desktop.updateStep`
    #[serde(rename = "desktop.updateStep")]
    UpdateStep,
    /// `desktop.preflightDb`
    #[serde(rename = "desktop.preflightDb")]
    PreflightDb,
    /// `desktop.renderServiceUnits`
    #[serde(rename = "desktop.renderServiceUnits")]
    RenderServiceUnits,
    /// `desktop.appleSpeech.available` (`#34027`)
    #[serde(rename = "desktop.appleSpeech.available")]
    AppleSpeechAvailable,
    /// `desktop.appleSpeech.supportedLocales` (`#34027`)
    #[serde(rename = "desktop.appleSpeech.supportedLocales")]
    AppleSpeechSupportedLocales,
    /// `desktop.appleSpeech.prepare` (`#34027`)
    #[serde(rename = "desktop.appleSpeech.prepare")]
    AppleSpeechPrepare,
    /// `desktop.appleSpeech.transcribe` (`#34027`)
    #[serde(rename = "desktop.appleSpeech.transcribe")]
    AppleSpeechTranscribe,
}

impl DesktopIpcMethod {
    /// Parses a wire method name into a [`DesktopIpcMethod`].
    pub fn parse(method: &str) -> Option<Self> {
        match method {
            "desktop.bootstrap" => Some(Self::Bootstrap),
            "desktop.resolveScheme" => Some(Self::ResolveScheme),
            "desktop.clampWindowBounds" => Some(Self::ClampWindowBounds),
            "desktop.updateStep" => Some(Self::UpdateStep),
            "desktop.preflightDb" => Some(Self::PreflightDb),
            "desktop.renderServiceUnits" => Some(Self::RenderServiceUnits),
            "desktop.appleSpeech.available" => Some(Self::AppleSpeechAvailable),
            "desktop.appleSpeech.supportedLocales" => Some(Self::AppleSpeechSupportedLocales),
            "desktop.appleSpeech.prepare" => Some(Self::AppleSpeechPrepare),
            "desktop.appleSpeech.transcribe" => Some(Self::AppleSpeechTranscribe),
            _ => None,
        }
    }

    /// Canonical wire method name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrap => "desktop.bootstrap",
            Self::ResolveScheme => "desktop.resolveScheme",
            Self::ClampWindowBounds => "desktop.clampWindowBounds",
            Self::UpdateStep => "desktop.updateStep",
            Self::PreflightDb => "desktop.preflightDb",
            Self::RenderServiceUnits => "desktop.renderServiceUnits",
            Self::AppleSpeechAvailable => "desktop.appleSpeech.available",
            Self::AppleSpeechSupportedLocales => "desktop.appleSpeech.supportedLocales",
            Self::AppleSpeechPrepare => "desktop.appleSpeech.prepare",
            Self::AppleSpeechTranscribe => "desktop.appleSpeech.transcribe",
        }
    }
}

/// Incoming Tauri v2 IPC JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIpcRequest {
    /// Optional caller correlation ID echoed back in [`DesktopIpcResponse::id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Target IPC method (e.g. `"desktop.bootstrap"`).
    #[serde(alias = "command", alias = "tag")]
    pub method: String,
    /// Method-specific JSON payload (defaults to `null` / empty object).
    #[serde(default, alias = "payload", alias = "args")]
    pub params: Value,
}

/// Machine-readable IPC error payload returned inside [`DesktopIpcResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIpcErrorPayload {
    /// Short stable error code (e.g. `"UnknownMethod"`, `"InvalidParams"`).
    pub code: String,
    /// Human-readable error explanation.
    pub message: String,
}

/// Outgoing Tauri v2 IPC JSON envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIpcResponse {
    /// Echoed correlation ID from [`DesktopIpcRequest::id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// `true` when dispatch succeeded and `result` is populated.
    pub ok: bool,
    /// Serialized response object when `ok == true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error details when `ok == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DesktopIpcErrorPayload>,
}

/// Desktop branding metadata matching `resolveDesktopAppBranding` (`DesktopEnvironment.ts:101`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAppBranding {
    /// Always `"T3 Code"`.
    pub base_name: String,
    /// `"Dev"`, `"Nightly"`, or `"Beta"`.
    pub stage_label: String,
    /// Formatted window title (`"T3 Code (Beta)"`).
    pub display_name: String,
}

/// Parameters for `desktop.bootstrap`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BootstrapParams {
    /// Override running desktop version (defaults to sidecar/crate version).
    pub app_version: Option<String>,
    /// Override development mode flag (`t3code-dev` vs `t3code`).
    pub is_development: Option<bool>,
    /// Optional Clerk frontend API hostname appended to CSP `script-src`.
    pub clerk_frontend_api_hostname: Option<String>,
    /// Optional T3 home directory override.
    pub base_dir: Option<String>,
    /// Whether `base_dir` was explicitly passed via `--base-dir` / `T3CODE_HOME`.
    pub base_dir_is_explicit: Option<bool>,
    /// Launcher protocol version override (defaults to `4`).
    pub launcher_protocol: Option<i64>,
    /// Persisted window bounds loaded from desktop settings.
    pub persisted_bounds: Option<WindowBounds>,
    /// Connected display work areas.
    pub displays: Option<Vec<DisplayRect>>,
    /// Update release track override.
    pub update_channel: Option<DesktopUpdateChannel>,
    /// Optional administrator managed policy document.
    pub managed_policy: Option<ManagedPolicyDocument>,
}

/// Result returned by `desktop.bootstrap`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapResult {
    /// Shared surface protocol version (`t3_surfaces::SURFACE_PROTOCOL_VERSION`).
    pub surface_protocol_version: u32,
    /// Bundle application identifier (`"com.t3tools.t3code"` or `.dev`).
    pub identifier: String,
    /// Active custom protocol scheme (`"t3code"` or `"t3code-dev"`).
    pub scheme: String,
    /// Entry URL (`"t3code://app/"` or `"t3code-dev://app/"`).
    pub desktop_url: String,
    /// Generated `Content-Security-Policy` header string.
    pub content_security_policy: String,
    /// Application branding strings.
    pub branding: DesktopAppBranding,
    /// Initial window geometry decision (`Restored` or `DefaultSize`).
    pub initial_window_bounds: InitialWindowBounds,
    /// Clamped bounds when `persisted_bounds` and `displays` were supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clamped_window_bounds: Option<WindowBounds>,
    /// Derived server paths for the active home/mode.
    pub paths: ServerDerivedPaths,
    /// Pre-start DB and launcher protocol compatibility check.
    pub preflight: ServicePreflightResult,
    /// Whether automatic updates are permitted by managed policy.
    pub updates_enabled: bool,
    /// Current desktop updater status.
    pub update_status: UpdateStatus,
    /// Active desktop update channel.
    pub update_channel: DesktopUpdateChannel,
    /// Active cross-track mismatch surfaced by updater checks (`#38506`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_mismatch: Option<TrackMismatch>,
    /// Discovered sidecar binary path if found on disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecar_binary: Option<String>,
    /// Launch arguments prepared for the sidecar binary.
    pub sidecar_launch_args: Vec<String>,
    /// Number of sanitized environment variables prepared for the sidecar (`BUG-09`).
    pub sidecar_sanitized_env_count: usize,
}

/// Parameters for `desktop.resolveScheme`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveSchemeParams {
    /// HTTP method (`"GET"` or `"HEAD"`, defaults to `"GET"`).
    #[serde(default)]
    pub method: Option<String>,
    /// Request URL such as `"t3code://app/assets/main.js"`.
    pub url: String,
    /// Optional `Accept` header value.
    #[serde(default, alias = "accept")]
    pub accept_header: Option<String>,
    /// Optional custom scheme override (`"t3code"` or `"t3code-dev"`).
    #[serde(default)]
    pub scheme: Option<String>,
    /// Selects `"t3code-dev"` when `scheme` is omitted and `true`.
    #[serde(default)]
    pub is_development: Option<bool>,
    /// Optional Clerk frontend API hostname appended to CSP `script-src`.
    #[serde(default)]
    pub clerk_frontend_api_hostname: Option<String>,
    /// Optional bundled asset catalog.
    #[serde(default)]
    pub assets: Option<Vec<String>>,
}

/// Result returned by `desktop.resolveScheme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveSchemeResult {
    /// HTTP status code (`200`, `400`, `404`, or `405`).
    pub status: u16,
    /// `true` when `status == 200`.
    pub allowed: bool,
    /// Detailed route decision from `t3_surfaces::desktop::DesktopSchemeRouter`.
    pub decision: DesktopRouteDecision,
}

/// Parameters for `desktop.clampWindowBounds`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClampWindowBoundsParams {
    /// Active or candidate bounds to clamp onto `displays`.
    #[serde(default)]
    pub bounds: Option<WindowBounds>,
    /// Persisted bounds checked by `WindowBounds::resolve_initial`.
    #[serde(default)]
    pub persisted_bounds: Option<WindowBounds>,
    /// Optional normal (un-maximized) bounds normalized onto the window's display.
    #[serde(default)]
    pub normal_bounds: Option<WindowBounds>,
    /// Connected display rectangles.
    #[serde(default)]
    pub displays: Vec<DisplayRect>,
}

/// Result returned by `desktop.clampWindowBounds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClampWindowBoundsResult {
    /// Sanitized and display-clamped bounds.
    pub clamped: WindowBounds,
    /// Initial window startup bounds (`Restored` or `DefaultSize`).
    pub initial: InitialWindowBounds,
    /// `true` when the input bounds fit inside at least one connected display.
    pub fits_within_any_display: bool,
    /// `true` when the input bounds meet `400x360` minimum dimensions.
    pub meets_minimum_size: bool,
    /// Normalized normal bounds when `normal_bounds` was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_on_window_display: Option<WindowBounds>,
}
