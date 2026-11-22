//! Preview browser and native mobile device manager RPCs (`preview.*`, `device.*`) plus routing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, peers_secrets_share, rpc_fail, utc_now_iso};

#[derive(Default)]
struct PreviewRegistry {
    revision: u64,
    sessions: BTreeMap<String, Value>,
}

fn preview_registry() -> &'static StdMutex<PreviewRegistry> {
    static REGISTRY: OnceLock<StdMutex<PreviewRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(PreviewRegistry::default()))
}

fn server_epoch() -> &'static str {
    static EPOCH: OnceLock<String> = OnceLock::new();
    EPOCH.get_or_init(|| Uuid::new_v4().to_string())
}

#[derive(Default)]
struct DeviceStateCache {
    revision: u64,
    agent_access_enabled: bool,
    onboarding_completed: bool,
}

fn device_cache() -> &'static StdMutex<DeviceStateCache> {
    static CACHE: OnceLock<StdMutex<DeviceStateCache>> = OnceLock::new();
    CACHE.get_or_init(|| StdMutex::new(DeviceStateCache::default()))
}

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "preview.list" => preview_list(&payload),
        "preview.open" => preview_open(&payload),
        "preview.resize" => preview_resize(&payload),
        "preview.adjust"
        | "preview.reportProfiles"
        | "preview.reportStatus"
        | "preview.clearProfile"
        | "preview.stopLocalServer"
        | "peerLinks.answerRequest" => RpcExit::Success { value: json!({}) },
        "preview.close" => preview_close(state, &payload).await,
        "device.configure" => device_configure(state, &payload),
        "device.list"
        | "device.testHost"
        | "device.easAccount"
        | "device.simlockIdentity"
        | "device.open"
        | "device.close"
        | "device.shutdown"
        | "device.action" => RpcExit::Success {
            value: device_state_snapshot(state),
        },
        "device.detail" => RpcExit::Success {
            value: json!({
                "hostId": "local",
                "deviceId": payload.get("deviceId").and_then(Value::as_str).unwrap_or("local"),
                "settings": {},
                "foregroundApp": Value::Null,
                "readAt": utc_now_iso(),
            }),
        },
        "peerLinks.list" => peers_secrets_share::peer_links_list(state),
        "peerLinks.link" => peers_secrets_share::peer_links_link(&payload),
        "peerLinks.unlink" => peers_secrets_share::peer_links_unlink(state, payload),
        "secrets.answerRequest" => peers_secrets_share::answer_secret_request(state, payload),
        "guestShare.create" => peers_secrets_share::guest_share_create(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn preview_list(payload: &Value) -> RpcExit {
    let filter_thread = payload.get("threadId").and_then(Value::as_str);
    let Ok(guard) = preview_registry().lock() else {
        return RpcExit::Success {
            value: json!({ "sessions": [], "serverEpoch": server_epoch(), "revision": 0 }),
        };
    };
    let sessions: Vec<Value> = guard
        .sessions
        .values()
        .filter(|s| {
            filter_thread.is_none_or(|tid| s.get("threadId").and_then(Value::as_str) == Some(tid))
        })
        .cloned()
        .collect();
    RpcExit::Success {
        value: json!({
            "sessions": sessions,
            "serverEpoch": server_epoch(),
            "revision": guard.revision,
        }),
    }
}

fn preview_open(payload: &Value) -> RpcExit {
    let thread_id = payload
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or("thread-default")
        .to_owned();
    let tab_id = payload
        .get("tabId")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .map_or_else(
            || format!("tab-{}", &Uuid::new_v4().simple().to_string()[..8]),
            str::to_owned,
        );
    let url = payload
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("about:blank");
    let snapshot = json!({
        "threadId": thread_id,
        "tabId": tab_id,
        "navStatus": { "status": "idle", "url": url, "title": "" },
        "canGoBack": false,
        "canGoForward": false,
        "runtime": payload.get("runtime").and_then(Value::as_str).unwrap_or("server"),
        "headless": true,
        "updatedAt": utc_now_iso(),
    });
    if let Ok(mut guard) = preview_registry().lock() {
        guard.revision += 1;
        guard.sessions.insert(tab_id, snapshot.clone());
    }
    RpcExit::Success { value: snapshot }
}

fn preview_resize(payload: &Value) -> RpcExit {
    let tab_id = payload
        .get("tabId")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Ok(mut guard) = preview_registry().lock()
        && let Some(entry) = guard.sessions.get_mut(tab_id)
    {
        if let Some(vp) = payload.get("viewport") {
            entry["viewport"] = vp.clone();
        }
        entry["updatedAt"] = Value::String(utc_now_iso());
        let updated = entry.clone();
        guard.revision += 1;
        return RpcExit::Success { value: updated };
    }
    RpcExit::Success { value: json!({}) }
}

async fn preview_close(state: &AppState, payload: &Value) -> RpcExit {
    let tab_id = payload
        .get("tabId")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str);
    let thread_id = payload.get("threadId").and_then(Value::as_str);
    if let Ok(mut guard) = preview_registry().lock() {
        if let Some(id) = tab_id {
            guard.sessions.remove(id);
        } else if let Some(tid) = thread_id {
            guard
                .sessions
                .retain(|_, s| s.get("threadId").and_then(Value::as_str) != Some(tid));
        }
        guard.revision += 1;
    }
    state.runtime_http.preview_browser().close().await;
    RpcExit::Success { value: json!({}) }
}

fn device_configure(state: &AppState, payload: &Value) -> RpcExit {
    if let Ok(mut guard) = device_cache().lock() {
        if let Some(enabled) = payload.get("agentAccessEnabled").and_then(Value::as_bool) {
            guard.agent_access_enabled = enabled;
        }
        if let Some(done) = payload.get("onboardingCompleted").and_then(Value::as_bool) {
            guard.onboarding_completed = done;
        }
        guard.revision += 1;
    }
    RpcExit::Success {
        value: device_state_snapshot(state),
    }
}

fn device_state_snapshot(state: &AppState) -> Value {
    let (revision, agent_access_enabled, onboarding_completed) =
        device_cache().lock().map_or((0, false, false), |g| {
            (g.revision, g.agent_access_enabled, g.onboarding_completed)
        });
    json!({
        "hosts": [{
            "id": "local",
            "label": "This machine",
            "kind": "local",
            "platforms": [
                { "platform": "ios", "available": cfg!(target_os = "macos") },
                { "platform": "android", "available": true },
            ],
            "hubInstalled": false,
            "agentDeviceInstalled": false,
        }],
        "easHosts": [],
        "simlockHosts": [],
        "hostStatus": "idle",
        "hostStatuses": { "local": { "status": "idle" } },
        "devices": [],
        "sessions": [],
        "onboardingCompleted": onboarding_completed,
        "agentAccessEnabled": agent_access_enabled,
        "hubBasePath": state.paths.state_dir.join("device-hub").to_string_lossy(),
        "revision": revision,
    })
}
