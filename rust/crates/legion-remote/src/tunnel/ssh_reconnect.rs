//! SSH forward reconnect resilience (`#39802`, `#39807`), readiness probing, archive validation,
//! and SSH auth diagnostic matchers.

use std::net::Ipv4Addr;
use std::sync::OnceLock;
use std::time::Duration;

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig};
use regex::Regex;
use reqwest::Client;
use tokio::net::TcpStream;

use crate::host_class::is_local_loopback_host;
use crate::ssh_helper::WaitReadyOutcome;

use super::SshError;

fn exact_archive_version_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
        );
        // INFALLIBLE: constant SemVer pattern matching tunnel.ts:1100-1101.
        compiled.expect("exact archive SemVer regex must compile")
    })
}

fn auth_failure_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r"(?i)permission denied \((?:publickey|password|keyboard-interactive|hostbased|gssapi-with-mic)[^)]*\)|authentication failed|too many authentication failures",
        );
        // INFALLIBLE: constant SSH auth failure pattern matching auth.ts:242-246.
        compiled.expect("SSH auth failure regex must compile")
    })
}

fn agent_signing_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r"sign_and_send_pubkey: signing failed(?: for [^\n]* from agent:|: (?:agent refused operation|communication with agent failed))",
        );
        // INFALLIBLE: constant SSH agent signing failure pattern matching auth.ts:253.
        compiled.expect("SSH agent signing failure regex must compile")
    })
}

/// Validates an exact `SemVer` archive directory segment (`tunnel.ts:1100-1120`).
///
/// # Errors
///
/// Returns [`SshError::InvalidArchiveVersion`] if `archive_version` is not exact `X.Y.Z[-prerelease]`.
pub fn validate_archive_version(archive_version: &str) -> Result<(), SshError> {
    if exact_archive_version_regex().is_match(archive_version) {
        Ok(())
    } else {
        Err(SshError::InvalidArchiveVersion {
            archive_version: archive_version.to_owned(),
        })
    }
}

/// Validates that a forwarded SSH bridge base URL targets loopback (`tunnel.ts:1418-1438`).
///
/// # Errors
///
/// Returns [`SshError::HttpBridge`] when the URL scheme is not HTTP(S) or the host is not loopback.
pub fn resolve_loopback_http_base_url(raw: &str) -> Result<String, SshError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SshError::HttpBridge {
            message: "Invalid SSH forwarded http base URL.".to_owned(),
        });
    }
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or_else(|| SshError::HttpBridge {
            message: "Invalid SSH forwarded http base URL.".to_owned(),
        })?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_part = if authority.starts_with('[') {
        authority.find(']').map_or("", |idx| &authority[..=idx])
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host_part.is_empty() || !is_local_loopback_host(host_part) {
        return Err(SshError::HttpBridge {
            message: "SSH desktop bridge only supports loopback forwarded URLs.".to_owned(),
        });
    }
    Ok(trimmed.to_owned())
}

/// Returns `true` when `message` indicates an SSH authentication rejection (`auth.ts:238-248`).
#[must_use]
pub fn is_ssh_auth_failure(message: &str) -> bool {
    auth_failure_regex().is_match(message)
}

/// Returns `true` when `message` indicates a local SSH agent signing failure (`auth.ts:251-256`).
#[must_use]
pub fn is_ssh_agent_signing_failure(message: &str) -> bool {
    agent_signing_regex().is_match(message)
}

/// Result of the loopback HTTP readiness check against an existing SSH forward (`#39802`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForwardHttpOutcome {
    /// Forward readiness check succeeded with HTTP 2xx.
    Ready,
    /// Forward answered with non-2xx HTTP status or immediate connection/reset error.
    AnsweredOther,
    /// Forward readiness GET timed out waiting for the remote server handshake.
    TimedOut,
}

/// Inputs sampled while deciding whether to keep an existing SSH local forward across slow server
/// reconnect handshakes (`#39802`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExistingForwardProbe {
    /// Result of the loopback readiness HTTP GET against `base_url`.
    pub http_outcome: ForwardHttpOutcome,
    /// Whether the tracked SSH tunnel child process is still running.
    pub process_running: bool,
    /// Whether a listener accepts connections on `127.0.0.1:<local_port>`.
    pub local_listener_active: bool,
}

