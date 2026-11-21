//! Unified provider driver specifications, sanitized environment wiring, and
//! provider-specific coordinators (`antigravity`, `grok`, built-ins, `dev_replay`).
//!
//! Every command, probe, and environment builder in this module operates strictly
//! over [`t3_protocol::provider_env::SanitizedEnv`] (`BUG-12`), guaranteeing zero
//! ambient host `std::env::var` reads.

pub mod antigravity;
pub mod built_ins;
pub mod built_ins_acp;
pub mod built_ins_extra;
pub mod dev_replay;
pub mod grok;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3_contracts::ids::{IdError, ProviderDriverKind};
use t3_protocol::provider_env::SanitizedEnv;

use crate::core::{ProbeAvailability, ServerModelEntry, UsageLimitsSnapshot};

pub use antigravity::{
    ANTIGRAVITY_FORCE_FILE_STORAGE_ENV, ANTIGRAVITY_REMOVED_ENV_KEYS, AntigravityAuthConfig,
    AntigravityInstallTarget, AntigravityInstanceDirectories, AntigravityProbeOutcome,
    AntigravityVersionLeaseGuard, AntigravityVersionLeases, build_antigravity_launch_spec,
    probe_antigravity, reconcile_antigravity_snapshot, resolve_antigravity_instance_directories,
};
pub use built_ins::{
    BUILTIN_DRIVER_SPECS, CLAUDE_HOST_SESSION_VARIABLES, ClaudeExecutableError, CursorSidecarSpec,
    FX_ACP_CANCEL_BEHAVIOR, FxLaunchSpec, KIRO_AUTOPILOT_CONFIG_ID, KIRO_EFFORT_CONFIG_ID,
    KiroLaunchSpec, MINIMUM_OPENCODE_VERSION, MINIMUM_PI_VERSION, MUSE_PROTOCOL_VERSION,
    PiLaunchError, RegistryExecError, build_claude_env, codex_app_server_args, codex_text_gen_args,
    codex_thread_start_envelope, codex_turn_start_envelope, cursor_isolated_sidecar_spec,
    fill_compaction_recents, fx_launch_spec, fx_session_mode_for_policy,
    fx_tool_call_was_cancelled, kiro_autopilot_value, kiro_launch_spec, muse_launch_spec,
    muse_verbatim_path, normalize_registry_command_path, opencode_basic_auth_header,
    opencode_server_env, pi_launch_args, pi_probe_spec, resolve_claude_executable,
    validate_registry_executable,
};
pub use dev_replay::{
    DevReplayScenario, DevReplayTurnScript, build_dev_replay_script, generate_dev_replay_events,
    materialize_template, stream_word_chunks,
};
pub use grok::{
    GROK_ACP_CANCEL_TRIGGER, GROK_NPM_PACKAGE, GROK_OAUTH2_REFERRER, GrokUpdatePlan,
    build_grok_launch_spec, grok_acp_cancel_meta, grok_acp_initialize_meta,
    grok_slash_commands_from_initialize, parse_grok_models_cli_output, resolve_grok_maintenance,
};

/// Static capability and runtime descriptor shared across all T3 Code providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDriverSpec {
    /// Canonical driver slug matching [`ProviderDriverKind`].
    pub slug: &'static str,
    /// Human-readable driver display name.
    pub display_name: &'static str,
    /// Default CLI executable name searched on `PATH`.
    pub default_binary: &'static str,
    /// Whether this driver communicates via ACP JSON-RPC over stdio.
    pub supports_acp: bool,
    /// Whether this driver supports lightweight text generation (titles/commits/PRs).
    pub supports_text_generation: bool,
    /// Cancellation mode identifier (`"immediate"`, `"ctrl_c"`, `"wait-for-prompt"`, `"interrupt"`).
    pub cancel_behavior: &'static str,
}

impl ProviderDriverSpec {
    /// Parses [`Self::slug`] into a validated [`ProviderDriverKind`].
    pub fn driver_kind(self) -> Result<ProviderDriverKind, IdError> {
        ProviderDriverKind::new(self.slug)
    }
}

