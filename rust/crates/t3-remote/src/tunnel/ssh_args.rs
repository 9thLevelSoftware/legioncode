//! SSH argv builders, `ssh -G` parsing, and `ControlMaster` socket command constructors.

use std::collections::HashMap;

use super::{BatchMode, SshError, SshTarget};

/// Builds `[user@]destination`, rejecting targets with empty alias and hostname (`command.ts:93-99`).
///
/// # Errors
///
/// Returns [`SshError::InvalidTarget`] when both `alias` and `hostname` trim to empty.
pub fn build_ssh_host_spec(target: &SshTarget) -> Result<String, SshError> {
    let dest = if target.alias.trim().is_empty() {
        target.hostname.trim()
    } else {
        target.alias.trim()
    };
    if dest.is_empty() {
        return Err(SshError::InvalidTarget {
            message: "SSH target is missing its alias/hostname.".to_owned(),
        });
    }
    match target.username.as_deref().filter(|u| !u.is_empty()) {
        Some(user) => Ok(format!("{user}@{dest}")),
        None => Ok(dest.to_owned()),
    }
}

/// Builds baseline OpenSSH options overriding alias `RemoteCommand` and `RequestTTY` (`command.ts:112-129`).
#[must_use]
pub fn base_ssh_args(target: &SshTarget, batch_mode: BatchMode) -> Vec<String> {
    let mut args = vec![
        "-o".to_owned(),
        format!("BatchMode={}", batch_mode.as_str()),
        "-o".to_owned(),
        "ConnectTimeout=10".to_owned(),
        "-o".to_owned(),
        "RemoteCommand=none".to_owned(),
        "-o".to_owned(),
        "RequestTTY=no".to_owned(),
    ];
    if let Some(port) = target.port {
        args.push("-p".to_owned());
        args.push(port.to_string());
    }
    args
}

/// Builds `ControlMaster` socket arguments preventing user forwards and failover (`command.ts:133-135`).
#[must_use]
pub fn control_master_args(control_path: &str) -> Vec<String> {
    vec![
        "-F".to_owned(),
        "none".to_owned(),
        "-o".to_owned(),
        format!("ControlPath={control_path}"),
        "-o".to_owned(),
        "ProxyCommand=false".to_owned(),
    ]
}

/// Builds full `ssh` argv for a non-tunnel command with keepalives and `--` host separator (`command.ts:267-280`).
///
/// # Errors
///
/// Returns [`SshError::InvalidTarget`] when both `alias` and `hostname` trim to empty.
pub fn build_ssh_command_args(
    target: &SshTarget,
    batch_mode: BatchMode,
    pre_host_args: &[&str],
    remote_command_args: &[&str],
) -> Result<Vec<String>, SshError> {
    let host_spec = build_ssh_host_spec(target)?;
    let mut args = base_ssh_args(target, batch_mode);
    args.extend([
        "-o".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
    ]);
    args.extend(pre_host_args.iter().map(|s| (*s).to_owned()));
    args.push("--".to_owned());
    args.push(host_spec);
    args.extend(remote_command_args.iter().map(|s| (*s).to_owned()));
    Ok(args)
}

/// Builds direct `ssh -N -L` local port-forward argv (`tunnel.ts:1510-1532`).
///
/// # Errors
///
/// Returns [`SshError::InvalidTarget`] when both `alias` and `hostname` trim to empty.
pub fn build_direct_tunnel_args(
    target: &SshTarget,
    local_port: u16,
    remote_port: u16,
    batch_mode: BatchMode,
) -> Result<Vec<String>, SshError> {
    let host_spec = build_ssh_host_spec(target)?;
    let mut args = base_ssh_args(target, batch_mode);
    args.extend([
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-o".to_owned(),
        "ControlMaster=no".to_owned(),
        "-o".to_owned(),
        "ControlPath=none".to_owned(),
        "-o".to_owned(),
        "ControlPersist=no".to_owned(),
        "-o".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
        "-n".to_owned(),
        "-N".to_owned(),
        "-L".to_owned(),
        format!("{local_port}:127.0.0.1:{remote_port}"),
        "--".to_owned(),
        host_spec,
    ]);
    Ok(args)
}

