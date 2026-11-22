//! Antigravity ACP driver: install-first probing (`BUG-11`), SHA-256
//! `GEMINI_HOME` derivation, OAuth token/flow isolation, skills, lifecycle, and usage.

pub mod auth;
pub mod auth_flow;
pub mod auth_oauth;
pub mod failure_bounds;
pub mod leases;
pub mod lifecycle;
pub mod runtime;
pub mod skills;
mod skills_pattern;
pub mod skills_sync;
pub mod usage;

pub use auth::{
    AGY_CLI_DISABLE_AUTO_UPDATE_ENV, ANTIGRAVITY_REMOVED_ENV_KEYS, AntigravityAuthConfig,
    AntigravityAuthMethod, AntigravityInstallTarget, AntigravityInstanceDirectories,
    AntigravityProbeOutcome, probe_antigravity, reconcile_antigravity_snapshot,
    resolve_antigravity_instance_directories,
};
pub use auth_flow::{
    AntigravityAuthFlowPhase, AntigravityAuthState, AntigravityOAuthKind, AntigravityPendingAuth,
    ManagedReleaseIntegrity, clear_stale_auth_error, inspect_managed_release,
    parse_antigravity_authorization_url, validate_antigravity_login_response,
};
pub use auth_oauth::{
    AntigravityCliToken, AntigravityCliTokenStatus, AntigravitySecretDecodeError,
    decode_go_keyring_secret, evaluate_antigravity_cli_token, parse_antigravity_cli_token_json,
};
pub use leases::{AntigravityVersionLeaseGuard, AntigravityVersionLeases};
pub use lifecycle::{
    ACP_HOST_SLEEP_TOLERANCE_MILLIS, ACP_PROMPT_INACTIVITY_SLICE_MILLIS, AntigravityTurnFailure,
    CompleteJsonlTail, InactivitySliceAction, PLANNER_RESPONSE_STEP_TYPE,
    antigravity_tool_outlives_prompt, bound_antigravity_command_text, can_steer_antigravity_turn,
    classify_antigravity_turn_failure, evaluate_turn_inactivity_slice,
    has_unread_waking_inbox_message, is_antigravity_background_wait_tool,
    is_antigravity_subagent_killed, read_complete_jsonl_tail, sanitize_antigravity_command_fields,
    should_finalize_worktree_handoff_turn,
};
pub use runtime::{
    ANTIGRAVITY_FORCE_FILE_STORAGE_ENV, AntigravityModelCapability,
    AntigravityPermissionDisposition, AntigravitySuffixedModel, AntigravityThinkingDescriptor,
    antigravity_permission_disposition, antigravity_permission_mode,
    antigravity_runtime_mode_for_session_mode, antigravity_session_mode,
    build_antigravity_launch_spec, build_antigravity_launch_spec_for_auth,
    resolve_antigravity_suffixed_model, select_antigravity_text_gen_thinking,
};
pub use skills::{
    ANTIGRAVITY_USER_RULE_FILES, AntigravityDiscoveredSkill, AntigravitySkillsConfig,
    SkillConfigEntry, discover_antigravity_skills, resolve_skills_config_path,
    sync_antigravity_user_rule_copies,
};
pub use usage::{
    AcpTurnTokenUsage, ProviderCatalogModels, RawAcpUsageCounts, acp_per_turn_prompt_usage,
    antigravity_model_id_lookup, antigravity_rate_model, catalog_model_label,
    is_antigravity_conversation_db_tables,
};
