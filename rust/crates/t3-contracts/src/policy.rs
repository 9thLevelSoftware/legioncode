//! Machine-wide managed policy schema, deterministic fingerprinting, and templates.
//!
//! Mirrors `packages/contracts/src/managedPolicy.ts`,
//! `packages/contracts/src/providerPolicy.ts`, `apps/server/src/policy/ManagedPolicy.ts`,
//! `apps/server/src/cli/policy.ts`, and `apps/server/src/policy/policyTemplates.ts`.
//!
//! # Examples
//!
//! ```
//! use t3_contracts::policy::{
//!     ManagedPolicyDocument, policy_fingerprint_12hex, sample_policy,
//! };
//!
//! let sample = sample_policy();
//! assert_eq!(policy_fingerprint_12hex(&sample)?, "13ab1a3e32bc");
//!
//! let minimal: ManagedPolicyDocument = serde_json::from_str(
//!     r#"{"schemaVersion":1,"organization":"Minimal Org"}"#,
//! )?;
//! assert_eq!(policy_fingerprint_12hex(&minimal)?, "a95193de6477");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod describe;
mod macos_template;
mod manageable_settings;
mod serde_helpers;
mod summary;
mod windows_data;
mod windows_templates;

use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::ids::ProviderDriverKind;

use serde_helpers::{
    deserialize_env_var_name, deserialize_opt_https_url, deserialize_opt_non_empty_vec,
    deserialize_opt_trimmed_non_empty, deserialize_schema_version_1, deserialize_trimmed_non_empty,
};

pub use describe::{
    LoadedPolicySummary, RunningServerState, describe_policy, describe_running_server,
};
pub use macos_template::{PolicyTemplateError, macos_profile};
pub use manageable_settings::{describe_manageable_settings, list_manageable_settings};
pub use summary::{
    ManagedPolicySummary, ManagedPolicySummaryLimits, ManagedProviderSummaryLimits,
    ManagedUpdatesSummaryPolicy, allows_preview_tunnel,
};
pub use windows_data::{
    KNOWN_DRIVER_LABELS, WINDOWS_POLICY_GROUPS, WINDOWS_POLICY_KEY, WINDOWS_POLICY_LISTS,
    WINDOWS_POLICY_VALUES,
};
pub use windows_templates::{windows_adml, windows_admx};

/// Errors produced when validating, serializing, or fingerprinting a policy document.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PolicyError {
    /// Serialization to JSON failed.
    #[error("failed to serialize managed policy JSON: {0}")]
    Json(String),
    /// Validation of policy document invariants failed.
    #[error("invalid managed policy document: {0}")]
    Validation(String),
}

/// Thread permission mode enforced on provider turns (`providerPolicy.ts:25-30`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMode {
    /// Prompt the user for approval before tool execution (`Supervised`).
    ApprovalRequired,
    /// Automatically accept file edits while prompting for shell commands.
    AutoAcceptEdits,
    /// Run autonomously with workspace safety checks (`Auto`).
    Auto,
    /// Bypass runtime permission checks (`Full access`).
    FullAccess,
}

impl RuntimeMode {
    /// Numeric privilege rank matching `RUNTIME_MODE_RANK` in `providerPolicy.ts:34-39`.
    pub fn rank(self) -> u8 {
        match self {
            Self::ApprovalRequired => 0,
            Self::AutoAcceptEdits => 1,
            Self::Auto => 2,
            Self::FullAccess => 3,
        }
    }

    /// Returns `true` if `self` grants broader permissions than `ceiling`.
    pub fn is_above(self, ceiling: Option<Self>) -> bool {
        ceiling.is_some_and(|max| self.rank() > max.rank())
    }
}

/// Client surface permitted to connect to a policy-managed server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedPolicyClient {
    /// Desktop Electron/Tauri shell on the host machine.
    Desktop,
    /// Local browser session on loopback.
    LocalBrowser,
    /// Remote browsers, phones, and `app.t3.codes`.
    Remote,
}

/// Update behavior policy mode (`auto` or `off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedUpdatesMode {
    /// Checks for updates; installs when requested.
    Auto,
    /// Never checks for updates (MDM-managed installs).
    Off,
}

/// One administrator-managed server setting (`managedPolicy.ts:13-16`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedPolicySetting {
    /// Setting value in `ServerSettings` JSON representation.
    pub value: Value,
    /// When `true`, users and agents cannot override this setting.
    pub locked: bool,
}

