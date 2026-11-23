//! Provider probe/config updates, usage summaries, auth/install flows, and relay client status.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use super::projects_fs::base_dir_from_state;
use super::server_config::build_server_providers_list;
use super::server_settings::{deep_merge_json, read_settings_json, write_json_atomic};
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj, utc_now_iso};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.refreshProviders" => refresh_providers(state).await,
        "server.updateProvider" => update_provider(state, payload).await,
        "server.getUsageSummary" => get_usage_summary(&payload),
        "server.refreshUsageRates" => RpcExit::Success {
            value: json!({
                "status": "unavailable",
                "source": "unavailable",
                "fetchedAt": Value::Null,
                "knownModels": 0,
            }),
        },
        "provider.uploadFeedback" => RpcExit::Success {
            value: json!({ "feedbackId": format!("fb-{}", uuid::Uuid::new_v4()) }),
        },
        "provider.consumeResetCredit" => consume_reset_credit(payload),
        "provider.auth.start"
        | "provider.auth.respond"
        | "provider.auth.complete"
        | "provider.auth.cancel"
        | "provider.auth.removeMethod"
        | "provider.auth.logout"
        | "provider.chatgpt.import-profile" => auth_state_response(&payload),
        "provider.chatgpt.reconnect-profile" => RpcExit::Success { value: Value::Null },
        "provider.install.start" | "provider.install.cancel" | "provider.install.remove" => {
            install_state_response(&payload)
        }
        "cloud.getRelayClientStatus" => get_relay_client_status(state).await,
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

async fn refresh_providers(state: &AppState) -> RpcExit {
    let providers = build_server_providers_list(state).await;
    RpcExit::Success {
        value: json!({ "providers": providers }),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UpdateProviderPayload {
    instance_id: Option<String>,
    provider: Option<String>,
    id: Option<String>,
    patch: Value,
    config: Option<Value>,
}

async fn update_provider(state: &AppState, payload: Value) -> RpcExit {
    let req: UpdateProviderPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let instance_id = req
        .instance_id
        .or(req.provider)
        .or(req.id)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "devReplay".to_owned());
    let mut settings: Value = match read_settings_json(&state.paths.settings_path) {
        Ok(s) => s,
        Err(msg) => {
            return rpc_fail_obj(json!({
                "_tag": "ServerProviderUpdateError",
                "provider": instance_id,
                "reason": msg,
            }));
        }
    };
    let patch = req.config.unwrap_or(req.patch);
    if !settings.is_object() {
        settings = json!({});
    }
    if let Some(root) = settings.as_object_mut() {
        let instances = root.entry("providerInstances").or_insert_with(|| json!({}));
        if let Some(map) = instances.as_object_mut() {
            let driver_slug = legion_providers::drivers::normalize_driver_slug(&instance_id);
            let entry = map
                .entry(&instance_id)
                .or_insert_with(|| json!({ "driver": driver_slug, "config": {} }));
            if !entry.is_object() {
                *entry = json!({ "driver": driver_slug, "config": {} });
            }
            if patch.is_object() {
                deep_merge_json(entry, &patch);
            }
        }
    }
    if let Err(msg) = write_json_atomic(&state.paths.settings_path, &settings) {
        return rpc_fail_obj(json!({
            "_tag": "ServerProviderUpdateError",
            "provider": instance_id,
            "reason": msg,
        }));
    }
    let providers = build_server_providers_list(state).await;
    RpcExit::Success {
        value: json!({ "providers": providers }),
    }
}

fn get_usage_summary(payload: &Value) -> RpcExit {
    let since_day = payload
        .get("sinceDay")
        .and_then(Value::as_str)
        .unwrap_or("2026-01-01");
    let until_day = payload
        .get("untilDay")
        .and_then(Value::as_str)
        .unwrap_or("2026-12-31");
    let time_zone = payload
        .get("timeZone")
        .and_then(Value::as_str)
        .unwrap_or("UTC");
    RpcExit::Success {
        value: json!({
            "contractVersion": 6,
            "readAt": utc_now_iso(),
            "timeZone": time_zone,
            "sinceDay": since_day,
            "untilDay": until_day,
            "buckets": [],
            "sources": [],
            "pricing": {
                "status": "unavailable",
                "source": "unavailable",
                "fetchedAt": Value::Null,
                "knownModels": 0,
            },
            "scanDurationMs": 0,
        }),
    }
}

fn consume_reset_credit(_payload: Value) -> RpcExit {
    RpcExit::Success {
        value: json!({
            "outcome": "noCredit",
        }),
    }
}

fn auth_state_response(payload: &Value) -> RpcExit {
    let instance_id = payload
        .get("instanceId")
        .and_then(Value::as_str)
        .unwrap_or("codex");
    RpcExit::Success {
        value: json!({
            "instanceId": instance_id,
            "phase": "idle",
            "flowId": Value::Null,
            "authorizationUrl": Value::Null,
            "expiresAt": Value::Null,
            "message": Value::Null,
            "methods": [],
        }),
    }
}

fn install_state_response(payload: &Value) -> RpcExit {
    let driver = payload
        .get("driver")
        .or_else(|| payload.get("instanceId"))
        .and_then(Value::as_str)
        .unwrap_or("codex");
    RpcExit::Success {
        value: json!({
            "driver": driver,
            "operationId": Value::Null,
            "phase": "idle",
            "downloadedBytes": 0,
            "totalBytes": Value::Null,
            "version": Value::Null,
            "installedVersion": Value::Null,
            "canRemove": false,
            "message": Value::Null,
        }),
    }
}

async fn get_relay_client_status(state: &AppState) -> RpcExit {
    let managed = base_dir_from_state(state)
        .join("bin")
        .join(if cfg!(windows) {
            "cloudflared.exe"
        } else {
            "cloudflared"
        });
    if managed.is_file() {
        return RpcExit::Success {
            value: json!({
                "status": "available",
                "executablePath": managed.to_string_lossy(),
                "source": "managed",
                "version": env!("CARGO_PKG_VERSION"),
            }),
        };
    }
    if let Some(on_path) = find_on_path("cloudflared") {
        let version = Command::new(&on_path)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map_or_else(
                || env!("CARGO_PKG_VERSION").to_owned(),
                |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
            );
        return RpcExit::Success {
            value: json!({
                "status": "available",
                "executablePath": on_path.to_string_lossy(),
                "source": "path",
                "version": version,
            }),
        };
    }
    RpcExit::Success {
        value: json!({
            "status": "missing",
            "version": env!("CARGO_PKG_VERSION"),
        }),
    }
}

fn find_on_path(bin: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(bin);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) {
            for ext in ["exe", "cmd"] {
                let with_ext = dir.join(format!("{bin}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}
