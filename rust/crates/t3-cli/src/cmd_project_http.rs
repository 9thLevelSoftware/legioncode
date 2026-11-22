//! HTTP helpers and bearer resolution for live/remote `t3 project` commands.

use std::path::Path;

use reqwest::header::AUTHORIZATION;
use serde_json::Value;
use t3_contracts::brand::ACTIVE_BRAND;
use t3_persistence::paths::ServerDerivedPaths;
use t3_server_runtime::{HttpProject, HttpProjectSnapshot};

use crate::cmd_pair::exchange_local_bearer;
use crate::cmd_project_args::normalize_workspace_root;

pub(super) async fn resolve_bearer_token(
    paths: Option<&ServerDerivedPaths>,
    origin: &str,
    explicit_token: Option<&str>,
) -> Result<String, String> {
    if let Some(tok) = explicit_token.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(tok.to_owned());
    }
    if let Some(val) = ACTIVE_BRAND
        .read_env("TOKEN")
        .or_else(|| ACTIVE_BRAND.read_env("BEARER_TOKEN"))
    {
        return Ok(val.trim().to_owned());
    }
    if let Some(local_paths) = paths {
        return exchange_local_bearer(&local_paths.db_path, origin, "t3 project cli").await;
    }
    Err(format!(
        "--remote-url requires --token <bearer> (or {}TOKEN).",
        ACTIVE_BRAND.primary_env_prefix
    ))
}

pub(super) fn find_active_project<'a>(
    snapshot: &'a HttpProjectSnapshot,
    trimmed_id: &str,
) -> Option<&'a HttpProject> {
    if let Some(exact) = snapshot
        .projects
        .iter()
        .find(|p| p.deleted_at.is_none() && (p.id == trimmed_id || p.workspace_root == trimmed_id))
    {
        return Some(exact);
    }
    let normalized = normalize_workspace_root(trimmed_id, false).ok()?;
    snapshot
        .projects
        .iter()
        .find(|p| p.deleted_at.is_none() && p.workspace_root == normalized)
}

pub(super) fn derive_default_title(ws_root: &str) -> String {
    Path::new(ws_root)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("project")
        .to_owned()
}

pub(super) async fn fetch_snapshot(
    http: &reqwest::Client,
    origin: &str,
    bearer: &str,
) -> Result<HttpProjectSnapshot, String> {
    let url = format!("{}/api/projects", origin.trim_end_matches('/'));
    let resp = http
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .send()
        .await
        .map_err(|err| format!("Could not reach T3 Code server at {origin}: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "GET /api/projects failed with HTTP {}.",
            resp.status().as_u16()
        ));
    }
    resp.json::<HttpProjectSnapshot>()
        .await
        .map_err(|err| format!("Invalid /api/projects JSON response: {err}"))
}

pub(super) async fn post_mutate(
    http: &reqwest::Client,
    origin: &str,
    bearer: &str,
    body: Value,
) -> Result<(), String> {
    let url = format!("{}/api/projects/mutate", origin.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .json(&body)
        .send()
        .await
        .map_err(|err| format!("Could not reach T3 Code server at {origin}: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!(
            "POST /api/projects/mutate failed ({status}): {detail}"
        ));
    }
    Ok(())
}
