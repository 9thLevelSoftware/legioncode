//! Atomic persistence for `settings.json` (`server.getSettings`, `server.updateSettings`)
//! and `keybindings.json` (`server.upsertKeybinding`, `server.removeKeybinding`).

use std::fs;
use std::path::Path;

use legion_protocol::rpc_codec::RpcExit;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AppState, parse_optional_payload, parse_payload, rpc_fail, rpc_fail_obj};

pub(crate) fn dispatch(state: &AppState, tag: &str, payload: Value) -> RpcExit {
    match tag {
        "server.getSettings" => get_settings(state),
        "server.updateSettings" => update_settings(state, payload),
        "server.upsertKeybinding" => upsert_keybinding(state, payload),
        "server.removeKeybinding" => remove_keybinding(state, payload),
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

fn get_settings(state: &AppState) -> RpcExit {
    match read_settings_json(&state.paths.settings_path) {
        Ok(val) => RpcExit::Success { value: val },
        Err(msg) => rpc_fail_obj(json!({
            "_tag": "ServerSettingsError",
            "settingsPath": state.paths.settings_path.to_string_lossy(),
            "detail": msg,
            "message": msg,
        })),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UpdateSettingsPayload {
    patch: Option<Value>,
}

fn update_settings(state: &AppState, payload: Value) -> RpcExit {
    let raw: UpdateSettingsPayload = match parse_optional_payload(payload.clone()) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let patch = raw.patch.unwrap_or(payload);
    let mut current = match read_settings_json(&state.paths.settings_path) {
        Ok(v) => v,
        Err(msg) => {
            return rpc_fail_obj(json!({
                "_tag": "ServerSettingsError",
                "settingsPath": state.paths.settings_path.to_string_lossy(),
                "detail": msg,
                "message": msg,
            }));
        }
    };
    deep_merge_json(&mut current, &patch);
    if let Err(msg) = write_json_atomic(&state.paths.settings_path, &current) {
        return rpc_fail_obj(json!({
            "_tag": "ServerSettingsError",
            "settingsPath": state.paths.settings_path.to_string_lossy(),
            "detail": msg,
            "message": msg,
        }));
    }
    RpcExit::Success { value: current }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RawKeybindingRule {
    key: String,
    command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    when: Option<String>,
}

fn upsert_keybinding(state: &AppState, payload: Value) -> RpcExit {
    let incoming: RawKeybindingRule = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = &state.paths.keybindings_config_path;
    let mut rules = match read_raw_keybindings(path) {
        Ok(r) => r,
        Err(detail) => return keybindings_err(path, &detail),
    };
    if let Some(existing) = rules
        .iter_mut()
        .find(|r| r.command == incoming.command && r.when == incoming.when)
    {
        *existing = incoming;
    } else {
        rules.push(incoming);
    }
    if let Err(detail) = write_json_atomic(path, &json!(rules)) {
        return keybindings_err(path, &detail);
    }
    let (resolved, issues) = read_keybindings_with_issues(path);
    RpcExit::Success {
        value: json!({ "keybindings": resolved, "issues": issues }),
    }
}

fn remove_keybinding(state: &AppState, payload: Value) -> RpcExit {
    let target: RawKeybindingRule = match parse_payload(payload) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = &state.paths.keybindings_config_path;
    let mut rules = match read_raw_keybindings(path) {
        Ok(r) => r,
        Err(detail) => return keybindings_err(path, &detail),
    };
    rules.retain(|r| {
        !(r.command == target.command
            && r.key.eq_ignore_ascii_case(&target.key)
            && r.when == target.when)
    });
    if let Err(detail) = write_json_atomic(path, &json!(rules)) {
        return keybindings_err(path, &detail);
    }
    let (resolved, issues) = read_keybindings_with_issues(path);
    RpcExit::Success {
        value: json!({ "keybindings": resolved, "issues": issues }),
    }
}

fn keybindings_err(path: &Path, detail: &str) -> RpcExit {
    rpc_fail_obj(json!({
        "_tag": "KeybindingsConfigParseError",
        "configPath": path.to_string_lossy(),
        "detail": detail,
        "message": detail,
    }))
}

pub(crate) fn read_settings_json(path: &Path) -> Result<Value, String> {
    let mut base = default_server_settings();
    match fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => {
            let parsed: Value = serde_json::from_str(&raw)
                .map_err(|e| format!("Invalid JSON in settings.json: {e}"))?;
            deep_merge_json(&mut base, &parsed);
            Ok(base)
        }
        Ok(_) => Ok(base),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(base),
        Err(err) => Err(err.to_string()),
    }
}

fn default_server_settings() -> Value {
    json!({
        "favorites": null,
        "providerModelPreferences": null,
        "worktreeCleanup": null,
        "storageCleanup": {
            "worktreeKeepWhen": "uncommitted-changes",
            "worktreeAfterDays": null,
            "worktreeSettledAfterDays": null,
            "worktreeOnMerge": false,
            "worktreeOnDelete": false,
            "worktreeUnchanged": false,
            "browserArtifactsAfterDays": null,
            "logsAfterDays": null,
        },
        "worktreesDirectory": "",
        "fastWorktreeCheckout": false,
        "previousWorktreesDirectories": [],
        "responseStreamingMode": "paragraph",
        "enableProviderUpdateChecks": true,
        "continueThreadsAfterServerUpdate": true,
        "keepAwakeWhileAgentsWork": true,
        "requireConnectDeviceApproval": false,
        "enableAgentBrowserAccess": true,
        "cloudSpeechCredentials": { "openai": "", "xai": "", "openrouter": "" },
        "providerInstances": {},
        "providers": {},
        "projectOverrides": {},
        "defaultTheme": "system",
    })
}

fn read_raw_keybindings(path: &Path) -> Result<Vec<RawKeybindingRule>, String> {
    match fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => {
            serde_json::from_str(&raw).map_err(|e| format!("Invalid keybindings JSON: {e}"))
        }
        Ok(_) => Ok(Vec::new()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err.to_string()),
    }
}

pub(crate) fn read_keybindings_with_issues(path: &Path) -> (Vec<Value>, Vec<Value>) {
    match read_raw_keybindings(path) {
        Ok(rules) => (
            rules
                .into_iter()
                .filter_map(|r| resolve_shortcut_rule(&r))
                .collect(),
            Vec::new(),
        ),
        Err(detail) => (
            Vec::new(),
            vec![json!({
                "kind": "keybindings.malformed-config",
                "message": detail,
            })],
        ),
    }
}

fn resolve_shortcut_rule(rule: &RawKeybindingRule) -> Option<Value> {
    let parts: Vec<&str> = rule.key.split('+').map(str::trim).collect();
    let key_part = (*parts.last()?).to_owned();
    let mut meta_key = false;
    let mut ctrl_key = false;
    let mut shift_key = false;
    let mut alt_key = false;
    let mut mod_key = false;
    for m in &parts[..parts.len().saturating_sub(1)] {
        match m.to_ascii_lowercase().as_str() {
            "mod" => mod_key = true,
            "meta" | "cmd" | "super" => meta_key = true,
            "ctrl" | "control" => ctrl_key = true,
            "shift" => shift_key = true,
            "alt" | "option" => alt_key = true,
            _ => {}
        }
    }
    let resolved_key = if key_part.eq_ignore_ascii_case("space") {
        " ".to_owned()
    } else {
        key_part
    };
    Some(json!({
        "command": rule.command,
        "shortcut": {
            "key": resolved_key,
            "metaKey": meta_key,
            "ctrlKey": ctrl_key,
            "shiftKey": shift_key,
            "altKey": alt_key,
            "modKey": mod_key,
        },
    }))
}

pub(crate) fn deep_merge_json(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t_map), Value::Object(p_map)) => {
            for (k, v) in p_map {
                match t_map.get_mut(k) {
                    Some(existing) => deep_merge_json(existing, v),
                    None => {
                        t_map.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

pub(crate) fn write_json_atomic(path: &Path, val: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let formatted = serde_json::to_string_pretty(val).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    fs::write(&tmp, format!("{formatted}\n")).map_err(|e| e.to_string())?;
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err.to_string());
    }
    Ok(())
}
