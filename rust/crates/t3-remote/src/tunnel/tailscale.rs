//! Tailscale CLI status parsing, `MagicDNS` HTTPS URL construction, Serve inspection, and pairing errors.
//!
//! Mirrors `packages/tailscale/src/tailscale.ts` and `apps/server/src/cli/pair.ts:101-190`.

#[path = "tailscale_diag.rs"]
mod diag;
#[path = "tailscale_root.rs"]
mod root;
#[path = "tailscale_serve.rs"]
mod serve;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host_class::is_tailscale_ipv4_address;

pub use diag::{TailscaleError, TailscaleStderrDiagnostic, classify_stderr_diagnostic};
pub use root::{
    TailscaleServeRootHolder, TailscaleServeRootStatus, classify_serve_root_holder,
    parse_tailscale_serve_root_holders,
};
pub use serve::{
    ServeTarget, TailscaleServeRoute, ensure_tailscale_serve_args, parse_tailscale_serve_routes_to,
    tailscale_serve_off_args,
};

/// Default Tailscale Serve HTTPS port (`tailscale.ts:13`).
pub const DEFAULT_TAILSCALE_SERVE_PORT: u16 = 443;
/// Timeout for `tailscale status` and `tailscale debug prefs` in milliseconds (`tailscale.ts:20`).
pub const TAILSCALE_STATUS_TIMEOUT_MS: u64 = 3_500;
/// Escalation grace between SIGTERM and SIGKILL (`tailscale.ts:27`).
pub const TAILSCALE_KILL_GRACE_MS: u64 = 500;
/// Timeout for `tailscale serve` commands in milliseconds (`tailscale.ts:28`).
pub const TAILSCALE_SERVE_TIMEOUT_MS: u64 = 10_000;
/// Timeout for probing `/.well-known/t3/environment` over Tailscale HTTPS (`tailscale.ts:29`).
pub const TAILSCALE_PROBE_TIMEOUT_MS: u64 = 2_500;

/// Returns `"tailscale.exe"` on Windows and `"tailscale"` elsewhere (`tailscale.ts:31-32`).
#[must_use]
pub const fn tailscale_command_for_platform(is_windows: bool) -> &'static str {
    if is_windows {
        "tailscale.exe"
    } else {
        "tailscale"
    }
}

/// Parsed summary of `tailscale status --json` (`tailscale.ts:158-172`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleStatus {
    /// `Some(BackendState == "Running")`, or `None` when `BackendState` was omitted.
    pub running: Option<bool>,
    /// Self `MagicDNS` hostname with trailing dot stripped, or `None`.
    pub magic_dns_name: Option<String>,
    /// IPv4 addresses belonging to `100.64.0.0/10`.
    pub tailnet_ipv4_addresses: Vec<String>,
    /// Whether `Self.CapMap` includes `"https"`, or `None` if no capability map object was present.
    pub https_serve_enabled: Option<bool>,
}

/// Active Tailscale exit node reported by `tailscale status --json` (`tailscale.ts:329-332`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscaleExitNode {
    /// Tailnet machine DNS label (or OS hostname fallback), if identifiable.
    pub name: Option<String>,
    /// Whether the exit node is currently online.
    pub online: bool,
}

#[derive(Deserialize)]
struct StatusSelfJson {
    #[serde(rename = "DNSName")]
    dns_name: Option<Value>,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Value>,
    #[serde(rename = "CapMap")]
    cap_map: Option<Value>,
}

#[derive(Deserialize)]
struct StatusJson {
    #[serde(rename = "BackendState")]
    backend_state: Option<Value>,
    #[serde(rename = "Self")]
    self_node: Option<StatusSelfJson>,
}

/// Parses `tailscale status --json` output (`tailscale.ts:215-242`).
///
/// # Examples
///
/// ```
/// use t3_remote::tunnel::tailscale::parse_tailscale_status;
///
/// let status = parse_tailscale_status(
///     r#"{"BackendState":"Running","Self":{"DNSName":"box.tail.ts.net.","TailscaleIPs":["100.64.0.5","192.168.1.2"],"CapMap":{"https":null}}}"#
/// )?;
/// assert_eq!(status.running, Some(true));
/// assert_eq!(status.magic_dns_name.as_deref(), Some("box.tail.ts.net"));
/// assert_eq!(status.tailnet_ipv4_addresses, vec!["100.64.0.5"]);
/// assert_eq!(status.https_serve_enabled, Some(true));
/// # Ok::<(), t3_remote::tunnel::tailscale::TailscaleError>(())
/// ```
///
/// # Errors
///
/// Returns [`TailscaleError::StatusParse`] if `raw_status_json` is invalid JSON.
pub fn parse_tailscale_status(raw_status_json: &str) -> Result<TailscaleStatus, TailscaleError> {
    let parsed: StatusJson =
        serde_json::from_str(raw_status_json).map_err(|err| TailscaleError::StatusParse {
            detail: err.to_string(),
        })?;
    let running = parsed
        .backend_state
        .as_ref()
        .map(|state| state.as_str() == Some("Running"));
    let magic_dns_name = parsed
        .self_node
        .as_ref()
        .and_then(|s| s.dns_name.as_ref())
        .and_then(Value::as_str)
        .map(|s| s.trim().trim_end_matches('.'))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let tailnet_ipv4_addresses = parsed
        .self_node
        .as_ref()
        .and_then(|s| s.tailscale_ips.as_ref())
        .and_then(Value::as_array)
        .map_or_default(|ips| {
            ips.iter()
                .filter_map(Value::as_str)
                .filter(|ip| is_tailscale_ipv4_address(ip))
                .map(str::to_owned)
                .collect()
        });
    let https_serve_enabled = parsed
        .self_node
        .as_ref()
        .and_then(|s| s.cap_map.as_ref())
        .and_then(Value::as_object)
        .map(|map| map.contains_key("https"));
    Ok(TailscaleStatus {
        running,
        magic_dns_name,
        tailnet_ipv4_addresses,
        https_serve_enabled,
    })
}

