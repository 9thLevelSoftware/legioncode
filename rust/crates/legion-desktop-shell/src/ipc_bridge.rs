//! Core `window.desktopBridge` IPC handlers for [`crate::ipc::DesktopIpcRouter`]:
//! auth tokens, persisted client settings & connection catalog, native dialogs,
//! external URLs, media reading, updater state, and window controls.

use std::fs;
use std::path::Path;
use std::time::Duration;

use legion_edge::desktop_updates::DesktopUpdateMachine;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::bridge_types::DesktopBridgeRuntimeState;
use crate::http_loopback::{base64_encode, fetch_media_bytes, http_request};
use crate::ipc_handlers::{
    build_update_state, encode_result, handle_check_for_update, handle_download_update,
    handle_install_update, handle_set_update_channel, ipc_error,
};
use crate::ipc_types::DesktopIpcErrorPayload;
use crate::native_dialogs::{
    detect_preferred_system_languages, open_external_url, pick_folder, pick_project_favicon,
    pick_theme_files,
};
use crate::sidecar_process::mint_desktop_pairing_token;

const CLIENT_SETTINGS_FILE: &str = "client-settings.json";
const CONNECTION_CATALOG_FILE: &str = "connection-catalog.json";
const HTTP_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);
const MEDIA_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct BearerTokenParams {
    force_refresh: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PickFolderParams {
    initial_path: Option<String>,
    target_environment_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PickFaviconParams {
    initial_path: Option<String>,
}

/// Dispatches core `desktop.bridge.*` methods, returning `Some(result)` when matched.
pub(crate) fn dispatch_core_bridge(
    method: &str,
    params: Value,
    state: &mut DesktopBridgeRuntimeState,
    update_machine: &mut DesktopUpdateMachine,
) -> Option<Result<Value, DesktopIpcErrorPayload>> {
    match method {
        "desktop.bridge.getLocalEnvironmentBearerToken" => {
            Some(handle_get_bearer_token(params, state))
        }
        "desktop.bridge.setLocalEnvironmentEnabled" => {
            Some(handle_set_local_env_enabled(&params, state))
        }
        "desktop.bridge.getHostEnvironmentId" => Some(Ok(handle_get_host_environment_id(state))),
        "desktop.bridge.getClientSettings" => Some(handle_get_client_settings(state)),
        "desktop.bridge.setClientSettings" => Some(handle_set_client_settings(&params, state)),
        "desktop.bridge.getConnectionCatalog" => Some(handle_get_connection_catalog(state)),
        "desktop.bridge.setConnectionCatalog" => Some(handle_set_connection_catalog(params, state)),
        "desktop.bridge.pickFolder" => Some(handle_pick_folder(params)),
        "desktop.bridge.pickProjectFavicon" => Some(handle_pick_favicon(params)),
        "desktop.bridge.pickThemeFiles" => Some(handle_pick_theme_files()),
        "desktop.bridge.openExternal" => Some(handle_open_external(&params)),
        "desktop.bridge.readMedia" => Some(handle_read_media(&params)),
        "desktop.bridge.setTheme" => Some(handle_set_theme(&params, state)),
        "desktop.bridge.revealWindow" => {
            state.reveal_requests = state.reveal_requests.saturating_add(1);
            Some(Ok(Value::Null))
        }
        "desktop.bridge.getSystemLocale" => {
            let langs = detect_preferred_system_languages();
            Some(encode_result(langs.first().cloned()))
        }
        "desktop.bridge.getPreferredSystemLanguages" => {
            Some(encode_result(detect_preferred_system_languages()))
        }
        "desktop.bridge.getUpdateState" => Some(encode_result(build_update_state(update_machine))),
        "desktop.bridge.setUpdateChannel" => {
            Some(handle_set_update_channel(params, update_machine))
        }
        "desktop.bridge.checkForUpdate" => Some(Ok(handle_check_for_update(update_machine))),
        "desktop.bridge.downloadUpdate" => Some(Ok(handle_download_update(update_machine))),
        "desktop.bridge.installUpdate" => Some(handle_install_update(params, update_machine)),
        _ => None,
    }
}

fn handle_get_bearer_token(
    params: Value,
    state: &mut DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    let req: BearerTokenParams = if params.is_null() {
        BearerTokenParams::default()
    } else {
        serde_json::from_value(params).map_err(|e| ipc_error("InvalidParams", e.to_string()))?
    };
    if !req.force_refresh
        && let Some(cached) = &state.cached_bearer_token
    {
        return Ok(Value::String(cached.clone()));
    }

    let Some(base_url) = state.local_http_base_url.as_deref() else {
        return Err(ipc_error(
            "LocalBackendUnavailable",
            "local environment HTTP base URL is not configured",
        ));
    };

    let credential = if state.db_path.is_file() || state.local_bootstrap_token.is_none() {
        mint_desktop_pairing_token(&state.db_path, "LegionCode Desktop")
            .or_else(|_| {
                state
                    .local_bootstrap_token
                    .clone()
                    .ok_or_else(|| "no pairing credential available".to_owned())
            })
            .map_err(|msg| ipc_error("TokenMintError", msg))?
    } else {
        state
            .local_bootstrap_token
            .clone()
            .ok_or_else(|| ipc_error("TokenMissing", "missing bootstrap token"))?
    };

    let endpoint = format!("{}/oauth/token", base_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&json!({
        "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
        "subject_token": credential,
        "client_label": "LegionCode Desktop",
    }))
    .map_err(|e| ipc_error("SerializationError", e.to_string()))?;

    let resp = http_request(
        "POST",
        &endpoint,
        &[("Content-Type", "application/json")],
        Some(&body),
        HTTP_EXCHANGE_TIMEOUT,
    )
    .map_err(|msg| ipc_error("TokenExchangeNetworkError", msg))?;

    if !(200..300).contains(&resp.status) {
        return Err(ipc_error(
            "TokenExchangeFailed",
            format!(
                "POST /oauth/token returned HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            ),
        ));
    }

    let parsed: Value = serde_json::from_slice(&resp.body)
        .map_err(|e| ipc_error("TokenExchangeParseError", e.to_string()))?;
    let token = parsed
        .get("access_token")
        .or_else(|| parsed.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ipc_error(
                "TokenExchangeParseError",
                "missing access_token in response",
            )
        })?
        .to_owned();

    state.cached_bearer_token = Some(token.clone());
    Ok(Value::String(token))
}

fn handle_set_local_env_enabled(
    params: &Value,
    state: &mut DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    let enabled = params
        .as_bool()
        .or_else(|| params.get("enabled").and_then(Value::as_bool))
        .ok_or_else(|| ipc_error("InvalidParams", "expected boolean enabled flag"))?;
    state.local_environment_enabled = enabled;
    Ok(Value::Null)
}

fn handle_get_host_environment_id(state: &DesktopBridgeRuntimeState) -> Value {
    let env_path = state.userdata_dir.join("environment-id");
    let value = fs::read_to_string(env_path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    match value {
        Some(id) => Value::String(id),
        None => Value::Null,
    }
}

fn handle_get_client_settings(
    state: &DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    let path = state.userdata_dir.join(CLIENT_SETTINGS_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(Value::Null);
            }
            serde_json::from_str::<Value>(trimmed)
                .map_err(|e| ipc_error("SettingsParseError", e.to_string()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(err) => Err(ipc_error("SettingsReadError", err.to_string())),
    }
}

fn handle_set_client_settings(
    params: &Value,
    state: &DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    if !params.is_object() {
        return Err(ipc_error(
            "InvalidParams",
            "client settings must be a JSON object",
        ));
    }
    let pretty = serde_json::to_string_pretty(params)
        .map_err(|e| ipc_error("SerializationError", e.to_string()))?;
    atomic_write_string(&state.userdata_dir.join(CLIENT_SETTINGS_FILE), &pretty)
        .map_err(|e| ipc_error("SettingsWriteError", e))?;
    Ok(Value::Null)
}

fn handle_get_connection_catalog(
    state: &DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    let path = state.userdata_dir.join(CONNECTION_CATALOG_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(Value::String(text))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(err) => Err(ipc_error("CatalogReadError", err.to_string())),
    }
}

fn handle_set_connection_catalog(
    params: Value,
    state: &DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    let raw_catalog = match params {
        Value::String(text) => {
            if serde_json::from_str::<Value>(&text).is_err() {
                return Ok(Value::Bool(false));
            }
            text
        }
        Value::Object(_) => serde_json::to_string_pretty(&params)
            .map_err(|e| ipc_error("SerializationError", e.to_string()))?,
        _ => {
            return Err(ipc_error(
                "InvalidParams",
                "expected connection catalog JSON string",
            ));
        }
    };
    atomic_write_string(
        &state.userdata_dir.join(CONNECTION_CATALOG_FILE),
        &raw_catalog,
    )
    .map_err(|e| ipc_error("CatalogWriteError", e))?;
    Ok(Value::Bool(true))
}

fn atomic_write_string(path: &Path, contents: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err("missing parent directory".to_owned());
    };
    fs::create_dir_all(parent)
        .map_err(|e| format!("failed creating '{}': {e}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("payload.json");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    fs::write(&tmp_path, contents.as_bytes())
        .map_err(|e| format!("failed writing '{}': {e}", tmp_path.display()))?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("failed moving into '{}': {err}", path.display()));
    }
    Ok(())
}

fn handle_pick_folder(params: Value) -> Result<Value, DesktopIpcErrorPayload> {
    let req: PickFolderParams = if params.is_null() {
        PickFolderParams::default()
    } else {
        serde_json::from_value(params).map_err(|e| ipc_error("InvalidParams", e.to_string()))?
    };
    if let Some(target) = req.target_environment_id.as_deref()
        && !target.is_empty()
        && target != "primary"
    {
        return Ok(Value::Null);
    }
    let picked =
        pick_folder(req.initial_path.as_deref()).map_err(|e| ipc_error("PickerError", e))?;
    encode_result(picked)
}

fn handle_pick_favicon(params: Value) -> Result<Value, DesktopIpcErrorPayload> {
    let req: PickFaviconParams = if params.is_null() {
        PickFaviconParams::default()
    } else {
        serde_json::from_value(params).map_err(|e| ipc_error("InvalidParams", e.to_string()))?
    };
    let picked = pick_project_favicon(req.initial_path.as_deref())
        .map_err(|e| ipc_error("PickerError", e))?;
    encode_result(picked)
}

fn handle_pick_theme_files() -> Result<Value, DesktopIpcErrorPayload> {
    let files = pick_theme_files().map_err(|e| ipc_error("PickerError", e))?;
    encode_result(files)
}

fn handle_open_external(params: &Value) -> Result<Value, DesktopIpcErrorPayload> {
    let url = params
        .as_str()
        .or_else(|| params.get("url").and_then(Value::as_str))
        .ok_or_else(|| ipc_error("InvalidParams", "expected URL string"))?;
    let opened = open_external_url(url).map_err(|e| ipc_error("OpenExternalError", e))?;
    Ok(Value::Bool(opened))
}

fn handle_read_media(params: &Value) -> Result<Value, DesktopIpcErrorPayload> {
    let url = params
        .as_str()
        .or_else(|| params.get("url").and_then(Value::as_str))
        .ok_or_else(|| ipc_error("InvalidParams", "expected URL string"))?;
    match fetch_media_bytes(url, MEDIA_FETCH_TIMEOUT) {
        Ok(resp) => Ok(json!({
            "status": resp.status,
            "mimeType": resp.content_type.unwrap_or_else(|| "application/octet-stream".to_owned()),
            "dataBase64": base64_encode(&resp.body),
        })),
        Err(_) => Ok(Value::Null),
    }
}

fn handle_set_theme(
    params: &Value,
    state: &mut DesktopBridgeRuntimeState,
) -> Result<Value, DesktopIpcErrorPayload> {
    let theme = params
        .as_str()
        .or_else(|| params.get("theme").and_then(Value::as_str))
        .ok_or_else(|| ipc_error("InvalidParams", "expected theme string"))?;
    match theme {
        "light" | "dark" | "system" => {
            theme.clone_into(&mut state.theme);
            Ok(Value::Null)
        }
        other => Err(ipc_error(
            "InvalidParams",
            format!("invalid theme `{other}`"),
        )),
    }
}
