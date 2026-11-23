//! Antigravity runtime launch specification, model suffix delegation, and mode mapping.

use std::collections::BTreeMap;

use legion_contracts::policy::RuntimeMode;
use legion_protocol::provider_env::SanitizedEnv;

use crate::core::ServerModelEntry;

use super::super::{ProviderLaunchSpec, build_child_env};
use super::auth::{
    AGY_CLI_DISABLE_AUTO_UPDATE_ENV, ANTIGRAVITY_REMOVED_ENV_KEYS, AntigravityAuthMethod,
    AntigravityInstallTarget, AntigravityInstanceDirectories,
};
pub use super::leases::{AntigravityVersionLeaseGuard, AntigravityVersionLeases};

/// Environment flag forcing Antigravity ACP file-backed session storage.
pub const ANTIGRAVITY_FORCE_FILE_STORAGE_ENV: &str = "AGY_ACP_FORCE_FILE_STORAGE";

const ISOLATED_SSH_CONNECTION_SENTINEL: &str = "127.0.0.1 0 127.0.0.1 0";

/// Base permission decision before Antigravity's `RuntimeMode::Auto` review override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityPermissionDisposition {
    /// Automatically approve without prompting.
    Approve,
    /// Prompt the user via ACP permission UI.
    Ask,
    /// Deny immediately (for example due to read-only or workspace sandbox).
    Deny,
}

/// Builds a sanitized Antigravity ACP subprocess launch specification (`BUG-12`).
///
/// Injects [`AGY_CLI_DISABLE_AUTO_UPDATE_ENV`] = `"1"` (`d1822be5a9`) so managed
/// `agy` binaries never self-update underneath `LegionCode`'s version leases.
#[must_use]
pub fn build_antigravity_launch_spec(
    env: &SanitizedEnv,
    target: &AntigravityInstallTarget,
    directories: &AntigravityInstanceDirectories,
    cwd: Option<&str>,
) -> ProviderLaunchSpec {
    build_antigravity_launch_spec_for_auth(
        env,
        target,
        directories,
        cwd,
        AntigravityAuthMethod::Custom,
    )
}

/// Builds a sanitized Antigravity ACP subprocess launch specification scoped to
/// `auth_method`, isolating `oauth-personal` runs via `SSH_CONNECTION` so `agy` 1.5
/// bypasses the shared OS keyring and isolates tokens per instance (`084ad5e1a0`).
#[must_use]
pub fn build_antigravity_launch_spec_for_auth(
    env: &SanitizedEnv,
    target: &AntigravityInstallTarget,
    directories: &AntigravityInstanceDirectories,
    cwd: Option<&str>,
    auth_method: AntigravityAuthMethod,
) -> ProviderLaunchSpec {
    let mut overrides = BTreeMap::new();
    overrides.insert("GEMINI_HOME".to_owned(), directories.gemini_home.clone());
    overrides.insert("TMPDIR".to_owned(), directories.runtime_temp.clone());
    overrides.insert(
        ANTIGRAVITY_FORCE_FILE_STORAGE_ENV.to_owned(),
        "1".to_owned(),
    );
    overrides.insert(AGY_CLI_DISABLE_AUTO_UPDATE_ENV.to_owned(), "1".to_owned());
    if auth_method.isolates_personal_oauth() {
        let ssh_connection = env
            .get("SSH_CONNECTION")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(ISOLATED_SSH_CONNECTION_SENTINEL)
            .to_owned();
        overrides.insert("SSH_CONNECTION".to_owned(), ssh_connection);
    }
    if let AntigravityInstallTarget::AcpServer {
        localharness_external,
        ..
    } = target
    {
        overrides.insert(
            "AGY_LOCALHARNESS_EXTERNAL_PATH".to_owned(),
            localharness_external.clone(),
        );
    }

    let args = match target {
        AntigravityInstallTarget::AgyCli { .. } => vec!["acp".to_owned()],
        AntigravityInstallTarget::AcpServer { .. } => Vec::new(),
    };

    ProviderLaunchSpec {
        executable: target.executable().to_owned(),
        args,
        env: build_child_env(env, ANTIGRAVITY_REMOVED_ENV_KEYS, &overrides),
        removed_env_keys: ANTIGRAVITY_REMOVED_ENV_KEYS
            .iter()
            .map(|k| (*k).to_owned())
            .collect(),
        cwd: cwd.map(ToOwned::to_owned),
        cancel_behavior: "immediate",
    }
}

