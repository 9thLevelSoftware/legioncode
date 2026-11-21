//! Tailscale CLI diagnostic classification and exact `cli/pair.ts:101-190` errors.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Sanitized stderr diagnostic categories (`tailscale.ts:45-53`).
///
/// Unrecognized output collapses to [`TailscaleStderrDiagnostic::Unknown`] so auth keys or node
/// names in CLI stderr never leak into logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TailscaleStderrDiagnostic {
    /// Existing serve handler does not exist.
    NoExistingHandler,
    /// Node is logged out or needs login.
    NotLoggedIn,
    /// `tailscaled` daemon is not running.
    NotRunning,
    /// Local user lacks permission / operator rights.
    PermissionDenied,
    /// macOS/sandboxed Tailscale refused non-localhost proxy target.
    UnsupportedTarget,
    /// Any unrecognized non-empty stderr output.
    Unknown,
}

/// Structured Tailscale CLI, `MagicDNS`, and Serve errors matching `cli/pair.ts:101-190`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TailscaleError {
    /// Failed to decode `tailscale status --json` or `debug prefs` output (`TailscaleStatusParseError`).
    #[error("Failed to decode tailscale status JSON.")]
    StatusParse {
        /// Underlying decode detail.
        detail: String,
    },
    /// Invalid `MagicDNS` hostname when constructing a Serve HTTPS URL.
    #[error("Invalid MagicDNS hostname '{magic_dns_name}'.")]
    InvalidMagicDnsName {
        /// Rejected `MagicDNS` hostname.
        magic_dns_name: String,
    },
    /// `TailscaleUnavailableError` (`cli/pair.ts:101-108`).
    #[error("Could not talk to Tailscale. Is tailscaled running? Try `tailscale status`.")]
    TailscaleUnavailable,
    /// `MagicDnsNameMissingError` (`cli/pair.ts:110-117`).
    #[error("This machine has no MagicDNS name. Run `tailscale up` and enable MagicDNS.")]
    MagicDnsNameMissing,
    /// `TailscaleServeNotEnabledError` (`cli/pair.ts:119-126`).
    #[error(
        "Tailscale Serve is not enabled on your tailnet. Enable HTTPS certificates at https://login.tailscale.com/admin/dns, then run this command again."
    )]
    TailscaleServeNotEnabled,
    /// `ServesOtherEnvironmentError` (`cli/pair.ts:128-135`).
    #[error(
        "Tailscale Serve on HTTPS port {serve_port} already fronts a different T3 Code server. Pass --tailscale-serve-port to publish this one on another port."
    )]
    ServesOtherEnvironment {
        /// Occupied Tailscale Serve port.
        serve_port: u16,
    },
    /// `TailscaleServeFailedError` (`cli/pair.ts:137-144`).
    #[error(
        "tailscale serve failed for HTTPS port {serve_port}. Run `tailscale serve --https={serve_port} --bg <local-url>` by hand to see why."
    )]
    TailscaleServeFailed {
        /// Serve port that failed.
        serve_port: u16,
    },
    /// `TailscaleOperatorRequiredError` (`cli/pair.ts:146-153`).
    #[error(
        "Tailscale refused the Serve config because this user is not its operator. Run `sudo tailscale set --operator=$USER` once, then run this command again."
    )]
    TailscaleOperatorRequired,
    /// `TailscaleServeTargetRefusedError` (`cli/pair.ts:155-162`).
    #[error(
        "This Tailscale install can only proxy to localhost, and the server listens on {local_host}. Restart it with `--host 127.0.0.1` or `--host 0.0.0.0`, then run this command again."
    )]
    TailscaleServeTargetRefused {
        /// Bound host rejected by Tailscale Serve.
        local_host: String,
    },
    /// `ServePortOccupiedError` (`cli/pair.ts:164-171`).
    #[error(
        "HTTPS port {serve_port} on the tailnet already serves something that is not a T3 Code server. Pass --tailscale-serve-port to publish this one on another port."
    )]
    ServePortOccupied {
        /// Occupied Serve port.
        serve_port: u16,
    },
    /// `DevServerNotProxiableError` (`cli/pair.ts:183-190`).
    #[error(
        "Tailscale Serve can only proxy plain-HTTP local targets, and this dev server runs at {dev_url}. Pair without --tailscale instead."
    )]
    DevServerNotProxiable {
        /// Non-HTTP dev server URL.
        dev_url: String,
    },
}

fn compile_static_diag(pattern: &str) -> Regex {
    let compiled = Regex::new(pattern);
    // INFALLIBLE: caller passes static compile-time constant Tailscale CLI diagnostic patterns.
    compiled.expect("tailscale diagnostic regex must compile")
}

fn stderr_patterns() -> &'static [(Regex, TailscaleStderrDiagnostic)] {
    static PATTERNS: OnceLock<Vec<(Regex, TailscaleStderrDiagnostic)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                compile_static_diag(r"(?i)handler does not exist"),
                TailscaleStderrDiagnostic::NoExistingHandler,
            ),
            (
                compile_static_diag(r"(?i)not logged in|logged out|needs? login"),
                TailscaleStderrDiagnostic::NotLoggedIn,
            ),
            (
                compile_static_diag(
                    r"(?i)doesn't appear to be running|tailscaled\.service not running|tailscale is stopped|tailscaled: the system cannot find the file specified|failed to connect to local tailscale[\s\S]*(connection refused|no such file or directory|cannot find the file)",
                ),
                TailscaleStderrDiagnostic::NotRunning,
            ),
            (
                compile_static_diag(
                    r"(?i)permission denied|access denied|must be root|operation not permitted",
                ),
                TailscaleStderrDiagnostic::PermissionDenied,
            ),
            (
                compile_static_diag(r"(?i)doesn't support connecting to remote destinations"),
                TailscaleStderrDiagnostic::UnsupportedTarget,
            ),
        ]
    })
}

/// Classifies `tailscale` stderr without quoting raw CLI text (`tailscale.ts:71-76`).
#[must_use]
pub fn classify_stderr_diagnostic(stderr: &str) -> Option<TailscaleStderrDiagnostic> {
    if stderr.trim().is_empty() {
        return None;
    }
    for (pattern, diag) in stderr_patterns() {
        if pattern.is_match(stderr) {
            return Some(*diag);
        }
    }
    Some(TailscaleStderrDiagnostic::Unknown)
}
