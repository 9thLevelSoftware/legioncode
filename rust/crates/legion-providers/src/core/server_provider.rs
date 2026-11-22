//! Contract-aligned `ServerProvider` and `ServerProviderModel` wire snapshot schemas.
//!
//! Mirrors `packages/contracts/src/server.ts` and `packages/contracts/src/model.ts`.

use serde::{Deserialize, Serialize};

use crate::core::catalog::ServerModelEntry;
use crate::core::server_provider_models::capabilities_for_reasoning;
pub use crate::core::server_provider_models::{default_models_for_driver, resolve_cli_model_arg};
use crate::drivers::{ProviderDriverSpec, ProviderProbeStatus};

/// Lifecycle readiness status matching `ServerProviderState` in `packages/contracts/src/server.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerProviderState {
    /// Installed, configured, and ready to execute turns.
    Ready,
    /// Installed, but needs user action (e.g. authentication or advisory warning).
    Warning,
    /// Installation or probe check failed.
    Error,
    /// Disabled or not installed on this machine.
    Disabled,
}

/// Authentication state matching `ServerProviderAuthStatus` in `packages/contracts/src/server.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerProviderAuthStatus {
    /// Active credentials confirmed.
    Authenticated,
    /// Credentials missing, expired, or rejected.
    Unauthenticated,
    /// Auth state could not be determined statically.
    Unknown,
}

/// Auth detail block matching `ServerProviderAuth` in `packages/contracts/src/server.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderAuth {
    /// High-level auth classification.
    pub status: ServerProviderAuthStatus,
    /// Auth mechanism label (e.g. `"oauth"`, `"apiKey"`, `"cli"`).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<String>,
    /// Human-readable credential/account label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Choice item within a [`ProviderOptionDescriptor::Select`] option control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptionChoice {
    /// Stable option value identifier.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional tooltip description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this choice is the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

/// Typed per-model option control matching `ProviderOptionDescriptor` in `model.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ProviderOptionDescriptor {
    /// Enumerated selector (for example reasoning effort).
    #[serde(rename_all = "camelCase")]
    Select {
        /// Stable option identifier.
        id: String,
        /// Display label.
        label: String,
        /// Optional helper text.
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Allowed choices.
        options: Vec<ProviderOptionChoice>,
        /// Optional selected choice ID.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_value: Option<String>,
        /// Optional role marker (`"reasoning"`).
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Boolean toggle option.
    #[serde(rename_all = "camelCase")]
    Boolean {
        /// Stable option identifier.
        id: String,
        /// Display label.
        label: String,
        /// Optional helper text.
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Optional active boolean state.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_value: Option<bool>,
    },
}

/// Model option capabilities matching `ModelCapabilities` in `packages/contracts/src/model.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    /// Option controls offered for this model in composer pickers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_descriptors: Option<Vec<ProviderOptionDescriptor>>,
}

/// Wire model schema matching `ServerProviderModel` in `packages/contracts/src/server.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderModel {
    /// Canonical model identifier.
    pub slug: String,
    /// Human-readable display name.
    pub name: String,
    /// Compact label used inside narrow pickers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    /// Optional model alias list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    /// Whether user-configured custom model.
    pub is_custom: bool,
    /// Whether default model for its driver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// Whether marked legacy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_legacy: Option<bool>,
    /// Explicit text-generation support override (`false` when session-only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_text_generation: Option<bool>,
    /// Structured model option capabilities.
    pub capabilities: Option<ModelCapabilities>,
}

impl From<ServerModelEntry> for ServerProviderModel {
    fn from(entry: ServerModelEntry) -> Self {
        Self {
            slug: entry.slug,
            name: entry.name,
            short_name: Some(entry.short_name),
            aliases: None,
            is_custom: entry.is_custom,
            is_default: None,
            is_legacy: None,
            supports_text_generation: None,
            capabilities: capabilities_for_reasoning(entry.supports_reasoning),
        }
    }
}

/// Advertised provider slash command matching `ServerProviderSlashCommand`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSlashCommand {
    /// Command name without leading slash.
    pub name: String,
    /// Optional short description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Discovered provider skill entry matching `ServerProviderSkill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSkill {
    /// Skill identifier.
    pub name: String,
    /// Optional summary text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// On-disk skill path.
    pub path: String,
    /// Discovery scope (for example `"user"` or `"project"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether currently enabled.
    pub enabled: bool,
}

/// Setup capability hints matching `ServerProvider.setup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSetup {
    /// Whether authentication flow is supported.
    pub can_authenticate: bool,
    /// Whether automated installation is supported.
    pub can_install: bool,
}

/// Native session capability hints matching `ServerProvider.nativeSessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderNativeSessions {
    /// Supports listing native sessions.
    pub can_list: bool,
    /// Supports loading native sessions.
    pub can_load: bool,
    /// Supports resuming native sessions.
    pub can_resume: bool,
}