/// Reasoning-level option descriptor attached to an Antigravity model family (`5ee1c2f652`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityThinkingDescriptor {
    /// ACP config option id (for example `"thinking_level"`).
    pub option_id: String,
    /// Valid option values advertised for this model family.
    pub allowed_values: Vec<String>,
}

/// Advertised Antigravity model family plus its optional reasoning option values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityModelCapability {
    /// Base catalog entry (`slug`, display names, `is_custom`, `supports_reasoning`).
    pub entry: ServerModelEntry,
    /// Optional aliases (e.g. legacy suffixed IDs).
    pub aliases: Vec<String>,
    /// Reasoning selector metadata when supported.
    pub thinking: Option<AntigravityThinkingDescriptor>,
}

/// Resolved suffixed model delegation (`gemini-3.8-flash-high` -> `gemini-3.8-flash` + thinking) (`5ee1c2f652`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravitySuffixedModel {
    /// Canonical model family slug.
    pub model: String,
    /// Config option id for reasoning level.
    pub option_id: String,
    /// Thinking suffix value extracted from the requested slug.
    pub value: String,
}

/// Resolves an Antigravity 1.3-style thinking-suffixed model ID against advertised base models (`5ee1c2f652`).
#[must_use]
pub fn resolve_antigravity_suffixed_model(
    models: &[AntigravityModelCapability],
    requested_model: &str,
) -> Option<AntigravitySuffixedModel> {
    let directly_advertised = models.iter().any(|candidate| {
        candidate.entry.slug.eq_ignore_ascii_case(requested_model)
            || candidate
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(requested_model))
    });
    if directly_advertised {
        return None;
    }
    let (family, value) = requested_model.rsplit_once('-')?;
    let base = models
        .iter()
        .find(|candidate| candidate.entry.slug == family)?;
    let thinking = base.thinking.as_ref()?;
    if thinking.allowed_values.iter().any(|choice| choice == value) {
        Some(AntigravitySuffixedModel {
            model: family.to_owned(),
            option_id: thinking.option_id.clone(),
            value: value.to_owned(),
        })
    } else {
        None
    }
}

/// Selects the saved thinking value for Antigravity text generation when supported by
/// the active ACP session config options and different from the current value (`e74b795535`).
#[must_use]
pub fn select_antigravity_text_gen_thinking(
    saved_options: &[(String, String)],
    thinking_option_id: &str,
    current_value: &str,
    supported_values: &[String],
) -> Option<String> {
    let (_, selected) = saved_options
        .iter()
        .find(|(id, _)| id == thinking_option_id)?;
    if selected != current_value && supported_values.iter().any(|v| v == selected) {
        Some(selected.clone())
    } else {
        None
    }
}

/// Maps `LegionCode` [`RuntimeMode`] to Antigravity's internal mode identifier (`5dd565d6b1`).
#[must_use]
pub const fn antigravity_permission_mode(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::ApprovalRequired => "default",
        RuntimeMode::AutoAcceptEdits => "auto_edit",
        RuntimeMode::Auto => "auto",
        RuntimeMode::FullAccess => "yolo",
    }
}

/// Maps Antigravity's ACP session mode back to `LegionCode` [`RuntimeMode`] (`5dd565d6b1`).
#[must_use]
pub fn antigravity_runtime_mode_for_session_mode(session_mode: &str) -> Option<RuntimeMode> {
    match session_mode {
        "yolo" => Some(RuntimeMode::FullAccess),
        "auto_edit" => Some(RuntimeMode::AutoAcceptEdits),
        "auto" => Some(RuntimeMode::Auto),
        "default" => Some(RuntimeMode::ApprovalRequired),
        _ => None,
    }
}

