//! Policy summary types and capability enforcement helpers (`managedPolicy.ts`).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::ids::ProviderDriverKind;

use super::serde_helpers::{deserialize_opt_non_empty_vec, deserialize_opt_trimmed_non_empty};
use super::{ManagedUpdatesMode, RuntimeMode};

/// Update summary limits exposed to clients (`feedUrl` omitted per `managedPolicy.ts:114`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedUpdatesSummaryPolicy {
    /// Automatic check or completely disabled updates.
    pub mode: ManagedUpdatesMode,
    /// Whether nightly releases are permitted (`false` blocks nightly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_nightly: Option<bool>,
}

/// Provider limits exposed to clients in [`ManagedPolicySummaryLimits`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedProviderSummaryLimits {
    /// Non-empty list of permitted provider drivers; absent allows all drivers.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_non_empty_vec"
    )]
    pub allowed_drivers: Option<Vec<ProviderDriverKind>>,
    /// Per-driver locked instance fields (`config.binaryPath`, `environment.<NAME>`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_instance_fields: Option<IndexMap<ProviderDriverKind, Vec<String>>>,
}

/// Effective policy limits exposed to clients (`managedPolicy.ts:90-116`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedPolicySummaryLimits {
    /// Broadest runtime permission mode any thread may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime_mode: Option<RuntimeMode>,
    /// Non-empty list of allowed client categories.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_non_empty_vec"
    )]
    pub clients: Option<Vec<super::ManagedPolicyClient>>,
    /// `false` also when `clients` omits `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<bool>,
    /// `Some(false)` disables webhook-triggered scheduled tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_tasks: Option<bool>,
    /// `Some(false)` disables product telemetry and relay traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<bool>,
    /// `Some(false)` blocks browser cookie import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_cookie_import: Option<bool>,
    /// `Some(false)` disables preview tunnels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_tunnel: Option<bool>,
    /// Provider allowlist and locked instance fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<ManagedProviderSummaryLimits>,
    /// Update channel summary (`feedUrl` stripped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<ManagedUpdatesSummaryPolicy>,
}

/// What clients learn about the managed policy (`managedPolicy.ts:134-142`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedPolicySummary {
    /// Organization display name (`"Managed by <organization>"`).
    pub organization: String,
    /// Optional IT/helpdesk URL shown to managed users.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub support_url: Option<String>,
    /// Dot paths into `ServerSettings` that users cannot change.
    pub locked_settings: Vec<String>,
    /// Effective limits enforced by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ManagedPolicySummaryLimits>,
}

/// Returns whether preview tunnels are allowed by both settings and managed policy (`managedPolicy.ts`).
#[must_use]
pub fn allows_preview_tunnel(
    enable_preview_tunnel: bool,
    summary: Option<&ManagedPolicySummary>,
) -> bool {
    enable_preview_tunnel
        && summary
            .and_then(|s| s.limits.as_ref())
            .and_then(|limits| limits.preview_tunnel)
            != Some(false)
}
