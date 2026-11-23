//! Unified provider driver specifications, sanitized environment wiring, and
//! provider-specific coordinators (`antigravity`, `grok`, built-ins, `dev_replay`).
//!
//! Every command, probe, and environment builder in this module operates strictly
//! over [`legion_protocol::provider_env::SanitizedEnv`] (`BUG-12`), guaranteeing zero
//! ambient host `std::env::var` reads.

pub mod acp_registry;
pub mod antigravity;
pub mod built_ins;
pub mod built_ins_acp;
pub mod built_ins_extra;
pub mod claude;
pub mod claude_lifecycle;
mod cli_probe_path;
pub mod codex;
pub mod codex_lifecycle;
mod codex_probe;
pub mod cursor_worker;
pub mod dev_replay;
pub mod grok;
pub mod opencode_pi;

use std::collections::BTreeMap;

use legion_contracts::ids::{IdError, ProviderDriverKind};
use legion_protocol::provider_env::SanitizedEnv;
use serde::{Deserialize, Serialize};

use crate::core::{ProbeAvailability, ServerModelEntry, UsageLimitsSnapshot};

pub use acp_registry::{
    AcpRegistryLocalFlavor, DeepSeekProbeIsolation, acp_registry_should_retry_with_refreshed_index,
    build_deepseek_probe_history_isolation, detect_acp_registry_local_flavor,
};
pub use antigravity::{
    ACP_HOST_SLEEP_TOLERANCE_MILLIS, ACP_PROMPT_INACTIVITY_SLICE_MILLIS,
    AGY_CLI_DISABLE_AUTO_UPDATE_ENV, ANTIGRAVITY_DEFAULT_RUNTIME_MODES,
    ANTIGRAVITY_FORCE_FILE_STORAGE_ENV, ANTIGRAVITY_POST_SETTLE_COMMAND_HOLD_MILLIS,
    ANTIGRAVITY_REMOVED_ENV_KEYS, ANTIGRAVITY_USER_RULE_FILES, AcpTurnTokenUsage,
    AntigravityAuthConfig, AntigravityAuthFlowPhase, AntigravityAuthMethod, AntigravityAuthState,
    AntigravityCliToken, AntigravityCliTokenStatus, AntigravityDiscoveredSkill,
    AntigravityInstallTarget, AntigravityInstanceDirectories, AntigravityModelCapability,
    AntigravityOAuthKind, AntigravityPendingAuth, AntigravityPermissionDisposition,
    AntigravityProbeOutcome, AntigravitySecretDecodeError, AntigravitySkillsConfig,
    AntigravitySuffixedModel, AntigravityThinkingDescriptor, AntigravityTurnFailure,
    AntigravityTurnPhase, AntigravityVersionLeaseGuard, AntigravityVersionLeases,
    COMMAND_LINE_TRUNCATED, CompleteJsonlTail, InactivitySliceAction, LEGION_MCP_SERVER_NAMES,
    ManagedReleaseIntegrity, PLANNER_RESPONSE_STEP_TYPE, PostSettleCommandHoldAction,
    PostSettleCommandHoldState, ProviderCatalogModels, RawAcpUsageCounts,
    RememberedAntigravityRuntimeModes, SkillConfigEntry, acp_per_turn_prompt_usage,
    antigravity_command_outlives_prompt, antigravity_model_id_lookup,
    antigravity_permission_disposition, antigravity_permission_disposition_for_tool,
    antigravity_permission_mode, antigravity_rate_model, antigravity_runtime_mode_for_session_mode,
    antigravity_session_mode, antigravity_tool_outlives_prompt, antigravity_usage_profile_dirs,
    bound_antigravity_command_line, bound_antigravity_command_text, build_antigravity_launch_spec,
    build_antigravity_launch_spec_for_auth, can_steer_antigravity_turn, catalog_model_label,
    classify_antigravity_turn_failure, clear_stale_auth_error, decode_go_keyring_secret,
    discover_antigravity_skills, evaluate_antigravity_cli_token, evaluate_post_settle_command_hold,
    evaluate_turn_inactivity_slice, has_unread_waking_inbox_message, inspect_managed_release,
    is_antigravity_background_wait_tool, is_antigravity_conversation_db_tables,
    is_antigravity_legion_mcp_request, is_antigravity_subagent_killed, is_known_legion_mcp_tool,
    parse_antigravity_authorization_url, parse_antigravity_cli_token_json,
    parse_remembered_antigravity_runtime_modes, probe_antigravity, read_complete_jsonl_tail,
    reconcile_antigravity_snapshot, resolve_antigravity_instance_directories,
    resolve_antigravity_suffixed_model, resolve_initial_antigravity_runtime_modes,
    resolve_skills_config_path, sanitize_antigravity_command_fields,
    select_antigravity_text_gen_thinking, should_finalize_worktree_handoff_turn,
    should_persist_remembered_runtime_modes, sync_antigravity_user_rule_copies,
    validate_antigravity_login_response,
};
pub use built_ins::{
    BUILTIN_DRIVER_SPECS, CLAUDE_HOST_SESSION_VARIABLES, ClaudeExecutableError, CursorSidecarSpec,
    DISCOVERY_CLI_DRIVERS, FX_ACP_CANCEL_BEHAVIOR, FxLaunchSpec, KIRO_AUTOPILOT_CONFIG_ID,
    KIRO_EFFORT_CONFIG_ID, KiroLaunchSpec, MUSE_PROTOCOL_VERSION, PiLaunchError, RegistryExecError,
    build_claude_env, builtin_cli_probe_spec, codex_app_server_args, codex_text_gen_args,
    codex_thread_start_envelope, codex_turn_start_envelope, cursor_isolated_sidecar_spec,
    fill_compaction_recents, find_builtin_driver_spec, fx_launch_spec, fx_session_mode_for_policy,
    fx_tool_call_was_cancelled, kiro_autopilot_value, kiro_launch_spec, muse_launch_spec,
    muse_verbatim_path, normalize_driver_slug, normalize_registry_command_path,
    opencode_basic_auth_header, opencode_server_env, pi_launch_args, pi_probe_spec,
    resolve_claude_executable, validate_registry_executable,
};
pub use claude::{
    CLAUDE_ACTIVE_STEERING_INTERRUPTS_TOOLS, CLAUDE_LAUNCH_VALUE_FLAGS, ClaudeAssistantPhase,
    ClaudeTextGenRetryDecision, ClaudeTextOnlyModelsCache, NO_TEXT_GENERATION_PROVIDER_MESSAGE,
    check_text_generation_provider_usable, classify_claude_text_gen_retry,
    claude_fable_supports_fast_mode, claude_gateway_cooldown_reset_at,
    claude_launch_arg_plugin_directories, is_claude_images_unsupported_error,
    phase_for_held_steer_result, resolve_claude_inline_plugin_directories,
    summarize_claude_tool_request, truncate_without_trailing_high_surrogate,
};
pub use claude_lifecycle::{
    CLAUDE_HOMEBREW_LATEST_SWITCH_COMMAND, CLAUDE_HOMEBREW_STABLE_UPDATE_COMMAND,
    ClaudeCompletedThinkingBlock, ClaudeHistoryEntry, ClaudeLiveQueryReuse,
    ClaudeLostBackgroundWorkPlan, ClaudeModelSwitchRestartCheck, ClaudeThinkingTracker,
    ClaudeTrackedBackgroundTask, claude_history_ends_on_failed_request,
    claude_program_was_replaced, claude_resume_interrupted_turn_env,
    claude_writes_updates_as_thinking, format_claude_version_upgrade_message,
    resolve_claude_lost_background_work, should_start_updated_claude_on_model_switch,
};
pub use cli_probe_path::{
    AcpTerminalInvocation, build_acp_terminal_invocation, classify_cli_probe_output,
    did_host_path_change, extract_cli_version, find_executable_on_path, git_for_windows_bash,
    instances_needing_rebuild_after_path_change, list_login_shell_candidates, merge_path_entries,
    resolve_cli_probe_command, resolve_host_path_value, summarize_antigravity_probe,
};
pub use codex::{
    CODEX_INVALID_ENCRYPTED_CONTENT_CODE, CODEX_INVALID_ENCRYPTED_HISTORY_MESSAGE,
    CodexBinaryFingerprint, CodexHistoryRecoveryDecision, CodexLocalBinaryKind,
    CodexLocalProbeCache, CodexLocalProbeRecord, CodexManagedFailure, classify_codex_managed_error,
    codex_auto_command_approval_decision, codex_should_auto_accept_command_approval,
    codex_should_recover_rejected_history, codex_stdio_mcp_server_names, codex_thread_env_name,
    codex_thread_runtime_config_overrides, dedupe_web_search_queries,
    inspect_codex_binary_fingerprint, parse_codex_cli_version, resolve_codex_launch_args,
    resolve_codex_launch_args_with_lookup, tokenize_cli_args,
};
pub use codex_lifecycle::{
    CODEX_APP_SERVER_REQUEST_DEADLINE_MS, CODEX_APP_SERVER_RETIRED_TAG,
    CODEX_APP_SERVER_UNRESPONSIVE_TAG, CodexAppServerLifecycleError, CodexAppServerRetirementState,
    CodexIdleThreadUnloadRegistry, IdleThreadUnloadEntry, is_load_key_for_provider_thread,
    provider_thread_runtime_key,
};
pub use cursor_worker::{
    CURSOR_BACKGROUND_WAIT_INSTRUCTIONS, CURSOR_SUPPORTS_BACKGROUND_WAIT,
    CursorRuntimeInstructionTracker, CursorSdkErrorKind, CursorSdkModel, CursorSdkModelSelection,
    CursorSdkParam, CursorSdkVariant, CursorTrackedBackgroundShell, CursorWorkerMessage,
    CursorWorkerRequest, MAX_WORKER_ERROR_CAUSE_DEPTH, SerializedBackgroundShellEvent,
    SerializedWorkerError, await_cursor_background_shells, cursor_background_shell_detail,
    cursor_sdk_model_selection, cursor_turn_prompt_dropped, decode_cursor_worker_request,
    encode_cursor_worker_message, is_delivered_provider_turn,
};
pub use dev_replay::{
    DevReplayScenario, DevReplayTurnScript, build_dev_replay_script, generate_dev_replay_events,
    materialize_template, stream_word_chunks,
};
pub use grok::{
    GROK_ACP_CANCEL_TRIGGER, GROK_NPM_PACKAGE, GROK_OAUTH2_REFERRER, GrokUpdatePlan,
    build_grok_launch_spec, grok_acp_cancel_meta, grok_acp_initialize_meta,
    grok_should_refresh_on_interval, grok_slash_commands_from_initialize,
    parse_grok_models_cli_output, resolve_grok_maintenance,
};
pub use opencode_pi::{
    DEFAULT_VARIANT_CHOICE_ID, OpenCodeCommandCompletionStatus, OpenCodeReleaseMeta,
    OpenCodeSessionActivity, OpenCodeVariantDescriptor, OpenCodeVariantOption, PiAsyncChildStep,
    mark_open_code_legacy, open_code_price, open_code_release_line, open_code_variant_descriptor,
    open_code_wire_variant, opencode_should_complete_command_immediately,
    opencode_usage_error_message, pi_async_run_should_settle, reasoning_variant_label,
};

/// Static capability and runtime descriptor shared across all `LegionCode` providers.
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
/// use legion_protocol::provider_env::SanitizedEnv;
/// use legion_providers::drivers::build_child_env;
///
/// let base = SanitizedEnv::from_entries([("PATH", "/usr/bin"), ("SECRET", "old")], false);
/// let mut overrides = BTreeMap::new();
/// overrides.insert("GROK_OAUTH2_REFERRER".to_owned(), "legioncode".to_owned());
/// let env = build_child_env(&base, &["SECRET"], &overrides);
/// assert!(!env.contains_key("SECRET"));
/// assert_eq!(env.get("GROK_OAUTH2_REFERRER").map(String::as_str), Some("legioncode"));
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
