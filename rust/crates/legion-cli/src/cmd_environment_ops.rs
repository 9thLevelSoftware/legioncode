//! Live HTTP peer operations for `legioncode environment link|list|unlink`.

use std::collections::HashSet;
use std::env;
use std::time::{Duration, SystemTime};

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::McpClientAccess;
use legion_persistence::ownership::format_epoch_millis_utc;
use legion_persistence::paths::{
    ServerDerivedPaths, ensure_environment_id, ensure_server_directories,
};
use legion_persistence::sqlite::SqliteStore;
use legion_remote::live_network::LivePeerClient;
use legion_remote::peer::links::{
    InsertPeerLinkInput, PeerLink, PeerLinksService, format_peer_links, format_peer_links_json,
    format_peer_unlink_result, validate_peer_origin,
};
use legion_remote::secrets::FileSecretStore;
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::cmd_auth_ops::derive_auth_paths;
use crate::cmd_trace::current_epoch_ms;
use crate::fallback::write_stdout_line;

pub(crate) struct EnvLinkArgs {
    pub(crate) base_dir: Option<String>,
    pub(crate) dev_url: Option<String>,
    pub(crate) url: String,
    pub(crate) alternate_urls: Vec<String>,
    pub(crate) pairing_code: String,
    pub(crate) access: McpClientAccess,
}

fn format_access(access: McpClientAccess) -> &'static str {
    match access {
        McpClientAccess::ReadOnly => "read-only",
        McpClientAccess::ApprovalRequired => "approval-required",
        McpClientAccess::AutoAcceptEdits => "auto-accept-edits",
        McpClientAccess::Auto => "auto",
        McpClientAccess::FullAccess => "full-access",
    }
}

fn open_peer_storage(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
) -> Result<(ServerDerivedPaths, SqliteStore, FileSecretStore), i32> {
    let paths = derive_auth_paths(base_dir, dev_url);
    if let Err(err) = ensure_server_directories(&paths) {
        eprintln!("Failed to prepare environment directories: {err}");
        return Err(1);
    }
    let store = SqliteStore::open(&paths.db_path).map_err(|err| {
        eprintln!("{err}");
        1
    })?;
    let secrets = FileSecretStore::new(&paths.secrets_dir).map_err(|err| {
        eprintln!("{err}");
        1
    })?;
    Ok((paths, store, secrets))
}

pub(crate) fn run_link(args: &EnvLinkArgs) -> i32 {
    let Ok((paths, store, secrets)) =
        open_peer_storage(args.base_dir.as_deref(), args.dev_url.as_deref())
    else {
        return 1;
    };
    let self_env_id = ensure_environment_id(&paths.environment_id_path)
        .ok()
        .and_then(|raw| EnvironmentId::new(raw).ok());
    let client_label = env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "LegionCode CLI".to_owned());

    let linked_at = format_epoch_millis_utc(current_epoch_ms());
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `legioncode environment link`.");
        return 1;
    };
    let linked = match rt.block_on(link_peer_over_http(
        store.connection(),
        &secrets,
        args,
        self_env_id.as_ref(),
        &client_label,
        &linked_at,
    )) {
        Ok(link) => link,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    write_stdout_line(
        &format!(
            "Linked {} ({}) with {} access, until {}.\n",
            linked.label,
            linked.environment_id,
            format_access(linked.access),
            linked.expires_at
        ),
        "environment output",
    )
}