/// Reconnect action for an already-opened SSH local port forward (`#39802`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExistingForwardDecision {
    /// Forward answered 2xx cleanly and can be reused immediately.
    ReuseReady,
    /// Forward stalled without an answer while the child & local socket stayed live; keep it open.
    KeepSlowForward,
    /// Forward returned an error/close or died; tear down and spawn a replacement forward.
    ReplaceForward,
}

/// Evaluates whether an existing SSH local port forward stays open across a slow server reconnect
/// (`#39802`).
#[must_use]
pub const fn evaluate_existing_forward(probe: ExistingForwardProbe) -> ExistingForwardDecision {
    match probe.http_outcome {
        ForwardHttpOutcome::Ready => ExistingForwardDecision::ReuseReady,
        ForwardHttpOutcome::TimedOut if probe.process_running && probe.local_listener_active => {
            ExistingForwardDecision::KeepSlowForward
        }
        ForwardHttpOutcome::TimedOut | ForwardHttpOutcome::AnsweredOther => {
            ExistingForwardDecision::ReplaceForward
        }
    }
}

/// Actively probes `base_url` + `127.0.0.1:<local_port>` to classify slow vs dead SSH forwards (`#39802`).
pub async fn probe_existing_forward_async(
    base_url: &str,
    local_port: u16,
    timeout: Duration,
    process_running: bool,
) -> ExistingForwardDecision {
    let Ok(client) = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
    else {
        return ExistingForwardDecision::ReplaceForward;
    };
    let http_outcome = match client.get(base_url).send().await {
        Ok(resp) if resp.status().is_success() => ForwardHttpOutcome::Ready,
        Err(err) if err.is_timeout() => ForwardHttpOutcome::TimedOut,
        Ok(_) | Err(_) => ForwardHttpOutcome::AnsweredOther,
    };
    let local_listener_active = if http_outcome == ForwardHttpOutcome::TimedOut && process_running {
        tokio::time::timeout(
            Duration::from_millis(250),
            TcpStream::connect((Ipv4Addr::LOCALHOST, local_port)),
        )
        .await
        .is_ok_and(|res| res.is_ok())
    } else {
        false
    };
    evaluate_existing_forward(ExistingForwardProbe {
        http_outcome,
        process_running,
        local_listener_active,
    })
}

/// Reconnect decision when the previously tracked managed remote server refuses loopback
/// connections (`#39807`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteReconnectDecision {
    /// Managed server is alive and ready.
    ReuseRunning,
    /// Managed server closed its listener while shutting down; abort retries immediately.
    StopWhileShuttingDown {
        /// Remote PID still shutting down.
        pid: u32,
        /// Actionable error message for the caller.
        message: String,
    },
    /// Previous server exited; clear stale PID/port state and launch a fresh server.
    ClearStaleAndRelaunch,
}

/// Decides whether SSH launch should abort immediately while the previous remote server shuts down
/// (`#39807`).
#[must_use]
pub fn evaluate_remote_stopping_reconnect(
    default_runtime_pid: Option<u32>,
    remote_pid: u32,
    wait_outcome: WaitReadyOutcome,
    still_running_after_wait: bool,
) -> RemoteReconnectDecision {
    evaluate_remote_stopping_reconnect_for_brand(
        default_runtime_pid,
        remote_pid,
        wait_outcome,
        still_running_after_wait,
        &ACTIVE_BRAND,
    )
}

/// Brand-parameterized variant of [`evaluate_remote_stopping_reconnect`] (`#39807`).
#[must_use]
pub fn evaluate_remote_stopping_reconnect_for_brand(
    default_runtime_pid: Option<u32>,
    remote_pid: u32,
    wait_outcome: WaitReadyOutcome,
    still_running_after_wait: bool,
    brand: &BrandConfig,
) -> RemoteReconnectDecision {
    if wait_outcome == WaitReadyOutcome::Ready {
        return RemoteReconnectDecision::ReuseRunning;
    }
    if default_runtime_pid == Some(remote_pid) && wait_outcome == WaitReadyOutcome::RefusedOnly {
        if still_running_after_wait {
            let short = brand.cli_bin_name.to_ascii_uppercase();
            return RemoteReconnectDecision::StopWhileShuttingDown {
                pid: remote_pid,
                message: format!(
                    "The previous remote {short} server (PID {remote_pid}) is still shutting down. Retry once it has exited."
                ),
            };
        }
        return RemoteReconnectDecision::ClearStaleAndRelaunch;
    }
    RemoteReconnectDecision::ClearStaleAndRelaunch
}
