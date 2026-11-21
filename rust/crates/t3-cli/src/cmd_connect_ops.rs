//! Secret-store, OAuth, and disconnect operations for `t3 connect`.

use std::env;
use std::fs;

use serde_json::{Value, json};
use t3_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use t3_remote::live_network::{
    CloudCliLinkMode, CloudConnectExposure, CloudConnectManager, CloudConnectStatus,
    CloudDisconnectOptions, CloudLinkProvisionState, RelayUnlinkOutcome,
};
use t3_remote::secrets::FileSecretStore;

use crate::cmd_connect::run_device_auth;
use crate::fallback::{resolve_base_dir, write_stdout_line};
use crate::invocation::has_cloud_public_config;

const DEFAULT_RELAY_CLIENT_VERSION: &str = "2026.10.4";

pub(crate) fn read_cloud_status_flag_pair(base_dir: Option<&str>) -> Option<(bool, bool)> {
    if !has_cloud_public_config() {
        return None;
    }
    let (resolved, is_explicit) = resolve_base_dir(base_dir);
    let paths = ServerDerivedPaths::derive(&resolved, false, is_explicit);
    if !paths.secrets_dir.is_dir() {
        return Some((false, false));
    }
    let secrets = FileSecretStore::new(&paths.secrets_dir).ok()?;
    let mgr = CloudConnectManager::new(secrets).ok()?;
    let status = mgr.read_status().ok()?;
    Some((status.is_desired(), status.is_revoked()))
}

pub(crate) fn open_cloud_manager(
    base_dir: Option<&str>,
) -> Result<(ServerDerivedPaths, CloudConnectManager), i32> {
    let (resolved, is_explicit) = resolve_base_dir(base_dir);
    let paths = ServerDerivedPaths::derive(&resolved, false, is_explicit);
    if let Err(err) = ensure_server_directories(&paths) {
        eprintln!("Failed to prepare T3 Connect directories: {err}");
        return Err(1);
    }
    let secrets = FileSecretStore::new(&paths.secrets_dir).map_err(|err| {
        eprintln!("Failed to open secrets store: {err}");
        1
    })?;
    let mgr = CloudConnectManager::new(secrets).map_err(|err| {
        eprintln!("Failed to initialize T3 Connect manager: {err}");
        1
    })?;
    Ok((paths, mgr))
}

