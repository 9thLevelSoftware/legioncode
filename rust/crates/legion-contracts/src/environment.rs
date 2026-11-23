//! Execution environment descriptors, host platform metadata, and server capabilities.
//!
//! Mirrors `packages/contracts/src/environment.ts` and the capability builder in
//! `apps/server/src/environment/ServerEnvironment.ts:211-305`.
//!
//! # Examples
//!
//! ```
//! use legion_contracts::environment::{
//!     ExecutionEnvironmentDescriptor, ORCHESTRATION_PROTOCOL_VERSION,
//!     PROVIDER_SEND_TURN_MAX_FILE_BYTES, PlatformArch, PlatformInfo, PlatformOs,
//! };
//! use legion_contracts::ids::EnvironmentId;
//!
//! let env_id = EnvironmentId::new("env-1")?;
//! let platform = PlatformInfo::new(PlatformOs::Darwin, PlatformArch::Arm64);
//! let desc = ExecutionEnvironmentDescriptor::builder(env_id, "Local", platform, "0.1.10")
//!     .voice_transcription(true)
//!     .build();
//!
//! assert_eq!(desc.orchestration_protocol_version, Some(ORCHESTRATION_PROTOCOL_VERSION));
//! assert_eq!(
//!     desc.capabilities.file_attachments.as_ref().map(|f| f.max_upload_bytes.get()),
//!     Some(PROVIDER_SEND_TURN_MAX_FILE_BYTES),
//! );
//! assert_eq!(desc.capabilities.fast_worktree_checkout, Some(true));
//! assert_eq!(desc.capabilities.local_server_stop, Some(true));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod builder;
mod capabilities;

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::ids::EnvironmentId;

pub use builder::ExecutionEnvironmentBuilder;
pub use capabilities::{
    EnvironmentCapabilities, FileAttachmentsCapability, ServerInstallation,
    ServerSelfUpdateCapability,
};

/// Wire orchestration protocol version supported by this crate.
pub const ORCHESTRATION_PROTOCOL_VERSION: u32 = 2;

/// String form of [`ORCHESTRATION_PROTOCOL_VERSION`] used in query parameters and headers.
pub const ORCHESTRATION_PROTOCOL_VERSION_TEXT: &str = "2";

/// WebSocket query parameter carrying the orchestration protocol version.
pub const ORCHESTRATION_PROTOCOL_QUERY_PARAM: &str = "orchestrationProtocol";

/// HTTP header carrying the orchestration protocol version.
pub const ORCHESTRATION_PROTOCOL_HEADER: &str = "x-legioncode-orchestration-protocol";

/// WebSocket query parameter carrying comma-separated client capabilities.
pub const CLIENT_CAPABILITIES_QUERY_PARAM: &str = "clientCapabilities";

/// Client capability indicating support for secret provider references in settings.
pub const SETTINGS_SECRET_REFERENCES_CAPABILITY: &str = "settingsSecretReferences";

/// Maximum file attachment upload size in bytes (`50 MiB`), matching `chatAttachment.ts`.
pub const PROVIDER_SEND_TURN_MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

pub(crate) fn forward_compatible_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    if raw.is_null() {
        return Ok(None);
    }
    Ok(serde_json::from_value::<T>(raw).ok())
}

pub(crate) fn deserialize_trimmed_non_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(de::Error::custom(
            "string cannot be empty or whitespace-only",
        ));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn deserialize_non_empty_raw<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() {
        return Err(de::Error::custom("string cannot be empty"));
    }
    Ok(raw)
}

/// Host operating system reported by an execution environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlatformOs {
    /// macOS (`darwin`).
    Darwin,
    /// Linux.
    Linux,
    /// Windows (`win32`).
    Windows,
    /// Any other operating system.
    Unknown,
}

/// Host CPU architecture reported by an execution environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlatformArch {
    /// 64-bit ARM (`arm64`).
    Arm64,
    /// 64-bit x86 (`x64`).
    X64,
    /// Any other architecture.
    Other,
}

/// Machine form factor icon/classification for an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MachineKind {
    /// Rack or headless server.
    Server,
    /// Cloud VM or container.
    Cloud,
    /// Linux workstation.
    Linux,
    /// Desktop computer.
    Desktop,
    /// Laptop computer.
    Laptop,
    /// Apple Mac mini.
    MacMini,
    /// Apple Mac Studio.
    MacStudio,
}

/// Host operating system, CPU architecture, and optional machine form factor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    /// Host operating system.
    pub os: PlatformOs,
    /// Host CPU architecture.
    pub arch: PlatformArch,
    /// Optional machine kind; unrecognized future variants decode as `None`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "forward_compatible_optional"
    )]
    pub machine: Option<MachineKind>,
}

impl PlatformInfo {
    /// Creates a [`PlatformInfo`] without a machine classification.
    pub fn new(os: PlatformOs, arch: PlatformArch) -> Self {
        Self {
            os,
            arch,
            machine: None,
        }
    }

    /// Sets the optional machine classification.
    #[must_use]
    pub fn with_machine(mut self, machine: MachineKind) -> Self {
        self.machine = Some(machine);
        self
    }
}

/// Wire descriptor identifying an execution environment and its advertised capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEnvironmentDescriptor {
    /// Persistent environment identifier.
    pub environment_id: EnvironmentId,
    /// Human-readable trimmed label for this environment.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub label: String,
    /// Host OS, architecture, and optional machine form factor.
    pub platform: PlatformInfo,
    /// Semantic version of the running `LegionCode` server.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub server_version: String,
    /// Protocol version (`2`), omitted only by very old servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestration_protocol_version: Option<u32>,
    /// Highest SQLite schema migration applied by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_migration: Option<u32>,
    /// Feature and capability flags advertised by this environment.
    pub capabilities: EnvironmentCapabilities,
}

impl ExecutionEnvironmentDescriptor {
    /// Starts building a descriptor matching `ServerEnvironment.ts:211-305`.
    pub fn builder(
        environment_id: EnvironmentId,
        label: impl Into<String>,
        platform: PlatformInfo,
        server_version: impl Into<String>,
    ) -> ExecutionEnvironmentBuilder {
        ExecutionEnvironmentBuilder::new(environment_id, label, platform, server_version)
    }
}