async fn link_peer_over_http(
    conn: &Connection,
    secrets: &FileSecretStore,
    args: &EnvLinkArgs,
    self_env_id: Option<&EnvironmentId>,
    client_label: &str,
    linked_at: &str,
) -> Result<PeerLink, String> {
    let client = LivePeerClient::new().map_err(|err| err.to_string())?;
    let (primary_origin, descriptor) = client
        .probe_environment(&args.url)
        .await
        .map_err(|err| err.to_string())?;
    let mut origins = vec![primary_origin.clone()];
    let mut seen = HashSet::from([primary_origin.clone()]);
    for alt in &args.alternate_urls {
        let alt_origin = validate_peer_origin(alt).map_err(|err| err.to_string())?;
        if seen.insert(alt_origin.clone()) {
            origins.push(alt_origin);
        }
    }
    if self_env_id == Some(&descriptor.environment_id) {
        return Err("That address is this environment.".to_owned());
    }
    if descriptor.capabilities.mcp_mode_limit_header != Some(true)
        || descriptor.capabilities.link_fence != Some(true)
    {
        return Err(format!(
            "{} runs a LegionCode version that cannot keep a linked agent's limits. Update it, then link again.",
            descriptor.label
        ));
    }

    let service = PeerLinksService::new(secrets.clone());
    if let Some(existing) = service
        .get(conn, descriptor.environment_id.as_str())
        .map_err(|err| err.to_string())?
        && existing.expires_at.as_str() > linked_at
        && !existing.urls.contains(&primary_origin)
    {
        return Err(format!(
            "{primary_origin} answers as {}, which is already linked at another address. If {} moved, forget that link first.",
            existing.label, existing.label
        ));
    }

    let (access_token, expires_at) =
        exchange_peer_pair_token(&primary_origin, &args.pairing_code, client_label).await?;
    service
        .insert(
            conn,
            InsertPeerLinkInput {
                environment_id: descriptor.environment_id,
                label: descriptor.label,
                urls: origins,
                access: args.access,
                access_token,
                linked_at: linked_at.to_owned(),
                expires_at,
            },
        )
        .map_err(|err| err.to_string())
}

async fn exchange_peer_pair_token(
    origin: &str,
    pairing_code: &str,
    client_label: &str,
) -> Result<(String, String), String> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| format!("Failed to initialize peer HTTP client: {err}"))?;
    let resp = http
        .post(format!("{origin}/api/auth/pair"))
        .header(ACCEPT, "application/json")
        .json(&json!({ "token": pairing_code, "clientLabel": client_label }))
        .send()
        .await
        .map_err(|err| format!("{origin} stopped answering while linking: {err}"))?;
    let status = resp.status();
    if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        let detail = resp.text().await.unwrap_or_default();
        return Err(if detail.trim().is_empty() {
            "The peer refused the pairing code.".to_owned()
        } else {
            detail
        });
    }
    if !status.is_success() {
        return Err(format!(
            "{origin} returned HTTP {} during pairing.",
            status.as_u16()
        ));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|_| format!("{origin} answered in a shape this version does not understand."))?;
    let access_token = body
        .get("accessToken")
        .or_else(|| body.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{origin} answered in a shape this version does not understand."))?
        .to_owned();
    let expires_at = body
        .get("expiresAt")
        .or_else(|| body.get("expires_at"))
        .and_then(Value::as_str)
        .map_or_else(
            || {
                let secs = body
                    .get("expiresIn")
                    .or_else(|| body.get("expires_in"))
                    .and_then(Value::as_i64)
                    .unwrap_or(3_600)
                    .max(1);
                format_epoch_millis_utc(
                    current_epoch_ms().saturating_add(secs.saturating_mul(1_000)),
                )
            },
            str::to_owned,
        );
    Ok((access_token, expires_at))
}

pub(crate) fn run_list(base_dir: Option<&str>, dev_url: Option<&str>, json: bool) -> i32 {
    let Ok((_, store, secrets)) = open_peer_storage(base_dir, dev_url) else {
        return 1;
    };
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0_i64, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let now_iso = format_epoch_millis_utc(now_ms);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `legioncode environment list`.");
        return 1;
    };
    let client = match LivePeerClient::new() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let summaries = match rt.block_on(client.list_live_peer_summaries(
        store.connection(),
        &secrets,
        &now_iso,
    )) {
        Ok(list) => list,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    if json {
        match format_peer_links_json(&summaries) {
            Ok(out) => write_stdout_line(&out, "environment output"),
            Err(err) => {
                eprintln!("{err}");
                1
            }
        }
    } else {
        write_stdout_line(&format_peer_links(&summaries), "environment output")
    }
}

pub(crate) fn run_unlink(
    base_dir: Option<&str>,
    dev_url: Option<&str>,
    environment_id: &str,
) -> i32 {
    let trimmed = environment_id.trim();
    if trimmed.is_empty() {
        eprintln!("Environment id cannot be empty.");
        return 1;
    }
    let Ok((_, store, secrets)) = open_peer_storage(base_dir, dev_url) else {
        return 1;
    };
    let service = PeerLinksService::new(secrets);
    match service.unlink(store.connection(), trimmed) {
        Ok(removed) => write_stdout_line(
            &format_peer_unlink_result(trimmed, removed),
            "environment output",
        ),
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}
