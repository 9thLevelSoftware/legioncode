//! CLI human-readable policy summaries matching `apps/server/src/cli/policy.ts:18-67`.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::{ManagedPolicyDocument, PolicyError, policy_fingerprint_12hex};

/// Compact fingerprint metadata recorded in `PersistedServerRuntimeState.managedPolicy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPolicySummary {
    /// File path or platform policy key that supplied the document.
    pub source: String,
    /// Managed organization name from the policy document.
    pub organization: String,
    /// 12-character lowercase SHA-256 hex digest of the encoded document.
    pub fingerprint: String,
}

/// Live server policy state used by [`describe_running_server`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningServerState {
    /// OS process ID of the running server.
    pub pid: u32,
    /// ISO-8601 timestamp when the server started.
    pub started_at: String,
    /// `None` when an older server did not record `managedPolicy`;
    /// `Some(None)` when the server recorded `managedPolicy: null`;
    /// `Some(Some(summary))` when a policy was loaded at startup.
    pub managed_policy: Option<Option<LoadedPolicySummary>>,
}

/// Formats the installed managed policy for CLI display (`cli/policy.ts:18-41`).
pub fn describe_policy(
    policy_path: &str,
    document: Option<&ManagedPolicyDocument>,
) -> Result<String, PolicyError> {
    let Some(doc) = document else {
        return Ok(
            "No managed policy on this machine. Every setting belongs to the user.\n".to_owned(),
        );
    };
    let fingerprint = policy_fingerprint_12hex(doc)?;
    let mut out = String::with_capacity(512);
    let _ = writeln!(
        out,
        "Managed by {} ({}, fingerprint {fingerprint}).",
        doc.organization, policy_path
    );
    if let Some(support_url) = &doc.support_url {
        let _ = writeln!(out, "Support: {support_url}");
    }
    out.push('\n');

    match &doc.settings {
        Some(settings) if !settings.is_empty() => {
            out.push_str("Settings:\n");
            for (path, setting) in settings {
                let value_json = serde_json::to_string(&setting.value)
                    .map_err(|err| PolicyError::Json(err.to_string()))?;
                let mode = if setting.locked { "locked" } else { "default" };
                let _ = writeln!(out, "  {path} = {value_json} ({mode})");
            }
        }
        _ => out.push_str("No managed settings.\n"),
    }

    if let Some(limits) = &doc.limits {
        let limits_json = serde_json::to_string_pretty(limits)
            .map_err(|err| PolicyError::Json(err.to_string()))?;
        out.push_str("\nLimits:\n");
        out.push_str(&limits_json);
        out.push('\n');
    }

    Ok(out)
}

/// Reports whether the running server loaded the currently installed policy (`cli/policy.ts:44-67`).
pub fn describe_running_server(
    state: Option<&RunningServerState>,
    installed: Option<&LoadedPolicySummary>,
) -> String {
    let Some(running) = state else {
        return "No T3 Code server is running from this T3 home. The installed policy applies when one starts.".to_owned();
    };
    let server = format!(
        "The running server (pid {}, started {})",
        running.pid, running.started_at
    );
    let Some(loaded) = &running.managed_policy else {
        return format!(
            "{server} does not report which policy it loaded. Restart it to apply the installed policy."
        );
    };
    let loaded_fp = loaded.as_ref().map(|summary| summary.fingerprint.as_str());
    let installed_fp = installed.map(|summary| summary.fingerprint.as_str());
    if loaded_fp == installed_fp {
        return match loaded {
            None => format!("{server} runs without a policy, as installed."),
            Some(_) => format!("{server} loaded this policy."),
        };
    }
    let what = match loaded {
        None => "no policy".to_owned(),
        Some(summary) => format!(
            "{}'s policy from {} (fingerprint {})",
            summary.organization, summary.source, summary.fingerprint
        ),
    };
    format!("{server} loaded {what}. Restart T3 Code to apply the installed policy.")
}