/// Complete provider discovery record matching `ServerProvider` in `packages/contracts/src/server.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProviderSnapshot {
    /// Stable routing instance identifier (`"codex"`, `"claudeAgent"`, `"devReplay"`, ...).
    pub instance_id: String,
    /// Driver kind slug matching `ProviderDriverKind`.
    pub driver: String,
    /// UI display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Supported orchestration runtime modes.
    pub supported_runtime_modes: Vec<String>,
    /// Whether conversation rollback is supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_conversation_rollback: Option<bool>,
    /// Whether lightweight text generation is supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_text_generation: Option<bool>,
    /// Setup capability flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<ServerProviderSetup>,
    /// Native session management capability flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_sessions: Option<ServerProviderNativeSessions>,
    /// Whether enabled in configuration.
    pub enabled: bool,
    /// Whether binary/runtime installation is present.
    pub installed: bool,
    /// Effective CLI/server version string.
    pub version: Option<String>,
    /// Detected local CLI version when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// Overall provider readiness status.
    pub status: ServerProviderState,
    /// Authentication metadata block.
    pub auth: ServerProviderAuth,
    /// RFC-3339 probe timestamp.
    pub checked_at: String,
    /// Optional user-facing diagnostic detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Availability state (`"available"` | `"unavailable"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    /// Reason when `availability == Some("unavailable")`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Available provider models conforming to `ServerProviderModel`.
    pub models: Vec<ServerProviderModel>,
    /// Discovered slash commands.
    pub slash_commands: Vec<ServerProviderSlashCommand>,
    /// Discovered skills.
    pub skills: Vec<ServerProviderSkill>,
}

/// Inputs supplied to [`build_server_provider_snapshot`] from a live probe pass.
#[derive(Debug, Clone)]
pub struct ProviderDiscoveryOutcome {
    /// Instance identifier (`spec.slug` by default).
    pub instance_id: String,
    /// Probe classification status.
    pub probe_status: ProviderProbeStatus,
    /// Detected CLI version string if available.
    pub version: Option<String>,
    /// RFC-3339 check timestamp.
    pub checked_at: String,
    /// Optional diagnostic message.
    pub message: Option<String>,
}

/// Builds a contract-aligned [`ServerProviderSnapshot`] from a static driver spec and live probe outcome.
#[must_use]
pub fn build_server_provider_snapshot(
    spec: &ProviderDriverSpec,
    outcome: ProviderDiscoveryOutcome,
) -> ServerProviderSnapshot {
    let installed = !matches!(outcome.probe_status, ProviderProbeStatus::NotInstalled);
    let (status, auth_status) = match outcome.probe_status {
        ProviderProbeStatus::Ready => (
            ServerProviderState::Ready,
            ServerProviderAuthStatus::Authenticated,
        ),
        ProviderProbeStatus::NeedsAuth => (
            ServerProviderState::Warning,
            ServerProviderAuthStatus::Unauthenticated,
        ),
        ProviderProbeStatus::Error => (
            ServerProviderState::Error,
            ServerProviderAuthStatus::Unknown,
        ),
        ProviderProbeStatus::NotInstalled => (
            ServerProviderState::Disabled,
            ServerProviderAuthStatus::Unauthenticated,
        ),
    };
    let availability = if installed {
        "available"
    } else {
        "unavailable"
    };
    let unavailable_reason = (!installed).then(|| {
        outcome
            .message
            .clone()
            .unwrap_or_else(|| format!("{} CLI not found on PATH", spec.display_name))
    });

    ServerProviderSnapshot {
        instance_id: outcome.instance_id,
        driver: spec.slug.to_owned(),
        display_name: Some(spec.display_name.to_owned()),
        supported_runtime_modes: vec!["approval-required".to_owned(), "full-access".to_owned()],
        supports_conversation_rollback: Some(!matches!(spec.slug, "devReplay")),
        supports_text_generation: Some(spec.supports_text_generation),
        setup: Some(ServerProviderSetup {
            can_authenticate: !matches!(spec.slug, "devReplay"),
            can_install: false,
        }),
        native_sessions: Some(ServerProviderNativeSessions {
            can_list: matches!(spec.slug, "codex" | "claudeAgent" | "opencode"),
            can_load: matches!(spec.slug, "codex" | "claudeAgent" | "opencode"),
            can_resume: matches!(spec.slug, "codex" | "claudeAgent" | "opencode" | "cursor"),
        }),
        enabled: installed,
        installed,
        version: outcome.version.clone(),
        installed_version: outcome.version,
        status,
        auth: ServerProviderAuth {
            status: auth_status,
            auth_type: (spec.slug == "devReplay").then(|| "builtin".to_owned()),
            label: (spec.slug == "devReplay").then(|| "Built-in Dev Replay".to_owned()),
        },
        checked_at: outcome.checked_at,
        message: outcome.message,
        availability: Some(availability.to_owned()),
        unavailable_reason,
        models: default_models_for_driver(spec.slug),
        slash_commands: Vec::new(),
        skills: Vec::new(),
    }
}
