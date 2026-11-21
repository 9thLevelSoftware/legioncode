//! SSH command argv builders, config/socket helpers, and tunnel state machine.
//!
//! Mirrors `packages/ssh/src/{auth,command,config,errors,tunnel}.ts`.

#[path = "ssh_args.rs"]
mod args;
#[path = "ssh_config.rs"]
mod config;
#[path = "ssh_hosts.rs"]
mod hosts;
#[path = "ssh_known_hosts.rs"]
mod known_hosts;
#[path = "ssh_reconnect.rs"]
mod reconnect;
#[path = "ssh_state.rs"]
mod state;
#[path = "ssh_types.rs"]
mod types;

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use t3_contracts::brand::ACTIVE_BRAND;
use thiserror::Error;

pub use args::{
    base_ssh_args, build_control_master_cancel_args, build_control_master_check_args,
    build_control_master_tunnel_args, build_direct_tunnel_args, build_ssh_command_args,
    build_ssh_host_spec, control_master_args, extract_control_path, parse_ssh_config_output,
    parse_ssh_resolve_output,
};
pub use config::{ParsedConfigLine, parse_ssh_config_line};
pub use hosts::{DiscoveredSshHost, SshHostSource, discover_ssh_hosts};
pub use known_hosts::{KnownHost, parse_known_hosts};
pub use reconnect::{
    ExistingForwardDecision, ExistingForwardProbe, ForwardHttpOutcome, RemoteReconnectDecision,
    evaluate_existing_forward, evaluate_remote_stopping_reconnect,
    evaluate_remote_stopping_reconnect_for_brand, is_ssh_agent_signing_failure,
    is_ssh_auth_failure, probe_existing_forward_async, resolve_loopback_http_base_url,
    validate_archive_version,
};
pub use state::{SshTunnelStateMachine, append_stderr_tail};
pub use types::{
    RemoteServerKind, SshTunnelState, TUNNEL_STDERR_TAIL_CHARS, TunnelAction, TunnelEndpoints,
    TunnelEvent, TunnelTransport,
};

/// Default remote T3 Code HTTP port (`tunnel.ts:56`).
pub const DEFAULT_REMOTE_PORT: u16 = 3773;
/// Port scan window size starting from `DEFAULT_REMOTE_PORT` (`tunnel.ts:57`).
pub const REMOTE_PORT_SCAN_WINDOW: u16 = 200;
/// Maximum retained length of SSH error stdout (`command.ts:18`).
pub const MAX_SSH_ERROR_OUTPUT_LENGTH: usize = 4_000;
/// Default SSH command timeout in milliseconds (`command.ts:17`).
pub const DEFAULT_SSH_COMMAND_TIMEOUT_MS: u64 = 60_000;

/// SSH target descriptor (`DesktopSshEnvironmentTarget` in `packages/contracts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    /// SSH config alias (or hostname when direct).
    pub alias: String,
    /// Resolved or explicit hostname.
    pub hostname: String,
    /// Optional SSH username.
    pub username: Option<String>,
    /// Optional TCP port override.
    pub port: Option<u16>,
}

/// SSH `BatchMode` setting (`yes` or `no`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BatchMode {
    /// Fail instead of prompting interactively (`BatchMode=yes`).
    Yes,
    /// Allow askpass/interactive prompts (`BatchMode=no`).
    #[default]
    No,
}

impl BatchMode {
    /// Returns the OpenSSH option string (`"yes"` or `"no"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

/// Errors produced by SSH argument construction, discovery, and tunnel state transitions.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SshError {
    /// Target lacked both alias and hostname (`SshInvalidTargetError`).
    #[error("{message}")]
    InvalidTarget {
        /// Validation diagnostic.
        message: String,
    },
    /// Archive version failed strict `SemVer` segment validation (`SshInvalidArchiveVersionError`).
    #[error(
        "'{archive_version}' is not an exact {} version and cannot name a runtime directory.",
        ACTIVE_BRAND.cli_bin_name
    )]
    InvalidArchiveVersion {
        /// Rejected archive version string.
        archive_version: String,
    },
    /// Remote runner lacked both `nodeScriptPath` and `archiveVersion` (`SshMissingRunnerError`).
    #[error(
        "A remote {} runner needs an archive version or a node script path.",
        ACTIVE_BRAND.cli_bin_name
    )]
    MissingRunner,
    /// Failed to discover local SSH hosts (`SshHostDiscoveryError`).
    #[error("{message}")]
    HostDiscovery {
        /// Discovery error description.
        message: String,
    },
    /// Invalid forwarded HTTP bridge URL (`SshHttpBridgeError`).
    #[error("{message}")]
    HttpBridge {
        /// URL validation error.
        message: String,
    },
    /// Invalid state transition on [`SshTunnelStateMachine`].
    #[error("invalid SSH tunnel transition from '{from}' on '{event}'")]
    InvalidStateTransition {
        /// Source state name.
        from: &'static str,
        /// Event name.
        event: &'static str,
    },
}

