//! `server.probe` and `server.getConfig` RPC handlers (`ServerConfig` parity).

use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use legion_providers::core::ServerProviderSnapshot;
use serde_json::{Value, json};

use super::server_settings::{read_keybindings_with_issues, read_settings_json};
use super::{AppState, rpc_fail, rpc_fail_obj, utc_now_iso};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, _payload: Value) -> RpcExit {
    match tag {
        "server.probe" => RpcExit::Success {
            value: json!({ "serverTime": utc_now_iso() }),
        },
        "server.getConfig" => get_config(state).await,
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

async fn get_config(state: &Arc<AppState>) -> RpcExit {
    let settings = match read_settings_json(&state.paths.settings_path) {
        Ok(s) => s,
        Err(err) => {
            return rpc_fail_obj(json!({
                "_tag": "ServerSettingsError",
                "settingsPath": state.paths.settings_path.to_string_lossy(),
                "detail": err,
                "message": err,
            }));
        }
    };
    let (keybindings, kb_issues) =
        read_keybindings_with_issues(&state.paths.keybindings_config_path);
    let providers = build_server_providers_list(state).await;
    let available_editors = discover_available_editors();
    let cwd = std::env::current_dir()
        .map_or_else(|_| ".".to_owned(), |p| p.to_string_lossy().into_owned());

    RpcExit::Success {
        value: json!({
            "environment": state.descriptor,
            "auth": state.runtime_http.auth_descriptor_json(),
            "cwd": cwd,
            "keybindingsConfigPath": state.paths.keybindings_config_path.to_string_lossy(),
            "keybindings": keybindings,
            "issues": kb_issues,
            "providers": providers,
            "availableEditors": available_editors,
            "observability": {
                "logsDirectoryPath": state.paths.logs_dir.to_string_lossy(),
                "localTracingEnabled": true,
                "otlpTracesEnabled": false,
                "otlpMetricsEnabled": false,
                "otlpLogsEnabled": false,
            },
            "settings": settings,
            "shellThreadFamilies": true,
            "shellOlderSettled": true,
            "threadSnapshotPagination": true,
            "serverTime": utc_now_iso(),
        }),
    }
}

pub(crate) async fn build_server_providers_list(state: &AppState) -> Vec<Value> {
    let probed: Vec<ServerProviderSnapshot> = state
        .runtime_http
        .probe_service()
        .discover_local_providers()
        .await;
    let checked_at = utc_now_iso();
    let mut out = Vec::with_capacity(probed.len() + 1);
    let has_replay = probed
        .iter()
        .any(|p| p.driver == "devReplay" || p.instance_id == "devReplay");
    if !has_replay {
        out.push(json!({
            "instanceId": "devReplay",
            "driver": "devReplay",
            "displayName": "Replay (dev)",
            "supportedRuntimeModes": ["approval-required", "auto-accept-edits", "full-access"],
            "enabled": true,
            "installed": true,
            "status": "ready",
            "checkedAt": checked_at,
            "auth": { "status": "authenticated" },
            "models": [{
                "slug": "dev-replay",
                "name": "Replay (dev)",
                "isCustom": false,
                "capabilities": Value::Null,
            }],
        }));
    }
    for p in probed {
        out.push(json!(p));
    }
    out
}

fn discover_available_editors() -> Vec<&'static str> {
    const CANDIDATES: &[(&str, &str)] = &[
        ("cursor", "cursor"),
        ("vscode", "code"),
        ("vscode-insiders", "code-insiders"),
        ("vscodium", "codium"),
        ("zed", "zed"),
        ("antigravity", "antigravity-ide"),
    ];
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<_> = std::env::split_paths(&path_var).collect();
    CANDIDATES
        .iter()
        .filter(|(_, bin)| {
            dirs.iter().any(|d| {
                d.join(bin).is_file()
                    || (cfg!(windows)
                        && (d.join(format!("{bin}.exe")).is_file()
                            || d.join(format!("{bin}.cmd")).is_file()))
            })
        })
        .map(|(id, _)| *id)
        .collect()
}
