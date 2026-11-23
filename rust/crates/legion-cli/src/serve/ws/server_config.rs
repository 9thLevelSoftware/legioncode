//! Server configuration and lifecycle readiness WebSocket streams.

use std::path::Path;
use std::time::{Duration, SystemTime};

use legion_orchestrator::time::now_iso_utc;
use legion_protocol::rpc_codec::RpcExit;
use serde_json::{Value, json};

use super::auth_access::stream_auth_access;
use super::stream_ctx::StreamContext;
use crate::serve::ws_mcp::rpc_fail;

const CONFIG_WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Returns `true` if `tag` belongs to server-config, lifecycle, or auth-access streams.
pub(super) fn handles_tag(tag: &str) -> bool {
    matches!(
        tag,
        "subscribeServerConfig" | "subscribeServerLifecycle" | "subscribeAuthAccess"
    )
}

/// Dispatches server-config, lifecycle, or auth-access streaming RPCs.
pub(super) async fn dispatch_stream(ctx: &StreamContext, tag: &str, _payload: Value) -> RpcExit {
    match tag {
        "subscribeServerConfig" => stream_server_config(ctx).await,
        "subscribeServerLifecycle" => stream_server_lifecycle(ctx).await,
        "subscribeAuthAccess" => stream_auth_access(ctx).await,
        other => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{other}`"),
        ),
    }
}

async fn stream_server_config(ctx: &StreamContext) -> RpcExit {
    let config = build_server_config(ctx).await;
    let snapshot = json!({
        "version": 1,
        "type": "snapshot",
        "config": config,
    });
    if !ctx.send_chunk(snapshot).await {
        return RpcExit::Success { value: Value::Null };
    }

    let mut settings_mtime = file_mtime(&ctx.state.paths.settings_path);
    let mut keybindings_mtime = file_mtime(&ctx.state.paths.keybindings_config_path);
    let mut ticker = tokio::time::interval(CONFIG_WATCH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    loop {
        ticker.tick().await;
        let next_settings = file_mtime(&ctx.state.paths.settings_path);
        if next_settings != settings_mtime {
            settings_mtime = next_settings;
            let settings = load_settings_json(&ctx.state.paths.settings_path);
            let event = json!({
                "version": 1,
                "type": "settingsUpdated",
                "payload": { "settings": settings },
            });
            if !ctx.send_chunk(event).await {
                break;
            }
        }

        let next_kb = file_mtime(&ctx.state.paths.keybindings_config_path);
        if next_kb != keybindings_mtime {
            keybindings_mtime = next_kb;
            let (keybindings, issues) = load_keybindings(&ctx.state.paths.keybindings_config_path);
            let event = json!({
                "version": 1,
                "type": "keybindingsUpdated",
                "payload": { "keybindings": keybindings, "issues": issues },
            });
            if !ctx.send_chunk(event).await {
                break;
            }
        }
    }
    RpcExit::Success { value: Value::Null }
}

async fn build_server_config(ctx: &StreamContext) -> Value {
    let state = &ctx.state;
    let providers = crate::serve::rpc::server_config::build_server_providers_list(state).await;
    let cwd = std::env::current_dir().map_or_else(|_| ".".to_owned(), |p| p.display().to_string());
    let kb_path = state.paths.keybindings_config_path.display().to_string();
    let (keybindings, issues) = load_keybindings(&state.paths.keybindings_config_path);
    let settings = load_settings_json(&state.paths.settings_path);

    json!({
        "environment": state.descriptor,
        "auth": state.runtime_http.auth_descriptor_json(),
        "cwd": non_empty_str(&cwd, "."),
        "keybindingsConfigPath": non_empty_str(&kb_path, "keybindings.json"),
        "keybindings": keybindings,
        "issues": issues,
        "providers": providers,
        "availableEditors": detect_available_editors(),
        "observability": {
            "logsDirectoryPath": non_empty_str(&state.paths.logs_dir.display().to_string(), "logs"),
            "localTracingEnabled": true,
            "otlpTracesEnabled": false,
            "otlpMetricsEnabled": false,
            "otlpLogsEnabled": false,
        },
        "settings": settings,
        "shellThreadFamilies": true,
        "shellOlderSettled": true,
        "threadSnapshotPagination": true,
        "serverTime": now_iso_utc(),
    })
}

async fn stream_server_lifecycle(ctx: &StreamContext) -> RpcExit {
    let cwd = std::env::current_dir().map_or_else(|_| ".".to_owned(), |p| p.display().to_string());
    let project_name = std::path::Path::new(&cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("workspace")
        .to_owned();

    let welcome = json!({
        "version": 1,
        "sequence": 0,
        "type": "welcome",
        "payload": {
            "environment": ctx.state.descriptor,
            "cwd": non_empty_str(&cwd, "."),
            "projectName": project_name,
        },
    });
    if !ctx.send_chunk(welcome).await {
        return RpcExit::Success { value: Value::Null };
    }

    let ready = json!({
        "version": 1,
        "sequence": 1,
        "type": "ready",
        "payload": {
            "at": now_iso_utc(),
            "environment": ctx.state.descriptor,
        },
    });
    if ctx.send_chunk(ready).await {
        std::future::pending::<()>().await;
    }
    RpcExit::Success { value: Value::Null }
}

fn non_empty_str(s: &str, fallback: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn load_settings_json(path: &Path) -> Value {
    let mut val =
        crate::serve::rpc::server_settings::read_settings_json(path).unwrap_or_else(|_| json!({}));
    if let Some(speech) = val
        .get_mut("cloudSpeechCredentials")
        .and_then(Value::as_object_mut)
    {
        for v in speech.values_mut() {
            if let Some(s) = v.as_str()
                && !s.is_empty()
            {
                *v = Value::String(String::new());
            }
        }
    }
    val
}

fn load_keybindings(path: &Path) -> (Vec<Value>, Vec<Value>) {
    crate::serve::rpc::server_settings::read_keybindings_with_issues(path)
}

fn detect_available_editors() -> Vec<&'static str> {
    const CANDIDATES: &[(&str, &str)] = &[
        ("cursor", "cursor"),
        ("vscode", "code"),
        ("vscode-insiders", "code-insiders"),
        ("vscodium", "codium"),
        ("zed", "zed"),
        ("antigravity", "antigravity-ide"),
    ];
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let dirs: Vec<_> = std::env::split_paths(&path_var).collect();
    let mut found = Vec::new();
    for (id, cmd) in CANDIDATES {
        let present = dirs.iter().any(|dir| {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                return true;
            }
            cfg!(windows) && dir.join(format!("{cmd}.cmd")).is_file()
                || cfg!(windows) && dir.join(format!("{cmd}.exe")).is_file()
        });
        if present {
            found.push(*id);
        }
    }
    found
}
