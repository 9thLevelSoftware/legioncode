//! Secondary `window.desktopBridge` IPC handlers: server exposure, WSL, SSH environments,
//! CLI command installation, snapshots, preview tabs, and lifecycle callbacks.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::bridge_types::{
    DesktopBridgeRuntimeState, DesktopCliCommandStatePayload, DesktopServerExposureStatePayload,
};
use crate::http_loopback::http_request;
use crate::ipc_bridge_preview::{dispatch_preview, dispatch_snapshot};
use crate::ipc_handlers::{encode_result, ipc_error};
use crate::ipc_types::DesktopIpcErrorPayload;
use crate::native_dialogs::{open_system_settings, probe_remote_editors, to_local_backend_path};
use crate::sidecar::SidecarController;

const SSH_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TailscaleServeParams {
    enabled: bool,
    port: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshBootstrapParams {
    http_base_url: String,
    credential: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshBearerParams {
    http_base_url: String,
    bearer_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToLocalBackendPathParams {
    backend_id: String,
    windows_path: String,
}

/// Dispatches secondary `desktop.bridge.*` domains, returning `Some(result)` when matched.
pub(crate) fn dispatch_extra_bridge(
    method: &str,
    params: Value,
    state: &mut DesktopBridgeRuntimeState,
    sidecar: &SidecarController,
) -> Option<Result<Value, DesktopIpcErrorPayload>> {
    if let Some(res) = dispatch_exposure_wsl(method, &params, state) {
        return Some(res);
    }
    if let Some(res) = dispatch_ssh(method, &params) {
        return Some(res);
    }
    if let Some(res) = dispatch_snapshot(method, &params, state) {
        return Some(res);
    }
    if let Some(res) = dispatch_preview(method, &params, state) {
        return Some(res);
    }
    match method {
        "desktop.bridge.cliCommand.getState"
        | "desktop.bridge.cliCommand.install"
        | "desktop.bridge.cliCommand.uninstall"
        | "desktop.bridge.cliCommand.addToPath" => Some(encode_result(cli_command_state(sidecar))),
        "desktop.bridge.webLinks.setReady" => {
            state.readiness.web_links_ready = params
                .as_bool()
                .or_else(|| params.get("ready").and_then(Value::as_bool))
                .unwrap_or(true);
            Some(Ok(Value::Null))
        }
        "desktop.bridge.appActivation.setReady" => {
            state.readiness.app_activation_ready = params
                .as_bool()
                .or_else(|| params.get("ready").and_then(Value::as_bool))
                .unwrap_or(true);
            Some(Ok(Value::Null))
        }
        "desktop.bridge.appActivation.complete"
        | "desktop.bridge.cancelProviderAuthCallback"
        | "desktop.bridge.showContextMenu" => Some(Ok(Value::Null)),
        "desktop.bridge.receiveProviderAuthCallback" => Some(Err(ipc_error(
            "ProviderAuthCancelled",
            "provider OAuth callback listener is not active in this host",
        ))),
        "desktop.bridge.startFileDrag" | "desktop.bridge.restartAudioService" => {
            Some(Ok(Value::Bool(false)))
        }
        "desktop.bridge.pasteAsText" => {
            state.paste_as_text_requests = state.paste_as_text_requests.saturating_add(1);
            Some(Ok(Value::Null))
        }
        "desktop.bridge.probeRemoteEditors" => Some(encode_result(probe_remote_editors())),
        "desktop.bridge.setNotificationBadge" => {
            state.notification_badge_count = params
                .get("count")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0);
            Some(Ok(Value::Null))
        }
        "desktop.bridge.checkSystemPermission" => {
            let pane = params
                .as_str()
                .or_else(|| params.get("pane").and_then(Value::as_str))
                .unwrap_or_default();
            Some(Ok(Value::Bool(
                pane == "full-disk-access" && !cfg!(target_os = "macos"),
            )))
        }
        "desktop.bridge.openSystemSettings" => {
            let pane = params
                .as_str()
                .or_else(|| params.get("pane").and_then(Value::as_str))
                .unwrap_or_default();
            Some(Ok(Value::Bool(open_system_settings(pane))))
        }
        "desktop.bridge.toLocalBackendPath" => {
            let req = serde_json::from_value::<ToLocalBackendPathParams>(params)
                .map_err(|e| ipc_error("InvalidParams", e.to_string()));
            Some(
                req.and_then(|r| {
                    encode_result(to_local_backend_path(&r.backend_id, &r.windows_path))
                }),
            )
        }
        _ => None,
    }
}

fn dispatch_exposure_wsl(
    method: &str,
    params: &Value,
    state: &mut DesktopBridgeRuntimeState,
) -> Option<Result<Value, DesktopIpcErrorPayload>> {
    match method {
        "desktop.bridge.getServerExposureState" => Some(encode_result(exposure_payload(state))),
        "desktop.bridge.setServerExposureMode" => {
            let mode = params
                .as_str()
                .or_else(|| params.get("mode").and_then(Value::as_str))
                .unwrap_or("local-only");
            match mode {
                "local-only" | "network-accessible" => {
                    mode.clone_into(&mut state.exposure_mode);
                    Some(encode_result(exposure_payload(state)))
                }
                other => Some(Err(ipc_error(
                    "InvalidParams",
                    format!("invalid server exposure mode `{other}`"),
                ))),
            }
        }
        "desktop.bridge.setTailscaleServeEnabled" => {
            let req: TailscaleServeParams = match serde_json::from_value(params.clone()) {
                Ok(parsed) => parsed,
                Err(e) => return Some(Err(ipc_error("InvalidParams", e.to_string()))),
            };
            state.tailscale_serve_enabled = req.enabled;
            if let Some(port) = req.port {
                state.tailscale_serve_port = port;
            }
            Some(encode_result(exposure_payload(state)))
        }
        "desktop.bridge.getAdvertisedEndpoints" => Some(Ok(json!([]))),
        "desktop.bridge.getWslState" => Some(encode_result(state.wsl_state.clone())),
        "desktop.bridge.setWslBackendEnabled" => {
            state.wsl_state.enabled = params
                .as_bool()
                .or_else(|| params.get("enabled").and_then(Value::as_bool))
                .unwrap_or(false);
            Some(encode_result(state.wsl_state.clone()))
        }
        "desktop.bridge.setWslDistro" => {
            state.wsl_state.distro = params
                .as_str()
                .or_else(|| params.get("distro").and_then(Value::as_str))
                .map(str::to_owned);
            Some(encode_result(state.wsl_state.clone()))
        }
        "desktop.bridge.setWslOnly" => {
            state.wsl_state.wsl_only = params
                .as_bool()
                .or_else(|| params.get("enabled").and_then(Value::as_bool))
                .unwrap_or(false);
            Some(encode_result(state.wsl_state.clone()))
        }
        _ => None,
    }
}

fn dispatch_ssh(method: &str, params: &Value) -> Option<Result<Value, DesktopIpcErrorPayload>> {
    match method {
        "desktop.bridge.discoverSshHosts" => Some(Ok(json!([]))),
        "desktop.bridge.resolveSshHost" => {
            let alias = params
                .as_str()
                .or_else(|| params.get("alias").and_then(Value::as_str))
                .unwrap_or_default()
                .trim();
            if alias.is_empty() {
                Some(Err(ipc_error("InvalidParams", "missing SSH alias")))
            } else {
                Some(Ok(json!({
                    "alias": alias,
                    "hostname": alias,
                    "username": null,
                    "port": 22,
                })))
            }
        }
        "desktop.bridge.ensureSshEnvironment" => Some(Err(ipc_error(
            "SshUnavailable",
            "SSH tunnel environments are not supported by this desktop host",
        ))),
        "desktop.bridge.disconnectSshEnvironment" | "desktop.bridge.resolveSshPasswordPrompt" => {
            Some(Ok(Value::Null))
        }
        "desktop.bridge.fetchSshEnvironmentDescriptor" => Some(fetch_ssh_descriptor(params)),
        "desktop.bridge.bootstrapSshBearerSession" => Some(bootstrap_ssh_bearer(params)),
        "desktop.bridge.fetchSshSessionState" => Some(fetch_ssh_session(params)),
        _ => None,
    }
}

fn exposure_payload(state: &DesktopBridgeRuntimeState) -> DesktopServerExposureStatePayload {
    DesktopServerExposureStatePayload {
        mode: state.exposure_mode.clone(),
        endpoint_url: if state.exposure_mode == "network-accessible" {
            state.local_http_base_url.clone()
        } else {
            None
        },
        advertised_host: None,
        tailscale_serve_enabled: state.tailscale_serve_enabled,
        tailscale_serve_port: state.tailscale_serve_port,
    }
}

fn cli_command_state(sidecar: &SidecarController) -> DesktopCliCommandStatePayload {
    let discovery = sidecar.discover_runtime();
    let launcher_path = discovery
        .found
        .then(|| discovery.binary_path.to_string_lossy().into_owned());
    let on_path = matches!(
        discovery.source,
        crate::sidecar::RuntimeDiscoverySource::SystemPath
    );
    DesktopCliCommandStatePayload {
        supported: launcher_path.is_some(),
        installed_path: launcher_path.clone(),
        on_path,
        launcher_path,
    }
}

fn fetch_ssh_descriptor(params: &Value) -> Result<Value, DesktopIpcErrorPayload> {
    let base_url = params
        .as_str()
        .or_else(|| params.get("httpBaseUrl").and_then(Value::as_str))
        .ok_or_else(|| ipc_error("InvalidParams", "missing httpBaseUrl"))?;
    let url = format!(
        "{}/.well-known/legioncode/environment",
        base_url.trim_end_matches('/')
    );
    let resp = http_request("GET", &url, &[], None, SSH_HTTP_TIMEOUT)
        .map_err(|e| ipc_error("SshDescriptorError", e))?;
    if !(200..300).contains(&resp.status) {
        return Err(ipc_error(
            "SshDescriptorHttpError",
            format!("HTTP {} from `{url}`", resp.status),
        ));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| ipc_error("SshDescriptorParseError", e.to_string()))
}

fn bootstrap_ssh_bearer(params: &Value) -> Result<Value, DesktopIpcErrorPayload> {
    let req: SshBootstrapParams = serde_json::from_value(params.clone())
        .map_err(|e| ipc_error("InvalidParams", e.to_string()))?;
    let url = format!("{}/oauth/token", req.http_base_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&json!({
        "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
        "subject_token": req.credential,
        "client_label": "LegionCode Desktop",
    }))
    .map_err(|e| ipc_error("SerializationError", e.to_string()))?;
    let resp = http_request(
        "POST",
        &url,
        &[("Content-Type", "application/json")],
        Some(&body),
        SSH_HTTP_TIMEOUT,
    )
    .map_err(|e| ipc_error("SshBearerBootstrapError", e))?;
    if !(200..300).contains(&resp.status) {
        return Err(ipc_error(
            "SshBearerBootstrapHttpError",
            format!("HTTP {} from `{url}`", resp.status),
        ));
    }
    serde_json::from_slice(&resp.body).map_err(|e| ipc_error("SshBearerParseError", e.to_string()))
}

fn fetch_ssh_session(params: &Value) -> Result<Value, DesktopIpcErrorPayload> {
    let req: SshBearerParams = serde_json::from_value(params.clone())
        .map_err(|e| ipc_error("InvalidParams", e.to_string()))?;
    let url = format!(
        "{}/api/auth/session",
        req.http_base_url.trim_end_matches('/')
    );
    let auth_header = format!("Bearer {}", req.bearer_token);
    let resp = http_request(
        "GET",
        &url,
        &[("Authorization", &auth_header)],
        None,
        SSH_HTTP_TIMEOUT,
    )
    .map_err(|e| ipc_error("SshSessionError", e))?;
    if !(200..300).contains(&resp.status) {
        return Err(ipc_error(
            "SshSessionHttpError",
            format!("HTTP {} from `{url}`", resp.status),
        ));
    }
    serde_json::from_slice(&resp.body).map_err(|e| ipc_error("SshSessionParseError", e.to_string()))
}
