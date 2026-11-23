//! Antigravity ACP driver: install-first probing (`BUG-11`), SHA-256
//! `GEMINI_HOME` derivation, OAuth token/flow isolation, skills, lifecycle, and usage.

pub mod auth;
pub mod auth_flow;
pub mod auth_oauth;
pub mod failure_bounds;
pub mod leases;
pub mod lifecycle;
pub mod mcp_tools;
pub mod runtime;
pub mod skills;
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
pub use failure_bounds::{COMMAND_LINE_TRUNCATED, bound_antigravity_command_line};
pub use leases::{AntigravityVersionLeaseGuard, AntigravityVersionLeases};
pub use lifecycle::{
    ACP_HOST_SLEEP_TOLERANCE_MILLIS, ACP_PROMPT_INACTIVITY_SLICE_MILLIS,
    ANTIGRAVITY_POST_SETTLE_COMMAND_HOLD_MILLIS, AntigravityTurnFailure, AntigravityTurnPhase,
    CompleteJsonlTail, InactivitySliceAction, PLANNER_RESPONSE_STEP_TYPE,
    PostSettleCommandHoldAction, PostSettleCommandHoldState, antigravity_command_outlives_prompt,
    antigravity_tool_outlives_prompt, bound_antigravity_command_text, can_steer_antigravity_turn,
    classify_antigravity_turn_failure, evaluate_post_settle_command_hold,
    evaluate_turn_inactivity_slice, has_unread_waking_inbox_message,
    is_antigravity_background_wait_tool, is_antigravity_subagent_killed, read_complete_jsonl_tail,
    sanitize_antigravity_command_fields, should_finalize_worktree_handoff_turn,
};
pub use mcp_tools::{
    LEGION_MCP_SERVER_NAMES, is_antigravity_legion_mcp_request, is_known_legion_mcp_tool,
};
pub use runtime::{
    ANTIGRAVITY_DEFAULT_RUNTIME_MODES, ANTIGRAVITY_FORCE_FILE_STORAGE_ENV,
    AntigravityModelCapability, AntigravityPermissionDisposition, AntigravitySuffixedModel,
    AntigravityThinkingDescriptor, RememberedAntigravityRuntimeModes,
    antigravity_permission_disposition, antigravity_permission_disposition_for_tool,
    antigravity_permission_mode, antigravity_runtime_mode_for_session_mode,
    antigravity_session_mode, build_antigravity_launch_spec,
    build_antigravity_launch_spec_for_auth, parse_remembered_antigravity_runtime_modes,
    resolve_antigravity_suffixed_model, resolve_initial_antigravity_runtime_modes,
    select_antigravity_text_gen_thinking, should_persist_remembered_runtime_modes,
};
pub use skills::{
    ANTIGRAVITY_USER_RULE_FILES, AntigravityDiscoveredSkill, AntigravitySkillsConfig,
    SkillConfigEntry, discover_antigravity_skills, resolve_skills_config_path,
    sync_antigravity_user_rule_copies,
};
pub use usage::{
    AcpTurnTokenUsage, ProviderCatalogModels, RawAcpUsageCounts, acp_per_turn_prompt_usage,
    antigravity_model_id_lookup, antigravity_rate_model, antigravity_usage_profile_dirs,
    catalog_model_label, is_antigravity_conversation_db_tables,
};
