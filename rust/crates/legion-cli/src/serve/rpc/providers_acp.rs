//! ACP Registry catalog lookup, agent lifecycle, and persisted ACP provider settings.

use std::fs;
use std::time::Duration;

use legion_protocol::rpc_codec::RpcExit;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use super::projects_fs::base_dir_from_state;
use super::server_settings::{read_settings_json, write_json_atomic};
use super::{AppState, parse_optional_payload, parse_payload, rpc_fail, rpc_fail_obj};

const ACP_REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

pub(crate) async fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.searchAcpRegistry" => search_acp_registry(payload).await,
        "server.prepareAcpRegistryAgent" => prepare_acp_agent(payload),
        "server.uninstallAcpRegistryManagedBinary" => uninstall_acp_binary(state, payload),
        "server.acceptAcpRegistryUrlAuth" => RpcExit::Success {
            value: json!({ "accepted": true }),
        },
        "server.listAcpRegistrySessions" => RpcExit::Success {
            value: json!({
                "sessions": [],
                "nextCursor": Value::Null,
                "canLoad": false,
                "canResume": false,
                "canDelete": false,
            }),
        },
        "server.importAcpRegistrySession" => import_acp_session(&payload),
        "server.deleteAcpRegistrySession" => RpcExit::Success {
            value: json!({ "deleted": true }),
        },
        "server.listAcpRegistryProviders" => list_acp_providers(state),
        "server.setAcpRegistryProvider" => set_acp_provider(state, &payload),
        "server.disableAcpRegistryProvider" => disable_acp_provider(state, payload),
        "server.logoutAcpRegistry" => RpcExit::Success {
            value: json!({ "loggedOut": true }),
        },
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SearchAcpPayload {
    query: Option<String>,
}

async fn search_acp_registry(payload: Value) -> RpcExit {
    let req: SearchAcpPayload = match parse_optional_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let q = req
        .query
        .as_deref()
        .map_or("", str::trim)
        .to_ascii_lowercase();
    let catalog = fetch_acp_catalog().await;
    let agents: Vec<Value> = catalog
        .into_iter()
        .filter(|entry| {
            if q.is_empty() {
                return true;
            }
            let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
            let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
            let desc = entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            id.to_ascii_lowercase().contains(&q)
                || name.to_ascii_lowercase().contains(&q)
                || desc.to_ascii_lowercase().contains(&q)
        })
        .filter_map(|entry| normalize_acp_registry_agent(&entry))
        .take(512)
        .collect();
    RpcExit::Success {
        value: json!({ "agents": agents }),
    }
}

fn normalize_acp_registry_agent(entry: &Value) -> Option<Value> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(id);
    let version = entry
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("0.0.0");
    let description = entry
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let authors: Vec<String> = entry
        .get("authors")
        .and_then(Value::as_array)
        .map_or_default(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .take(16)
                .collect()
        });
    let license = entry
        .get("license")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let website = entry
        .get("website")
        .or_else(|| entry.get("homepage"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let repository = entry
        .get("repository")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let icon = entry
        .get("icon")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            Some(format!(
                "https://cdn.agentclientprotocol.com/registry/v1/latest/{id}.svg"
            ))
        });
    let dist_obj = entry.get("distribution").and_then(Value::as_object);
    let distribution = if dist_obj.is_some_and(|d| d.get("npx").is_some()) {
        "npx"
    } else if dist_obj.is_some_and(|d| d.get("uvx").is_some()) {
        "uvx"
    } else {
        "binary"
    };
    Some(json!({
        "id": id,
        "name": name,
        "version": version,
        "description": description,
        "authors": authors,
        "license": license,
        "website": website,
        "repository": repository,
        "icon": icon,
        "distribution": distribution,
        "integrity": "registry",
    }))
}

async fn fetch_acp_catalog() -> Vec<Value> {
    let Ok(client) = Client::builder().timeout(Duration::from_secs(3)).build() else {
        return Vec::new();
    };
    let Ok(resp) = client.get(ACP_REGISTRY_URL).send().await else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(parsed) = resp.json::<Value>().await else {
        return Vec::new();
    };
    if let Some(arr) = parsed.get("agents").and_then(Value::as_array) {
        return arr.clone();
    }
    parsed.as_array().cloned().unwrap_or_default()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareAcpPayload {
    agent_id: String,
}

fn prepare_acp_agent(payload: Value) -> RpcExit {
    let req: PrepareAcpPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    rpc_fail_obj(json!({
        "_tag": "AcpRegistryOperationError",
        "reason": "agent_not_found",
        "message": format!("ACP Registry agent `{}` was not prepared in this session.", req.agent_id),
    }))
}

fn import_acp_session(payload: &Value) -> RpcExit {
    let title = payload
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("session");
    rpc_fail_obj(json!({
        "_tag": "AcpRegistryOperationError",
        "reason": "session_import_failed",
        "message": format!("ACP Registry session `{title}` cannot be imported without a prepared ACP runner."),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UninstallAcpPayload {
    agent_id: String,
}

fn uninstall_acp_binary(state: &AppState, payload: Value) -> RpcExit {
    let req: UninstallAcpPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let safe_id = req.agent_id.trim();
    if safe_id.is_empty()
        || safe_id.contains('/')
        || safe_id.contains('\\')
        || safe_id.contains("..")
    {
        return rpc_fail_obj(json!({
            "_tag": "AcpRegistryOperationError",
            "reason": "agent_not_found",
            "message": "Invalid ACP Registry agent id.",
        }));
    }
    let dir = base_dir_from_state(state)
        .join("acp-registry")
        .join("bin")
        .join(safe_id);
    let removed = if dir.exists() {
        fs::remove_dir_all(&dir).is_ok()
    } else {
        false
    };
    RpcExit::Success {
        value: json!({
            "agentId": safe_id,
            "removed": removed,
        }),
    }
}

fn list_acp_providers(state: &AppState) -> RpcExit {
    let settings = read_settings_json(&state.paths.settings_path).unwrap_or_else(|_| json!({}));
    let providers = settings
        .pointer("/acpProviders")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    RpcExit::Success {
        value: json!({ "providers": providers }),
    }
}

fn set_acp_provider(state: &AppState, payload: &Value) -> RpcExit {
    let mut settings = read_settings_json(&state.paths.settings_path).unwrap_or_else(|_| json!({}));
    let id = payload
        .get("providerId")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("acp-custom")
        .to_owned();
    if let Some(root) = settings.as_object_mut()
        && let Some(list) = root
            .entry("acpProviders")
            .or_insert_with(|| json!([]))
            .as_array_mut()
    {
        list.retain(|item| {
            item.get("providerId")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                != Some(&id)
        });
        list.push(payload.clone());
    }
    let _ = write_json_atomic(&state.paths.settings_path, &settings);
    RpcExit::Success {
        value: json!({ "configured": true }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisableAcpPayload {
    #[serde(alias = "id")]
    provider_id: String,
}

fn disable_acp_provider(state: &AppState, payload: Value) -> RpcExit {
    let req: DisableAcpPayload = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut settings = read_settings_json(&state.paths.settings_path).unwrap_or_else(|_| json!({}));
    if let Some(list) = settings
        .get_mut("acpProviders")
        .and_then(Value::as_array_mut)
    {
        list.retain(|item| {
            item.get("providerId")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                != Some(&req.provider_id)
        });
    }
    let _ = write_json_atomic(&state.paths.settings_path, &settings);
    RpcExit::Success {
        value: json!({ "disabled": true }),
    }
}