/// Chooses the session mode sent via `session/set_mode` given the agent's advertised modes (`5dd565d6b1`).
///
/// `agy` CLI rejects `"yolo"` and `"auto_edit"`, and sessions without native Auto reject `"auto"`,
/// so those stay in `"default"` where `LegionCode` answers each permission prompt according to the live policy.
#[must_use]
pub fn antigravity_session_mode<'a>(mode_id: &'a str, available_modes: &[&str]) -> &'a str {
    if mode_id == "auto" {
        if available_modes.contains(&"auto") {
            "auto"
        } else {
            "default"
        }
    } else if mode_id == "yolo" || mode_id == "auto_edit" {
        "default"
    } else {
        mode_id
    }
}

/// Applies Antigravity's `RuntimeMode::Auto` safety override onto a base permission decision (`5dd565d6b1`).
#[must_use]
pub const fn antigravity_permission_disposition(
    mode: RuntimeMode,
    base: AntigravityPermissionDisposition,
) -> AntigravityPermissionDisposition {
    match (mode, base) {
        (RuntimeMode::Auto, AntigravityPermissionDisposition::Deny) => {
            AntigravityPermissionDisposition::Deny
        }
        (RuntimeMode::Auto, _) => AntigravityPermissionDisposition::Ask,
        (_, other) => other,
    }
}

/// Applies Antigravity permission safety rules including built-in `LegionCode`/`T3` MCP tool
/// pre-approval in `Auto` and `AutoAcceptEdits` (`#39822`).
#[must_use]
pub fn antigravity_permission_disposition_for_tool(
    mode: RuntimeMode,
    approval_policy: Option<&str>,
    base: AntigravityPermissionDisposition,
    title: Option<&str>,
    raw_input: Option<&serde_json::Value>,
) -> AntigravityPermissionDisposition {
    if base == AntigravityPermissionDisposition::Deny {
        return AntigravityPermissionDisposition::Deny;
    }
    if matches!(mode, RuntimeMode::Auto | RuntimeMode::AutoAcceptEdits)
        && matches!(approval_policy, None | Some("never"))
        && super::mcp_tools::is_antigravity_legion_mcp_request(title, raw_input)
    {
        return AntigravityPermissionDisposition::Approve;
    }
    antigravity_permission_disposition(mode, base)
}

/// Default Antigravity supported modes before any session advertises native `Auto` (`#40154`).
pub const ANTIGRAVITY_DEFAULT_RUNTIME_MODES: &[RuntimeMode] = &[
    RuntimeMode::ApprovalRequired,
    RuntimeMode::AutoAcceptEdits,
    RuntimeMode::FullAccess,
];

/// Saved runtime modes per `auth_method` so `Auto` stays in the picker after restart (`#40154`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberedAntigravityRuntimeModes {
    /// Auth method identifier that observed these modes (`"oauth-personal"`, etc.).
    pub auth_method: String,
    /// Ordered supported runtime modes advertised by the last live session.
    pub modes: Vec<RuntimeMode>,
}

/// Resolves initial runtime modes for `auth_method`, restoring remembered modes only when
/// `auth_method` matches (`#40154`).
#[must_use]
pub fn resolve_initial_antigravity_runtime_modes(
    auth_method: &str,
    remembered: Option<&RememberedAntigravityRuntimeModes>,
) -> Vec<RuntimeMode> {
    if let Some(saved) = remembered
        && saved.auth_method == auth_method
        && !saved.modes.is_empty()
    {
        return saved.modes.clone();
    }
    ANTIGRAVITY_DEFAULT_RUNTIME_MODES.to_vec()
}

/// Parses remembered runtime modes JSON (`#40154`).
#[must_use]
pub fn parse_remembered_antigravity_runtime_modes(
    raw_json: &str,
) -> Option<RememberedAntigravityRuntimeModes> {
    serde_json::from_str(raw_json).ok()
}

/// Returns `true` when `next` modes differ from `previous` and should be persisted (`#40154`).
#[must_use]
pub fn should_persist_remembered_runtime_modes(
    previous: Option<&[RuntimeMode]>,
    next: &[RuntimeMode],
) -> bool {
    previous != Some(next)
}