/// Builds `ControlMaster` multiplexed `-L` tunnel argv (`tunnel.ts:1542-1555`).
///
/// # Errors
///
/// Returns [`SshError::InvalidTarget`] when both `alias` and `hostname` trim to empty.
pub fn build_control_master_tunnel_args(
    target: &SshTarget,
    control_path: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<Vec<String>, SshError> {
    let host_spec = build_ssh_host_spec(target)?;
    let mut args = base_ssh_args(target, BatchMode::Yes);
    args.extend(control_master_args(control_path));
    args.extend([
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
        "-o".to_owned(),
        "ControlMaster=no".to_owned(),
        "-T".to_owned(),
        "-L".to_owned(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"),
        "--".to_owned(),
        host_spec,
        "sh -c 'exec cat >/dev/null'".to_owned(),
    ]);
    Ok(args)
}

/// Builds `-O cancel -L ...` arguments to revoke a `ControlMaster` forward (`tunnel.ts:1567-1574`).
///
/// # Errors
///
/// Returns [`SshError::InvalidTarget`] when both `alias` and `hostname` trim to empty.
pub fn build_control_master_cancel_args(
    target: &SshTarget,
    control_path: &str,
    local_port: u16,
    remote_port: u16,
) -> Result<Vec<String>, SshError> {
    let mut pre = control_master_args(control_path);
    pre.extend([
        "-O".to_owned(),
        "cancel".to_owned(),
        "-L".to_owned(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"),
    ]);
    let pre_refs: Vec<&str> = pre.iter().map(String::as_str).collect();
    build_ssh_command_args(target, BatchMode::No, &pre_refs, &[])
}

/// Builds `-O check` arguments to probe a `ControlMaster` socket (`command.ts:472-474`).
///
/// # Errors
///
/// Returns [`SshError::InvalidTarget`] when both `alias` and `hostname` trim to empty.
pub fn build_control_master_check_args(
    target: &SshTarget,
    control_path: &str,
) -> Result<Vec<String>, SshError> {
    let mut pre = control_master_args(control_path);
    pre.extend(["-O".to_owned(), "check".to_owned()]);
    let pre_refs: Vec<&str> = pre.iter().map(String::as_str).collect();
    build_ssh_command_args(target, BatchMode::No, &pre_refs, &[])
}

/// Parses `ssh -G` key/value stdout keeping the first value per key (`command.ts:46-60`).
#[must_use]
pub fn parse_ssh_config_output(stdout: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() || values.contains_key(key) {
            continue;
        }
        values.insert(key.to_owned(), rest.join(" ").trim().to_owned());
    }
    values
}

/// Parses `ssh -G` output into a resolved [`SshTarget`] (`command.ts:62-76`).
#[must_use]
pub fn parse_ssh_resolve_output(alias: &str, stdout: &str) -> SshTarget {
    let values = parse_ssh_config_output(stdout);
    let hostname = values
        .get("hostname")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(alias)
        .to_owned();
    let username = values
        .get("user")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let port = values
        .get("port")
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|&p| p >= 1);
    SshTarget {
        alias: alias.to_owned(),
        hostname,
        username,
        port,
    }
}

/// Extracts `controlpath` from `ssh -G` stdout, returning `None` for missing or `"none"` (`command.ts:466-471`).
#[must_use]
pub fn extract_control_path(stdout: &str) -> Option<String> {
    let values = parse_ssh_config_output(stdout);
    values
        .get("controlpath")
        .map(String::as_str)
        .filter(|p| !p.is_empty() && *p != "none")
        .map(str::to_owned)
}
