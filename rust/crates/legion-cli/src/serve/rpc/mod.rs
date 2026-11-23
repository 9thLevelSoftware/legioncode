//! Modular Safe Rust WebSocket RPC dispatchers covering every unary method in
//! `WS_METHODS` and `ORCHESTRATION_V2_WS_METHODS` (`@legioncode/contracts`).

mod agent_sessions;
mod assets_attachments;
mod assets_paths;
mod fs_browse;
pub(crate) mod git_actions;
mod mcp_apps;
mod orchestration;
mod orchestration_commands;
mod orchestration_diff;
mod orchestration_launch;
mod orchestration_queries;
mod orchestration_search;
mod peers_secrets_share;
pub(crate) mod preview_device_peers;
mod projects_create;
mod projects_files;
mod projects_fs;
mod projects_search;
mod providers_acp;
mod providers_usage;
mod scm_hosts_clone;
mod scm_issues;
mod scm_prs;
mod scm_prs_actions;
mod scm_prs_issues;
pub(crate) mod server_config;
mod server_ops;
pub(crate) mod server_settings;
mod server_telemetry;
mod server_updates;
mod tasks_sessions_mcp;
mod tasks_webhooks_mcp;
mod vcs_git;
mod vcs_review;
mod workspace_walk;

#[cfg(test)]
mod tests_core;
#[cfg(test)]
mod tests_extra;
#[cfg(test)]
mod tests_orch;
#[cfg(test)]
mod tests_scm_ops;

use std::sync::Arc;
use std::time::SystemTime;

use legion_persistence::ownership::format_iso8601_utc;
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::AppState;

/// Dispatches a single unary WebSocket RPC call against live `legioncode serve` state.
pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    if let Some(method) = tag
        .strip_prefix("orchestration.v2.")
        .or_else(|| tag.strip_prefix("orchestration."))
    {
        return orchestration::dispatch(state, method, payload).await;
    }

    match tag {
        "providers.listDrivers"
        | "checkpoints.getDiff"
        | "diagnostics.traceSummary"
        | "themes.read" => super::rpc_wave_d::dispatch(state, tag, payload),
        "resources.sample" | "surfaces.decorateDiff" | "surfaces.mobileChrome" => {
            super::rpc_wave_h::dispatch(tag, payload)
        }
        "terminal.open" | "terminal.write" | "terminal.resize" | "terminal.clear"
        | "terminal.close" | "terminal.snapshot" => {
            super::rpc_terminal::dispatch(&state.terminal_runtime, tag, payload)
        }
        _ if tag.starts_with("projects.")
            || tag.starts_with("filesystem.")
            || tag == "shell.openInEditor" =>
        {
            projects_fs::dispatch(state, tag, payload).await
        }
        _ if tag.starts_with("assets.") || tag.starts_with("attachments.") => {
            assets_attachments::dispatch(state, tag, payload)
        }
        "server.probe" | "server.getConfig" => server_config::dispatch(state, tag, payload).await,
        "server.getSettings"
        | "server.updateSettings"
        | "server.upsertKeybinding"
        | "server.removeKeybinding" => server_settings::dispatch(state, tag, payload),
        "server.refreshProviders"
        | "server.updateProvider"
        | "server.getUsageSummary"
        | "server.refreshUsageRates" => providers_usage::dispatch(state, tag, payload).await,
        _ if tag.starts_with("server.") && tag.contains("AcpRegistry") => {
            providers_acp::dispatch(state, tag, payload).await
        }
        _ if tag.starts_with("provider.") || tag.starts_with("cloud.") => {
            providers_usage::dispatch(state, tag, payload).await
        }
        _ if tag.starts_with("server.") => server_ops::dispatch(state, tag, payload).await,
        _ if tag.starts_with("vcs.")
            || tag.starts_with("worktreeSetup.")
            || tag.starts_with("git.")
            || tag.starts_with("review.") =>
        {
            vcs_git::dispatch(state, tag, payload).await
        }
        _ if tag.starts_with("pullRequests.")
            || tag.starts_with("issues.")
            || tag.starts_with("issueTrackers.")
            || tag.starts_with("sourceControl.")
            || tag.starts_with("projectClone.") =>
        {
            scm_prs_issues::dispatch(state, tag, payload).await
        }
        _ if tag.starts_with("scheduledTasks.")
            || tag.starts_with("agentSessions.")
            || tag.starts_with("mcpApps.") =>
        {
            tasks_sessions_mcp::dispatch(state, tag, payload).await
        }
        _ if tag.starts_with("preview.")
            || tag.starts_with("device.")
            || tag.starts_with("peerLinks.")
            || tag == "secrets.answerRequest"
            || tag == "guestShare.create" =>
        {
            preview_device_peers::dispatch(state, tag, payload).await
        }
        _ => rpc_fail(
            "UnsupportedRpcMethod",
            &format!("Unsupported RPC method `{tag}`"),
        ),
    }
}

pub(crate) fn rpc_fail(tag: &str, message: &str) -> RpcExit {
    RpcExit::Failure {
        cause: vec![RpcCauseReason::Fail {
            error: json!({
                "_tag": tag,
                "message": message,
            }),
        }],
    }
}

pub(crate) fn rpc_fail_obj(error: Value) -> RpcExit {
    RpcExit::Failure {
        cause: vec![RpcCauseReason::Fail { error }],
    }
}

pub(crate) fn parse_payload<T: DeserializeOwned>(payload: Value) -> Result<T, RpcExit> {
    serde_json::from_value(payload).map_err(|err| rpc_fail("ParseError", &err.to_string()))
}

pub(crate) fn parse_optional_payload<T: Default + DeserializeOwned>(
    payload: Value,
) -> Result<T, RpcExit> {
    if payload.is_null() {
        Ok(T::default())
    } else {
        parse_payload(payload)
    }
}

pub(crate) fn with_db<F, T>(state: &AppState, op: F) -> Result<T, RpcExit>
where
    F: FnOnce(&Connection) -> Result<T, RpcExit>,
{
    let guard = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| rpc_fail("SqliteError", "database mutex poisoned"))?;
    op(&guard)
}

pub(crate) fn with_db_mut<F, T>(state: &AppState, op: F) -> Result<T, RpcExit>
where
    F: FnOnce(&mut Connection) -> Result<T, RpcExit>,
{
    let mut guard = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| rpc_fail("SqliteError", "database mutex poisoned"))?;
    op(&mut guard)
}

pub(crate) fn utc_now_iso() -> String {
    format_iso8601_utc(SystemTime::now())
}

pub(crate) fn base64url_encode(input: &[u8]) -> String {
    let std_b64 = super::rpc_wave_h::encode_standard_base64(input);
    std_b64
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

pub(crate) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut k_pad = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        k_pad[..32].copy_from_slice(&hashed);
    } else {
        k_pad[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36_u8; BLOCK_SIZE];
    let mut opad = [0x5c_u8; BLOCK_SIZE];
    for idx in 0..BLOCK_SIZE {
        ipad[idx] ^= k_pad[idx];
        opad[idx] ^= k_pad[idx];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let outer_digest = outer.finalize();
    let mut out = [0_u8; 32];
    out.copy_from_slice(&outer_digest);
    out
}