pub(crate) fn exec_status(base_dir: Option<&str>, json_out: bool, link_cmd: &str) -> i32 {
    let Ok((paths, mgr)) = open_cloud_manager(base_dir) else {
        return 1;
    };
    let status = match mgr.read_status() {
        Ok(st) => st,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let relay_client = detect_relay_client_json(&paths);
    let link_mode = match status.exposure {
        CloudConnectExposure::PublishOnly => "publish_only",
        CloudConnectExposure::Disabled | CloudConnectExposure::Managed => "managed",
    };

    if json_out {
        let payload = json!({
            "desired": status.is_desired(),
            "authenticated": status.authenticated,
            "linked": status.is_linked(),
            "revoked": status.is_revoked(),
            "cloudUserId": status.cloud_user_id,
            "relayUrl": status.relay_url,
            "publishAgentActivity": status.publish_agent_activity,
            "relayClient": relay_client,
            "linkMode": link_mode,
            "reachability": Value::Null,
        });
        return match serde_json::to_string_pretty(&payload) {
            Ok(text) => write_stdout_line(&text, "connect status"),
            Err(err) => {
                eprintln!("{err}");
                1
            }
        };
    }

    let rendered = format_human_connect_status(&status, link_cmd, &relay_client);
    write_stdout_line(&rendered, "connect status")
}

pub(crate) fn exec_login(base_dir: Option<&str>, headless: bool) -> i32 {
    let Ok((_, mgr)) = open_cloud_manager(base_dir) else {
        return 1;
    };
    if let Ok(Some(tok)) = mgr.load_oauth_token() {
        let suffix = tok
            .identity
            .as_deref()
            .map_or(String::new(), |id| format!(" as {id}"));
        return write_stdout_line(&format!("\u{2713} Signed in{suffix}"), "connect login");
    }
    run_device_auth(&mgr, headless)
}

pub(crate) fn exec_link(base_dir: Option<&str>, headless: bool, publish_only: bool) -> i32 {
    let Ok((_, mgr)) = open_cloud_manager(base_dir) else {
        return 1;
    };
    let existing = match mgr.load_oauth_token() {
        Ok(tok) => tok,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if existing.is_none() {
        let code = run_device_auth(&mgr, headless);
        if code != 0 {
            return code;
        }
    }
    let mode = if publish_only {
        CloudCliLinkMode::PublishOnly
    } else {
        CloudCliLinkMode::Managed
    };
    if let Err(err) = mgr.set_desired_link(true, mode) {
        eprintln!("{err}");
        return 1;
    }
    let _ = mgr.drop_revoked_link();
    if publish_only {
        let _ = mgr.set_publish_agent_activity(true);
    }
    let msg = if publish_only {
        "\u{2713} Authorized\n\nNext\n  Start T3 to publish agent activity (no managed tunnel)."
    } else {
        "\u{2713} Authorized\n\nNext\n  Start the server with `t3 serve` to make this machine reachable."
    };
    write_stdout_line(msg, "connect link")
}

pub(crate) fn exec_publish(base_dir: Option<&str>, disable: bool) -> i32 {
    let Ok((_, mgr)) = open_cloud_manager(base_dir) else {
        return 1;
    };
    let enabled = !disable;
    if let Err(err) = mgr.set_publish_agent_activity(enabled) {
        eprintln!("{err}");
        return 1;
    }
    let msg = if enabled {
        "Publishing agent activity enabled."
    } else {
        "Publishing agent activity disabled."
    };
    write_stdout_line(msg, "connect publish")
}

pub(crate) fn exec_disconnect(base_dir: Option<&str>, clear_authorization: bool) -> i32 {
    let Ok((paths, mgr)) = open_cloud_manager(base_dir) else {
        return 1;
    };
    let environment_id = fs::read_to_string(&paths.environment_id_path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let relay_url_override = resolve_relay_url_override();
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `t3 connect`.");
        return 1;
    };
    let report = match rt.block_on(mgr.disconnect(CloudDisconnectOptions {
        clear_authorization,
        environment_id,
        relay_url_override,
    })) {
        Ok(rep) => rep,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    println!("T3 Connect is disabled locally.");
    if matches!(report.relay_outcome, RelayUnlinkOutcome::Revoked) {
        println!("Revoked the relay-side environment record.");
    }
    if report.cleared_authorization {
        println!(
            "Signed out of T3 Connect locally.\nThe background service is managed separately with `t3 service`."
        );
    }
    0
}

fn detect_relay_client_json(paths: &ServerDerivedPaths) -> Value {
    let bin_name = if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    if let Ok(override_path) = env::var("T3CODE_CLOUDFLARED_PATH") {
        let trimmed = override_path.trim();
        if !trimmed.is_empty() && std::path::Path::new(trimmed).is_file() {
            return json!({
                "status": "available",
                "source": "configured override",
                "executablePath": trimmed,
                "version": DEFAULT_RELAY_CLIENT_VERSION,
            });
        }
    }
    let managed = paths.state_dir.join("relay").join(bin_name);
    if managed.is_file() {
        return json!({
            "status": "available",
            "source": "managed",
            "executablePath": managed,
            "version": DEFAULT_RELAY_CLIENT_VERSION,
        });
    }
    json!({ "status": "missing", "version": DEFAULT_RELAY_CLIENT_VERSION })
}

fn format_human_connect_status(
    status: &CloudConnectStatus,
    link_cmd: &str,
    relay_client: &Value,
) -> String {
    let desired = status.is_desired();
    let publish_only = desired && status.exposure == CloudConnectExposure::PublishOnly;
    let provisioned = match status.link_state {
        CloudLinkProvisionState::Revoked => "removed from T3 Connect",
        CloudLinkProvisionState::Provisioned => "provisioned",
        CloudLinkProvisionState::NotProvisioned => {
            if desired && status.authenticated {
                "pending server startup"
            } else {
                "not provisioned"
            }
        }
    };
    let exposure_str = if publish_only {
        "publish-only"
    } else if desired {
        "enabled"
    } else {
        "disabled"
    };
    let mut lines = vec![
        "T3 Connect".to_owned(),
        format!("  Exposure: {exposure_str}"),
        format!(
            "  Authorization: {}",
            if status.authenticated {
                "stored credential"
            } else {
                "missing"
            }
        ),
        format!("  Environment link: {provisioned}"),
        format!(
            "  Relay: {}",
            status.relay_url.as_deref().unwrap_or("not provisioned")
        ),
        format!(
            "  Publish agent activity: {}",
            if status.publish_agent_activity {
                "enabled"
            } else {
                "disabled"
            }
        ),
        format!(
            "  Relay client: {}",
            if relay_client.get("status").and_then(Value::as_str) == Some("available") {
                "available"
            } else {
                "not installed"
            }
        ),
    ];
    if !status.authenticated || status.is_revoked() || !desired {
        lines.push(String::new());
        lines.push(format!("Next: Run `{link_cmd}` to enable T3 Connect."));
    }
    lines.join("\n")
}

fn resolve_relay_url_override() -> Option<String> {
    if let Ok(raw) = env::var("T3CODE_RELAY_URL") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    let cfg_raw = env::var("T3CODE_CLOUD_PUBLIC_CONFIG").ok()?;
    let val: Value = serde_json::from_str(&cfg_raw).ok()?;
    val.get("relayUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
