//! Preview browser and native mobile device manager RPCs (`preview.*`, `device.*`) plus routing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, peers_secrets_share, rpc_fail, rpc_fail_obj, utc_now_iso};

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
        "preview.adjust" => preview_adjust(&payload),
        "preview.reportProfiles"
        | "preview.reportStatus"
        | "preview.clearProfile"
        | "preview.stopLocalServer"
        | "peerLinks.answerRequest"
        | "device.close"
        | "device.shutdown" => RpcExit::Success { value: Value::Null },
        "preview.close" => preview_close(state, &payload).await,
        "device.configure" => device_configure(state, &payload),
        "device.list" => RpcExit::Success {
            value: device_state_snapshot(state),
        },
        "device.testHost" => device_test_host(&payload),
        "device.easAccount" => rpc_fail_obj(json!({
            "_tag": "DeviceEasAccountError",
            "reason": "Expo account credentials are not configured.",
        })),
        "device.simlockIdentity" => rpc_fail_obj(json!({
            "_tag": "DeviceSimlockIdentityError",
            "reason": "Simlock identity is not configured.",
        })),
        "device.open" => device_open(&payload),
        "device.detail" | "device.action" => RpcExit::Success {
            value: json!({
                "hostId": payload.get("hostId").and_then(Value::as_str).unwrap_or("local"),
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

fn build_preview_snapshot(payload: &Value, thread_id: &str, tab_id: &str) -> Value {
    let url = payload
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let nav_status = match url {
        Some(u) => json!({ "_tag": "Success", "url": u, "title": "" }),
        None => json!({ "_tag": "Idle" }),
    };
    let viewport = payload
        .get("viewport")
        .filter(|v| v.get("_tag").is_some())
        .cloned()
        .unwrap_or_else(|| json!({ "_tag": "freeform", "width": 1280, "height": 800 }));
    json!({
        "threadId": thread_id,
        "tabId": tab_id,
        "navStatus": nav_status,
        "canGoBack": false,
        "canGoForward": false,
        "viewport": viewport,
        "colorScheme": payload.get("colorScheme").and_then(Value::as_str).unwrap_or("system"),
        "zoomFactor": payload.get("zoomFactor").and_then(Value::as_f64).unwrap_or(1.0),
        "profileId": payload.get("profileId").and_then(Value::as_str).unwrap_or("default"),
        "runtime": payload.get("runtime").and_then(Value::as_str).unwrap_or("server"),
        "headless": true,
        "automationOwner": Value::Null,
        "reveal": false,
        "updatedAt": utc_now_iso(),
    })
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
    let snapshot = build_preview_snapshot(payload, &thread_id, &tab_id);
    if let Ok(mut guard) = preview_registry().lock() {
        guard.revision += 1;
        guard.sessions.insert(tab_id, snapshot.clone());
    }
    RpcExit::Success { value: snapshot }
}

fn preview_resize(payload: &Value) -> RpcExit {
    let thread_id = payload
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or("thread-default")
        .to_owned();
    let tab_id = payload
        .get("tabId")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or("default");
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
    RpcExit::Success {
        value: build_preview_snapshot(payload, &thread_id, tab_id),
    }
}

fn preview_adjust(payload: &Value) -> RpcExit {
    let thread_id = payload
        .get("threadId")
        .and_then(Value::as_str)
        .unwrap_or("thread-default")
        .to_owned();
    let tab_id = payload
        .get("tabId")
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .unwrap_or("default");
    if let Ok(mut guard) = preview_registry().lock()
        && let Some(entry) = guard.sessions.get_mut(tab_id)
    {
        if let Some(cs) = payload.get("colorScheme") {
            entry["colorScheme"] = cs.clone();
        }
        if let Some(zf) = payload.get("zoomFactor") {
            entry["zoomFactor"] = zf.clone();
        }
        entry["updatedAt"] = Value::String(utc_now_iso());
        let updated = entry.clone();
        guard.revision += 1;
        return RpcExit::Success { value: updated };
    }
    RpcExit::Success {
        value: build_preview_snapshot(payload, &thread_id, tab_id),
    }
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
    RpcExit::Success { value: Value::Null }
}

fn device_test_host(payload: &Value) -> RpcExit {
    RpcExit::Success {
        value: json!({
            "id": payload.get("id").and_then(Value::as_str).unwrap_or("local"),
            "label": payload.get("label").and_then(Value::as_str).unwrap_or("This machine"),
            "kind": payload.get("kind").and_then(Value::as_str).unwrap_or("local"),
            "platforms": [
                { "platform": "ios", "available": cfg!(target_os = "macos") },
                { "platform": "android", "available": true },
            ],
            "hubInstalled": false,
            "agentDeviceInstalled": false,
        }),
    }
}

fn device_open(payload: &Value) -> RpcExit {
    RpcExit::Success {
        value: json!({
            "threadId": payload.get("threadId").and_then(Value::as_str).unwrap_or("thread-default"),
            "hostId": payload.get("hostId").and_then(Value::as_str).unwrap_or("local"),
            "deviceId": payload.get("deviceId").and_then(Value::as_str).unwrap_or("local"),
            "platform": payload.get("platform").and_then(Value::as_str).unwrap_or("android"),
            "openedAt": utc_now_iso(),
        }),
    }
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

pub(crate) fn device_state_snapshot(state: &AppState) -> Value {
    let (revision, agent_access_enabled, onboarding_completed) =
        device_cache().lock().map_or((0, false, false), |g| {
            (g.revision, g.agent_access_enabled, g.onboarding_completed)
        });
    json!({
        "supportsHostRetry": false,
        "supportsToolUpdate": false,
        "supportsToolInspection": false,
        "supportsEasHosts": false,
        "supportsSimlockHosts": false,
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
        "bootingDevices": [],
        "onboardingCompleted": onboarding_completed,
        "agentAccessEnabled": agent_access_enabled,
        "hubBasePath": state.paths.state_dir.join("device-hub").to_string_lossy(),
        "revision": revision,
    })
}
