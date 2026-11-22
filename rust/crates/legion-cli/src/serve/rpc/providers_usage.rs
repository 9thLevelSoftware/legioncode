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
use super::{AppState, parse_payload, rpc_fail, rpc_fail_obj, utc_now_iso, with_db};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.refreshProviders" => refresh_providers(state).await,
        "server.updateProvider" => update_provider(state, payload).await,
        "server.getUsageSummary" | "server.refreshUsageRates" => get_usage_summary(state),
        "provider.uploadFeedback" => RpcExit::Success {
            value: json!({ "accepted": true }),
        },
        "provider.consumeResetCredit" => consume_reset_credit(state, payload).await,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProviderPayload {
    #[serde(alias = "provider", alias = "id")]
    instance_id: String,
    #[serde(default)]
    patch: Value,
    #[serde(default)]
    config: Option<Value>,
}

async fn update_provider(state: &AppState, payload: Value) -> RpcExit {
    let req: UpdateProviderPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut settings: Value = match read_settings_json(&state.paths.settings_path) {
        Ok(s) => s,
        Err(msg) => {
            return rpc_fail_obj(json!({
                "_tag": "ServerProviderUpdateError",
                "message": msg,
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
            let entry = map
                .entry(&req.instance_id)
                .or_insert_with(|| json!({ "config": {} }));
            deep_merge_json(entry, &patch);
        }
    }
    if let Err(msg) = write_json_atomic(&state.paths.settings_path, &settings) {
        return rpc_fail_obj(json!({
            "_tag": "ServerProviderUpdateError",
            "message": msg,
        }));
    }
    let providers = build_server_providers_list(state).await;
    let updated = providers
        .into_iter()
        .find(|p| {
            p.get("instanceId")
                .or_else(|| p.get("driver"))
                .or_else(|| p.get("id"))
                .and_then(Value::as_str)
                == Some(&req.instance_id)
        })
        .unwrap_or_else(|| {
            json!({
                "instanceId": req.instance_id,
                "driver": req.instance_id,
                "supportedRuntimeModes": ["approval-required"],
                "enabled": true,
                "installed": false,
                "status": "error",
                "checkedAt": utc_now_iso(),
                "auth": { "status": "unknown" },
                "models": [],
            })
        });
    RpcExit::Success {
        value: json!({ "provider": updated }),
    }
}

fn get_usage_summary(state: &AppState) -> RpcExit {
    let updated_at = utc_now_iso();
    with_db(state, |conn| {
        let total_runs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM orchestration_v2_projection_runs",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let input_tokens: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(CAST(json_extract(payload_json, '$.usage.inputTokens') AS INTEGER)), 0)
                 FROM orchestration_v2_projection_runs",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let output_tokens: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(CAST(json_extract(payload_json, '$.usage.outputTokens') AS INTEGER)), 0)
                 FROM orchestration_v2_projection_runs",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(RpcExit::Success {
            value: json!({
                "updatedAt": updated_at,
                "totalCostUsd": 0.0,
                "totalRuns": total_runs,
                "totals": {
                    "inputTokens": input_tokens,
                    "outputTokens": output_tokens,
                    "cachedInputTokens": 0,
                    "costUsd": 0.0,
                },
                "providers": [],
                "byModel": [],
                "byDay": [],
                "sources": [],
            }),
        })
    })
    .unwrap_or_else(|e| e)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsumeCreditPayload {
    instance_id: String,
    #[serde(default)]
    credit_id: Option<String>,
}

async fn consume_reset_credit(state: &AppState, payload: Value) -> RpcExit {
    let req: ConsumeCreditPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let providers = build_server_providers_list(state).await;
    let provider = providers
        .into_iter()
        .find(|p| {
            p.get("instanceId")
                .or_else(|| p.get("driver"))
                .or_else(|| p.get("id"))
                .and_then(Value::as_str)
                == Some(&req.instance_id)
        })
        .unwrap_or_else(|| {
            json!({
                "instanceId": req.instance_id,
                "driver": req.instance_id,
                "supportedRuntimeModes": ["approval-required"],
                "enabled": true,
                "installed": false,
                "status": "error",
                "checkedAt": utc_now_iso(),
                "auth": { "status": "unknown" },
                "models": [],
            })
        });
    RpcExit::Success {
        value: json!({
            "consumed": false,
            "creditId": req.credit_id,
            "provider": provider,
        }),
    }
}

fn auth_state_response(payload: &Value) -> RpcExit {
    let instance_id = payload
        .get("instanceId")
        .and_then(Value::as_str)
        .unwrap_or("claude");
    RpcExit::Success {
        value: json!({
            "instanceId": instance_id,
            "status": "idle",
            "methods": [],
            "updatedAt": utc_now_iso(),
        }),
    }
}

fn install_state_response(payload: &Value) -> RpcExit {
    let instance_id = payload
        .get("instanceId")
        .and_then(Value::as_str)
        .unwrap_or("claude");
    RpcExit::Success {
        value: json!({
            "instanceId": instance_id,
            "status": "idle",
            "updatedAt": utc_now_iso(),
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