/// Resolved subprocess launch command, arguments, and sanitized child env.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLaunchSpec {
    /// Executable binary path or command name.
    pub executable: String,
    /// Ordered CLI arguments passed to the child process.
    pub args: Vec<String>,
    /// Final sanitized child environment variable map (with removals & overrides applied).
    pub env: BTreeMap<String, String>,
    /// Environment keys explicitly removed from the sanitized base environment.
    pub removed_env_keys: Vec<String>,
    /// Optional working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Cancellation mechanism advertised by this launch configuration.
    pub cancel_behavior: &'static str,
}

/// Resolved short-lived probe specification built from [`SanitizedEnv`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProbeSpec {
    /// Executable binary path or command name.
    pub executable: String,
    /// Ordered CLI flags for the probe invocation.
    pub args: Vec<String>,
    /// Final sanitized probe environment map (including offline flags where appropriate).
    pub env: BTreeMap<String, String>,
    /// Keys stripped prior to probing.
    pub removed_env_keys: Vec<String>,
}

/// Availability status emitted by a provider binary/auth probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProbeStatus {
    /// Executable is present and credentials are configured.
    Ready,
    /// Executable is present, but authentication setup is incomplete.
    NeedsAuth,
    /// Executable or local harness runtime could not be found.
    NotInstalled,
    /// Executable is present, but probing failed transiently or unexpectedly.
    Error,
}

impl From<ProviderProbeStatus> for ProbeAvailability {
    fn from(status: ProviderProbeStatus) -> Self {
        match status {
            ProviderProbeStatus::Ready => Self::Ready,
            ProviderProbeStatus::NeedsAuth => Self::NeedsAuth,
            ProviderProbeStatus::NotInstalled => Self::NotInstalled,
            ProviderProbeStatus::Error => Self::TransientError,
        }
    }
}

/// Cached/live provider state reconciled across probes and runtime refreshes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshotState {
    /// Whether an installation is currently detected on disk.
    pub installed: bool,
    /// Latest probe readiness status.
    pub status: ProviderProbeStatus,
    /// Whether authentication credentials appear valid.
    pub auth_configured: bool,
    /// Normalized provider models.
    pub models: Vec<ServerModelEntry>,
    /// Advertised slash commands.
    pub slash_commands: Vec<String>,
    /// Discovered agent skills.
    pub skills: Vec<String>,
    /// Current usage limits if known.
    pub usage_limits: Option<UsageLimitsSnapshot>,
    /// Whether lightweight text generation is enabled for this provider.
    pub supports_text_generation: bool,
    /// Optional status/error message for UI surfaces.
    pub message: Option<String>,
}

/// Derives a child environment map strictly from `base` [`SanitizedEnv`], stripping
/// `removed_keys` (case-insensitively on Windows) before applying `overrides`.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use t3_protocol::provider_env::SanitizedEnv;
/// use t3_providers::drivers::build_child_env;
///
/// let base = SanitizedEnv::from_entries([("PATH", "/usr/bin"), ("SECRET", "old")], false);
/// let mut overrides = BTreeMap::new();
/// overrides.insert("GROK_OAUTH2_REFERRER".to_owned(), "t3code".to_owned());
/// let env = build_child_env(&base, &["SECRET"], &overrides);
/// assert!(!env.contains_key("SECRET"));
/// assert_eq!(env.get("GROK_OAUTH2_REFERRER").map(String::as_str), Some("t3code"));
/// ```
#[must_use]
pub fn build_child_env(
    base: &SanitizedEnv,
    removed_keys: &[&str],
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let is_windows = base.is_windows();
    for (k, v) in base.as_map() {
        let removed = removed_keys.iter().any(|target| {
            if is_windows {
                k.eq_ignore_ascii_case(target)
            } else {
                k == target
            }
        });
        if !removed {
            env.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in overrides {
        if is_windows {
            env.retain(|existing, _| !existing.eq_ignore_ascii_case(k));
        }
        env.insert(k.clone(), v.clone());
    }
    env
}
