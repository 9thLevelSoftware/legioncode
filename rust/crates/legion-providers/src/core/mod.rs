//! Shared provider primitives independent of any concrete CLI or HTTP transport.
//!
//! Exports deterministic ID allocation ([`IdAllocator`]), context handoff
//! budgeting and truncation ([`HandoffBudget`], [`truncate_head_tail_utf16`]),
//! attachment prompt formatting ([`AttachmentPrompt`]), normalized model catalogs
//! ([`ModelCatalog`], [`model_short_name`]), structured text-generation prompts
//! ([`TextGenerationPrompts`]), failure classification/redaction
//! ([`classify_provider_failure`]), and usage-limit reconciliation
//! ([`resolve_usage_limits_after_probe`]).

pub mod allocator;
pub mod attachments;
pub mod budget;
pub mod catalog;
pub mod compatibility;
mod compatibility_semver;
pub mod failures;
pub mod prompts;
pub mod server_provider;
mod server_provider_models;
pub mod usage_limits;
pub mod usage_routing;

pub use allocator::{IdAllocator, encode_id_component, is_delegated_task_thread, join_id};
pub use attachments::{
    AttachmentPrompt, FormattedAttachmentPrompt, PROVIDER_SEND_TURN_MAX_INPUT_CHARS,
    PROVIDER_SEND_TURN_SUPPORTED_IMAGE_MIME_TYPES, PromptAttachment, TextAttachmentDecodeError,
    compact_accessibility_tree, decode_text_attachment_bytes,
};
pub use budget::{
    DEFAULT_HANDOFF_TOKEN_CAP, DEFAULT_TRUNCATION_MARKER, FILE_ATTACHMENT_TOKEN_ALLOWANCE,
    HANDOFF_BYTE_CAP, HandoffAttachmentKind, HandoffBudget, HistoryEntry, HistoryRole,
    IMAGE_ATTACHMENT_TOKEN_ALLOWANCE, MAX_HANDOFF_TOKEN_CAP, MAX_OMITTED_IDS_LISTED,
    MIN_HANDOFF_TOKEN_CAP, SelectedHistory, clamp_handoff_token_cap, estimate_text_tokens,
    select_history, truncate_head_tail_utf16,
};
pub use catalog::{
    CatalogModelEntry, ClassifiedCatalogModels, ModelCatalog, ServerModelEntry, model_short_name,
};
pub use compatibility::{
    CompatibilityManifestError, CompatibilityRangeRule, CompatibilityStatus,
    ProviderCompatibilityAdvisory, ProviderCompatibilityPolicy, Semver, broken_version_detail,
    default_compatibility_policies, minimum_supported_version_from_manifest,
    parse_compatibility_policies, resolve_provider_compatibility, satisfies_semver_range,
};
pub use failures::{
    ClassifiedProviderFailure, MAX_PROVIDER_FAILURE_CODE_LENGTH,
    MAX_PROVIDER_FAILURE_MESSAGE_LENGTH, PROVIDER_RETRY_BUDGET_MS, ProviderFailureInput,
    classify_provider_failure, redact_sensitive_text, truncate_bounded,
};
pub use prompts::{
    BranchNamePromptInput, CommitMessagePromptInput, GeneratedTextPrompt, MAX_COMMIT_SUBJECT_CHARS,
    MAX_THREAD_TITLE_CHARS, PrContentPromptInput, TEXT_GEN_SECTION_MAX_CHARS,
    TITLE_MESSAGE_MAX_CHARS, TextGenerationPrompts, TitleContextMessage, TitleContextRole,
    limit_section, limit_title_message, sanitize_commit_subject, sanitize_pr_title,
    sanitize_thread_title,
};
pub use server_provider::{
    ModelCapabilities, ProviderDiscoveryOutcome, ProviderOptionChoice, ProviderOptionDescriptor,
    ServerProviderAuth, ServerProviderAuthStatus, ServerProviderModel,
    ServerProviderNativeSessions, ServerProviderSetup, ServerProviderSkill,
    ServerProviderSlashCommand, ServerProviderSnapshot, ServerProviderState,
    build_server_provider_snapshot, default_models_for_driver,
};
pub use usage_limits::{
    ProbeAvailability, UsageLimitsSnapshot, UsageWindow, UsageWindowKind,
    apply_usage_limits_update, clamp_percent, normalize_usage_windows,
    resolve_usage_limits_after_probe,
};
pub use usage_routing::{
    UsageLimitSourceSnapshot, UsageSourceAccount, antigravity_window_limits_model,
    hub_pool_reset_at, hub_routed_accounts, instance_hubs, parse_rfc3339_millis,
    transcript_line_contains_any_marker,
};