/// Environment variable pinned by policy on a provider driver (`managedPolicy.ts:28`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedProviderEnvVar {
    /// POSIX/Windows environment variable name.
    #[serde(deserialize_with = "deserialize_env_var_name")]
    pub name: String,
    /// Plain environment variable value.
    pub value: String,
}

/// Provider instance overrides pinned by an administrator (`managedPolicy.ts:24-33`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedProviderInstance {
    /// Whether users are blocked from overriding these pinned fields.
    pub locked: bool,
    /// Environment variables merged into instances of this driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Vec<ManagedProviderEnvVar>>,
    /// Pinned executable binary path.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub binary_path: Option<String>,
    /// Pinned CLI launch arguments.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub launch_args: Option<String>,
}

/// Provider driver and instance restrictions (`managedPolicy.ts:36-40`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedProviderLimits {
    /// Non-empty list of permitted provider drivers; absent allows all drivers.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_non_empty_vec"
    )]
    pub allowed_drivers: Option<Vec<ProviderDriverKind>>,
    /// Per-driver pinned instance configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<IndexMap<ProviderDriverKind, ManagedProviderInstance>>,
}

/// Update channel and mirror restrictions (`managedPolicy.ts:56-62`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedUpdatesPolicy {
    /// Automatic check or completely disabled updates.
    pub mode: ManagedUpdatesMode,
    /// Optional custom HTTPS update feed mirror URL.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_https_url"
    )]
    pub feed_url: Option<String>,
    /// Whether nightly releases are permitted (`false` blocks nightly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_nightly: Option<bool>,
}

/// Hard policy limits that users cannot opt out of (`managedPolicy.ts:73-86`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedPolicyLimits {
    /// Broadest runtime permission mode any thread may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime_mode: Option<RuntimeMode>,
    /// Non-empty list of allowed client categories.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_non_empty_vec"
    )]
    pub clients: Option<Vec<ManagedPolicyClient>>,
    /// `Some(false)` turns T3 Connect off.
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
    /// `Some(false)` disables preview tunnels so remote clients cannot reach loopback ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_tunnel: Option<bool>,
    /// Provider allowlist and pinned instance settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<ManagedProviderLimits>,
    /// Update channel and mirror policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<ManagedUpdatesPolicy>,
}

/// Machine-wide policy document deployed by an administrator (`managedPolicy.ts:123-130`).
///
/// Field order matches the TypeScript `Schema.Struct` definition so compact JSON
/// serialization produces identical SHA-256 fingerprints across TS and Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManagedPolicyDocument {
    /// Policy schema version (must be `1`).
    #[serde(deserialize_with = "deserialize_schema_version_1")]
    pub schema_version: u32,
    /// Organization display name (`"Managed by <organization>"`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub organization: String,
    /// Optional IT/helpdesk URL shown to managed users.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub support_url: Option<String>,
    /// Managed `ServerSettings` defaults and locks keyed by dot path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<IndexMap<String, ManagedPolicySetting>>,
    /// Hard capability and provider restrictions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ManagedPolicyLimits>,
}

/// Built-in sample policy matching `SAMPLE_POLICY` in `apps/server/src/cli/policy.ts:110-114`.
pub static SAMPLE_POLICY: LazyLock<ManagedPolicyDocument> = LazyLock::new(sample_policy);

/// Constructs the sample managed policy matching `apps/server/src/cli/policy.ts:110-114`.
pub fn sample_policy() -> ManagedPolicyDocument {
    ManagedPolicyDocument {
        schema_version: 1,
        organization: "Example Corp".to_owned(),
        support_url: None,
        settings: None,
        limits: Some(ManagedPolicyLimits {
            max_runtime_mode: Some(RuntimeMode::AutoAcceptEdits),
            clients: Some(vec![ManagedPolicyClient::Desktop]),
            telemetry: Some(false),
            preview_tunnel: Some(false),
            ..ManagedPolicyLimits::default()
        }),
    }
}

/// Computes the 12-character lowercase SHA-256 hex fingerprint of `doc`.
///
/// Matches `ManagedPolicy.loadedPolicy` in `apps/server/src/policy/ManagedPolicy.ts:48-65`:
/// compact JSON encoded in exact `ManagedPolicyDocument` field order with `None` fields
/// omitted, hashed with SHA-256, and truncated to the first 12 lowercase hex characters.
pub fn policy_fingerprint_12hex(doc: &ManagedPolicyDocument) -> Result<String, PolicyError> {
    let json = serde_json::to_vec(doc).map_err(|err| PolicyError::Json(err.to_string()))?;
    let digest = Sha256::digest(&json);
    let encoded = hex::encode(digest);
    Ok(encoded[..12].to_owned())
}
