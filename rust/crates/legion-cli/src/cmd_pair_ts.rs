//! Tailscale Serve resolution and JSON output helpers for `legioncode pair`.

use std::env;
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use legion_contracts::environment::ExecutionEnvironmentDescriptor;
use legion_persistence::ownership::{PersistedServerRuntimeState, read_live_server_runtime_state};
use legion_persistence::pairing_tokens::IssuedPairingToken;
use legion_persistence::paths::ServerDerivedPaths;
use legion_remote::live_network::LiveTailscaleController;
use legion_remote::tunnel::tailscale::{DEFAULT_TAILSCALE_SERVE_PORT, ServeTarget};
use serde_json::json;

/// Default HTTPS port used when `--tailscale` is enabled without `--tailscale-serve-port`.
pub(crate) const DEFAULT_SERVE_PORT: u16 = DEFAULT_TAILSCALE_SERVE_PORT;

pub(crate) async fn publish_tailscale_pairing_base(
    runtime: &PersistedServerRuntimeState,
    descriptor: &ExecutionEnvironmentDescriptor,
    serve_port: u16,
) -> Result<(String, String), String> {
    let target = resolve_tailscale_target(runtime)?;
    let mut ctl = LiveTailscaleController::new()
        .map_err(|err| format!("Failed to initialize Tailscale Serve controller: {err}"))?;
    if let Ok(bin) = env::var("LEGIONCODE_TAILSCALE_BIN") {
        let trimmed = bin.trim();
        if !trimmed.is_empty() {
            ctl = ctl.with_executable(trimmed);
        }
    }
    let public_base_url = ctl
        .publish_serve(&target, serve_port, descriptor.environment_id.as_str())
        .await
        .map_err(|err| err.to_string())?;
    let note = format!(
        "Tailscale Serve now maps {public_base_url} to this server and persists across restarts. Remove it with `tailscale serve --https={serve_port} --set-path=/ off`."
    );
    Ok((public_base_url, note))
}

fn resolve_tailscale_target(runtime: &PersistedServerRuntimeState) -> Result<ServeTarget, String> {
    if let Some(ref dev_url) = runtime.dev_url {
        let parsed = reqwest::Url::parse(dev_url)
            .map_err(|err| format!("Invalid dev URL `{dev_url}` for Tailscale Serve: {err}"))?;
        if parsed.scheme() != "http" {
            return Err(format!(
                "Tailscale Serve can only proxy plain-HTTP local targets, and this dev server runs at {dev_url}. Pair without --tailscale instead."
            ));
        }
        let port = parsed.port_or_known_default().unwrap_or(80);
        let host = parsed.host_str().unwrap_or("127.0.0.1").to_owned();
        return Ok(ServeTarget { host, port });
    }
    let raw_host = runtime.host.as_deref().map_or("127.0.0.1", str::trim);
    let host = if raw_host.is_empty() || matches!(raw_host, "0.0.0.0" | "::" | "[::]") {
        "127.0.0.1".to_owned()
    } else {
        raw_host.trim_matches(|c| c == '[' || c == ']').to_owned()
    };
    Ok(ServeTarget {
        host,
        port: runtime.port,
    })
}

pub(crate) fn render_pair_json(
    issued: &IssuedPairingToken,
    origin: &str,
    pairing_url: &str,
    app_link: &str,
    scopes: &[String],
) -> Result<String, String> {
    serde_json::to_string_pretty(&json!({
        "id": issued.id,
        "credential": issued.credential,
        "token": issued.credential,
        "origin": origin,
        "pairingUrl": pairing_url,
        "appLink": app_link,
        "scopes": scopes,
        "expiresAt": issued.expires_at_iso,
    }))
    .map_err(|err| format!("Failed to serialize pairing JSON: {err}"))
}

pub(crate) fn resolve_direct_pairing_base(runtime: &PersistedServerRuntimeState) -> (String, bool) {
    if let Some(ref dev_url) = runtime.dev_url {
        let is_lb = dev_url.contains("://127.0.0.1")
            || dev_url.contains("://localhost")
            || dev_url.contains("://[::1]");
        return (dev_url.clone(), is_lb);
    }
    let host = runtime.host.as_deref().map_or("", str::trim);
    if host.is_empty() {
        return (format!("http://localhost:{}", runtime.port), true);
    }
    let conn_host = if matches!(host, "0.0.0.0" | "::" | "[::]") {
        local_lan_ipv4().unwrap_or_else(|| "localhost".to_owned())
    } else {
        host.trim_matches(|c| c == '[' || c == ']').to_owned()
    };
    let is_lb = matches!(conn_host.as_str(), "localhost" | "127.0.0.1" | "::1");
    let url_host = if conn_host.contains(':') {
        format!("[{conn_host}]")
    } else {
        conn_host
    };
    (format!("http://{url_host}:{}", runtime.port), is_lb)
}

fn local_lan_ipv4() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?;
    let IpAddr::V4(v4) = sock.local_addr().ok()?.ip() else {
        return None;
    };
    (!v4.is_loopback() && !v4.is_unspecified()).then(|| v4.to_string())
}

pub(crate) struct PairOutputFields<'a> {
    pub(crate) server_label: &'a str,
    pub(crate) origin: &'a str,
    pub(crate) qr: &'a str,
    pub(crate) pairing_url: &'a str,
    pub(crate) app_link: &'a str,
    pub(crate) token: &'a str,
    pub(crate) expires_at_iso: &'a str,
    pub(crate) notes: &'a [&'a str],
}

pub(crate) fn format_pair_output(fields: &PairOutputFields<'_>) -> String {
    let mut lines = vec![
        format!("Pairing with {} ({}).", fields.server_label, fields.origin),
        String::new(),
        fields.qr.to_owned(),
        String::new(),
        format!("Pairing URL: {}", fields.pairing_url),
        format!("App link: {}", fields.app_link),
        format!("Token: {}", fields.token),
        format!("Expires: {}", fields.expires_at_iso),
    ];
    for note in fields.notes {
        lines.push(String::new());
        lines.push(format!("Note: {note}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

pub(crate) async fn discover_pair_target(
    base_dir: &Path,
) -> Option<(
    &'static str,
    PathBuf,
    PersistedServerRuntimeState,
    ExecutionEnvironmentDescriptor,
)> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    for (variant, is_explicit) in [("userdata", true), ("dev", false)] {
        let paths = ServerDerivedPaths::derive(base_dir, !is_explicit, is_explicit);
        let Some(runtime) = read_live_server_runtime_state(&paths.server_runtime_state_path) else {
            continue;
        };
        let env_url = format!(
            "{}/.well-known/legioncode/environment",
            runtime.origin.trim_end_matches('/')
        );
        let Ok(resp) = http.get(&env_url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(descriptor) = resp.json::<ExecutionEnvironmentDescriptor>().await else {
            continue;
        };
        return Some((variant, paths.db_path, runtime, descriptor));
    }
    None
}
