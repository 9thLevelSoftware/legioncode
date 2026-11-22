//! Built-in Kiro and `fx` ACP driver launch and mode-mapping helpers.

use std::collections::BTreeMap;

use legion_contracts::policy::RuntimeMode;
use legion_protocol::provider_env::SanitizedEnv;
use serde_json::{Value, json};

use crate::acp::ToolCallStatus;

use super::{ProviderLaunchSpec, build_child_env};

/// Kiro session config identifier toggling autopilot mode.
pub const KIRO_AUTOPILOT_CONFIG_ID: &str = "autopilot";
/// Kiro session config identifier selecting reasoning effort level.
pub const KIRO_EFFORT_CONFIG_ID: &str = "effortLevel";
/// Cancel semantics identifier for `fx acp` turns.
pub const FX_ACP_CANCEL_BEHAVIOR: &str = "wait-for-prompt";

/// Resolved Kiro ACP launch and config-option descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroLaunchSpec {
    /// Subprocess launch command & sanitized env.
    pub launch: ProviderLaunchSpec,
    /// Value (`"on"` or `"off"`) applied to ACP `autopilot` config option.
    pub autopilot: &'static str,
    /// Optional `effortLevel` ACP config option value.
    pub effort_level: Option<String>,
}

/// Computes Kiro `autopilot` value: `"on"` strictly for `FullAccess` without
/// manual sandbox/approval overrides.
#[must_use]
pub const fn kiro_autopilot_value(
    runtime_mode: RuntimeMode,
    has_manual_override: bool,
) -> &'static str {
    if matches!(runtime_mode, RuntimeMode::FullAccess) && !has_manual_override {
        "on"
    } else {
        "off"
    }
}

/// Builds a sanitized Kiro ACP v3 launch specification (`BUG-12`).
#[must_use]
pub fn kiro_launch_spec(
    env: &SanitizedEnv,
    binary_path: &str,
    cwd: Option<&str>,
    runtime_mode: RuntimeMode,
    has_manual_override: bool,
    effort_level: Option<&str>,
) -> KiroLaunchSpec {
    let args = vec![
        "acp".to_owned(),
        "--agent-engine=v3".to_owned(),
        "--auth-method=cli".to_owned(),
    ];
    KiroLaunchSpec {
        launch: ProviderLaunchSpec {
            executable: binary_path.to_owned(),
            args,
            env: build_child_env(env, &[], &BTreeMap::new()),
            removed_env_keys: Vec::new(),
            cwd: cwd.map(ToOwned::to_owned),
            cancel_behavior: "immediate",
        },
        autopilot: kiro_autopilot_value(runtime_mode, has_manual_override),
        effort_level: effort_level
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
    }
}

/// Resolved `fx` ACP launch specification including session `_meta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxLaunchSpec {
    /// Subprocess launch command & sanitized env.
    pub launch: ProviderLaunchSpec,
    /// ACP session mode string mapped from [`RuntimeMode`].
    pub session_mode: &'static str,
    /// Session `_meta` payload (`{ "fx": { "profileMcpServers": true } }`).
    pub session_meta: Value,
}

/// Maps `LegionCode` [`RuntimeMode`] to `fx` ACP session mode identifier.
#[must_use]
pub const fn fx_session_mode_for_policy(runtime_mode: RuntimeMode) -> &'static str {
    match runtime_mode {
        RuntimeMode::FullAccess => "bypassPermissions",
        RuntimeMode::AutoAcceptEdits => "acceptEdits",
        RuntimeMode::Auto => "default",
        RuntimeMode::ApprovalRequired => "ask",
    }
}

/// Detects whether an `fx` tool-call status/content indicates user cancellation.
#[must_use]
pub fn fx_tool_call_was_cancelled(status: &ToolCallStatus, text: Option<&str>) -> bool {
    if matches!(status, ToolCallStatus::Unknown(s) if s.eq_ignore_ascii_case("cancelled")) {
        return true;
    }
    *status == ToolCallStatus::Failed
        && text.is_some_and(|t| t.to_ascii_lowercase().contains("cancelled"))
}

/// Builds a sanitized `fx acp` launch specification (`BUG-12`).
#[must_use]
pub fn fx_launch_spec(
    env: &SanitizedEnv,
    binary_path: &str,
    cwd: Option<&str>,
    runtime_mode: RuntimeMode,
) -> FxLaunchSpec {
    FxLaunchSpec {
        launch: ProviderLaunchSpec {
            executable: binary_path.to_owned(),
            args: vec!["acp".to_owned()],
            env: build_child_env(env, &[], &BTreeMap::new()),
            removed_env_keys: Vec::new(),
            cwd: cwd.map(ToOwned::to_owned),
            cancel_behavior: FX_ACP_CANCEL_BEHAVIOR,
        },
        session_mode: fx_session_mode_for_policy(runtime_mode),
        session_meta: json!({ "fx": { "profileMcpServers": true } }),
    }
}