#[derive(Deserialize)]
struct ExitNodePeerJson {
    #[serde(rename = "ExitNode")]
    exit_node: Option<Value>,
    #[serde(rename = "HostName")]
    host_name: Option<Value>,
    #[serde(rename = "DNSName")]
    dns_name: Option<Value>,
}

#[derive(Deserialize)]
struct ExitNodeStatusJson {
    #[serde(rename = "Online")]
    online: Option<Value>,
}

#[derive(Deserialize)]
struct ExitNodeRootJson {
    #[serde(rename = "ExitNodeStatus")]
    exit_node_status: Option<ExitNodeStatusJson>,
    #[serde(rename = "Peer")]
    peer: Option<BTreeMap<String, ExitNodePeerJson>>,
}

/// Parses the active exit node from `tailscale status --json` (`tailscale.ts:338-356`).
///
/// # Errors
///
/// Returns [`TailscaleError::StatusParse`] if `raw_status_json` is invalid JSON.
pub fn parse_tailscale_exit_node(
    raw_status_json: &str,
) -> Result<Option<TailscaleExitNode>, TailscaleError> {
    let parsed: ExitNodeRootJson =
        serde_json::from_str(raw_status_json).map_err(|err| TailscaleError::StatusParse {
            detail: err.to_string(),
        })?;
    let Some(exit_status) = parsed.exit_node_status else {
        return Ok(None);
    };
    let peer = parsed.peer.as_ref().and_then(|peers| {
        peers
            .values()
            .find(|p| p.exit_node.as_ref().and_then(Value::as_bool) == Some(true))
    });
    let dns_label = peer
        .and_then(|p| p.dns_name.as_ref())
        .and_then(Value::as_str)
        .and_then(|d| d.split('.').next())
        .unwrap_or("");
    let host_name = peer
        .and_then(|p| p.host_name.as_ref())
        .and_then(Value::as_str)
        .map_or("", str::trim);
    let name = if !dns_label.is_empty() {
        Some(dns_label.to_owned())
    } else if !host_name.is_empty() {
        Some(host_name.to_owned())
    } else {
        None
    };
    Ok(Some(TailscaleExitNode {
        name,
        online: exit_status.online.as_ref().and_then(Value::as_bool) == Some(true),
    }))
}

#[derive(Deserialize)]
struct PrefsJson {
    #[serde(rename = "RunSSH")]
    run_ssh: Option<Value>,
    #[serde(rename = "WantRunning")]
    want_running: Option<Value>,
}

/// Parses `tailscale debug prefs` JSON to detect active Tailscale SSH (`tailscale.ts:373-379`).
///
/// # Errors
///
/// Returns [`TailscaleError::StatusParse`] if `raw_prefs_json` is invalid JSON.
pub fn parse_tailscale_ssh_enabled(raw_prefs_json: &str) -> Result<bool, TailscaleError> {
    let prefs: PrefsJson =
        serde_json::from_str(raw_prefs_json).map_err(|err| TailscaleError::StatusParse {
            detail: err.to_string(),
        })?;
    let run_ssh = prefs.run_ssh.as_ref().and_then(Value::as_bool) == Some(true);
    let want_running = prefs.want_running.as_ref().and_then(Value::as_bool) != Some(false);
    Ok(run_ssh && want_running)
}

/// Builds the `MagicDNS` HTTPS base URL, omitting port `443` (`DEFAULT_TAILSCALE_SERVE_PORT`).
///
/// Mirrors `buildTailscaleHttpsBaseUrl` (`tailscale.ts:385-396`).
///
/// # Examples
///
/// ```
/// use t3_remote::tunnel::tailscale::build_tailscale_https_base_url;
///
/// assert_eq!(
///     build_tailscale_https_base_url("Box.Tail.ts.net.", None)?,
///     "https://box.tail.ts.net/"
/// );
/// assert_eq!(
///     build_tailscale_https_base_url("box.tail.ts.net", Some(8443))?,
///     "https://box.tail.ts.net:8443/"
/// );
/// # Ok::<(), t3_remote::tunnel::tailscale::TailscaleError>(())
/// ```
///
/// # Errors
///
/// Returns [`TailscaleError::InvalidMagicDnsName`] when `magic_dns_name` is empty or contains invalid URL characters.
pub fn build_tailscale_https_base_url(
    magic_dns_name: &str,
    serve_port: Option<u16>,
) -> Result<String, TailscaleError> {
    let trimmed = magic_dns_name
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if trimmed.is_empty()
        || trimmed.contains(['/', '?', '#', '@', ':', ' '])
        || trimmed.bytes().any(|b| b < 0x20 || b == 0x7f)
    {
        return Err(TailscaleError::InvalidMagicDnsName {
            magic_dns_name: magic_dns_name.to_owned(),
        });
    }
    let port = serve_port.unwrap_or(DEFAULT_TAILSCALE_SERVE_PORT);
    if port == DEFAULT_TAILSCALE_SERVE_PORT {
        Ok(format!("https://{trimmed}/"))
    } else {
        Ok(format!("https://{trimmed}:{port}/"))
    }
}