fn secret_redaction_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r#"(?i)("(?:access_token|bearerToken|credential|pairingToken|token)"\s*:\s*")[^"]+(")"#,
        );
        // INFALLIBLE: constant credential field redaction pattern matching command.ts:169.
        compiled.expect("SSH secret redaction regex must compile")
    })
}

fn safe_shell_arg_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^[\w@%+=:,./-]+$");
        // INFALLIBLE: constant POSIX/Windows safe token pattern matching command.ts:182.
        compiled.expect("safe shell arg regex must compile")
    })
}

/// Returns `"ssh.exe"` on Windows and `"ssh"` elsewhere (`command.ts:25-26`).
#[must_use]
pub const fn ssh_command_for_platform(is_windows: bool) -> &'static str {
    if is_windows { "ssh.exe" } else { "ssh" }
}

/// Formats the stable NUL-delimited target key (`command.ts:78-80`).
#[must_use]
pub fn target_connection_key(target: &SshTarget) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        target.alias,
        target.hostname,
        target.username.as_deref().unwrap_or(""),
        target.port.map_or_else(String::new, |p| p.to_string())
    )
}

/// Computes the stable 16-hex-char SHA-256 prefix for `~/.t3/ssh-launch/<key>` (`command.ts:82-91`).
///
/// ```
/// use t3_remote::tunnel::ssh::{SshTarget, remote_state_key};
///
/// let target = SshTarget {
///     alias: "devbox".to_owned(),
///     hostname: "devbox.example.com".to_owned(),
///     username: Some("julius".to_owned()),
///     port: Some(2222),
/// };
/// assert_eq!(remote_state_key(&target), "711bc738002d72fd");
/// ```
#[must_use]
pub fn remote_state_key(target: &SshTarget) -> String {
    let digest = Sha256::digest(target_connection_key(target).as_bytes());
    hex::encode(digest)[..16].to_owned()
}

/// Redacts token JSON fields and truncates long SSH command output (`command.ts:167-176`).
#[must_use]
pub fn redact_ssh_error_output(output: &str, keep_tail: bool) -> String {
    let redacted = secret_redaction_regex()
        .replace_all(output, "${1}[redacted]${2}")
        .into_owned();
    let total_chars = redacted.chars().count();
    if total_chars <= MAX_SSH_ERROR_OUTPUT_LENGTH {
        return redacted;
    }
    if keep_tail {
        let skip = total_chars - MAX_SSH_ERROR_OUTPUT_LENGTH;
        let tail = redacted
            .char_indices()
            .nth(skip)
            .map_or("", |(idx, _)| &redacted[idx..]);
        format!("[truncated]\n{tail}")
    } else {
        let head = redacted
            .char_indices()
            .nth(MAX_SSH_ERROR_OUTPUT_LENGTH)
            .map_or(redacted.as_str(), |(idx, _)| &redacted[..idx]);
        format!("{head}\n[truncated]")
    }
}

/// POSIX single-quote escapes a shell argument (`command.ts:178-180`).
#[must_use]
pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Formats actionable host-key verification guidance when `stderr` reports a mismatch (`command.ts:192-212`).
#[must_use]
pub fn host_key_verification_message(
    target: &SshTarget,
    stderr: &str,
    is_windows: bool,
) -> Option<String> {
    if !stderr.contains("Host key verification failed") {
        return None;
    }
    let host_spec = build_ssh_host_spec(target).ok()?;
    let mut parts = vec!["ssh".to_owned()];
    if let Some(port) = target.port {
        parts.push("-p".to_owned());
        parts.push(port.to_string());
    }
    if host_spec.starts_with('-') {
        parts.push("--".to_owned());
    }
    parts.push(host_spec.clone());
    let quoted = parts
        .into_iter()
        .map(|arg| {
            if safe_shell_arg_regex().is_match(&arg) {
                arg
            } else if is_windows {
                format!("\"{arg}\"")
            } else {
                shell_single_quote(&arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let guidance = if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        format!(
            "The host key for {host_spec} has changed since it was trusted. Only if you expect this change, remove the old key with the `ssh-keygen -R` command in the output below, run `{quoted}` in a terminal, check that the new host key fingerprint is the one you expect, then try again."
        )
    } else {
        format!(
            "SSH could not verify the host key for {host_spec}. Run `{quoted}` in a terminal, check that the host key fingerprint is the one you expect, then try again."
        )
    };
    Some(format!("{guidance}\n\n{}", stderr.trim()))
}
