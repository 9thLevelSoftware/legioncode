//! `window.desktopBridge` `WebView` initialization script generator (`with_initialization_script`).

use crate::bridge_types::{DesktopBridgeInitConfig, DesktopEnvironmentBootstrap};
use crate::ipc_handlers::resolve_branding;
use crate::native_dialogs::{client_platform, detect_preferred_system_languages};

const PRELOAD_BRIDGE_TEMPLATE: &str = include_str!("preload_bridge.js");
const INIT_PLACEHOLDER: &str = "__LEGION_DESKTOP_BRIDGE_INIT__";

/// Constructs default hydrated host metadata for [`render_desktop_bridge_init_script`].
#[must_use]
pub fn build_bridge_init_config(
    is_development: bool,
    app_version: &str,
    local_http_base_url: Option<String>,
    local_ws_base_url: Option<String>,
    bootstrap_token: Option<String>,
) -> DesktopBridgeInitConfig {
    let preferred_system_languages = detect_preferred_system_languages();
    let system_locale = preferred_system_languages.first().cloned();
    let bootstraps = if local_http_base_url.is_some() || local_ws_base_url.is_some() {
        vec![DesktopEnvironmentBootstrap {
            id: "primary".to_owned(),
            label: "Local".to_owned(),
            running_distro: None,
            http_base_url: local_http_base_url,
            ws_base_url: local_ws_base_url,
            bootstrap_token,
        }]
    } else {
        Vec::new()
    };

    DesktopBridgeInitConfig {
        branding: resolve_branding(is_development, app_version),
        relay_telemetry_enabled: Some(false),
        client_platform: client_platform().to_owned(),
        primary_paste_enabled: true,
        system_locale,
        preferred_system_languages,
        local_environment_bootstraps: bootstraps,
        local_environment_enabled: true,
        window_fullscreen_state: false,
        window_zoom_factor: 1.0,
    }
}

/// Renders the complete `window.desktopBridge` initialization script with escaped JSON state.
pub fn render_desktop_bridge_init_script(
    config: &DesktopBridgeInitConfig,
) -> Result<String, String> {
    let raw_json = serde_json::to_string(config)
        .map_err(|err| format!("failed serializing desktopBridge init config: {err}"))?;
    let safe_json = escape_json_for_js_literal(&raw_json);
    Ok(PRELOAD_BRIDGE_TEMPLATE.replace(INIT_PLACEHOLDER, &safe_json))
}

fn escape_json_for_js_literal(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        match ch {
            '<' => out.push_str(r"\u003c"),
            '>' => out.push_str(r"\u003e"),
            '&' => out.push_str(r"\u0026"),
            '\u{2028}' => out.push_str(r"\u2028"),
            '\u{2029}' => out.push_str(r"\u2029"),
            other => out.push(other),
        }
    }
    out
}
