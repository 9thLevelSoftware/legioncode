//! Antigravity installation detection, isolated home derivation, and auth probing.

use sha2::{Digest, Sha256};

use crate::core::{ProbeAvailability, resolve_usage_limits_after_probe};

use super::super::{ProviderProbeStatus, ProviderSnapshotState};

/// Environment flag preventing managed Antigravity CLI binaries from self-updating in place.
pub const AGY_CLI_DISABLE_AUTO_UPDATE_ENV: &str = "AGY_CLI_DISABLE_AUTO_UPDATE";

/// Ambient Gemini/Google variables stripped before injecting the isolated `GEMINI_HOME`.
pub const ANTIGRAVITY_REMOVED_ENV_KEYS: &[&str] = &[
    "GEMINI_HOME",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_GENAI_USE_VERTEXAI",
    "GOOGLE_APPLICATION_CREDENTIALS",
    AGY_CLI_DISABLE_AUTO_UPDATE_ENV,
];

/// Supported Antigravity authentication methods.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AntigravityAuthMethod {
    /// Consumer Google sign-in isolated per T3 instance (`oauth-personal`).
    #[default]
    OauthPersonal,
    /// Enterprise/workspace Google OAuth (`oauth-business`).
    OauthBusiness,
    /// Gemini API key (`api-key`).
    ApiKey,
    /// Vertex AI credentials (`vertex-ai`).
    VertexAi,
    /// Custom/provider-managed credentials (`custom`).
    Custom,
}

impl AntigravityAuthMethod {
    /// Returns the wire identifier used by Antigravity settings and telemetry.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OauthPersonal => "oauth-personal",
            Self::OauthBusiness => "oauth-business",
            Self::ApiKey => "api-key",
            Self::VertexAi => "vertex-ai",
            Self::Custom => "custom",
        }
    }

    /// Returns `true` when credentials must stay isolated per instance instead of
    /// reading/writing the shared OS keyring.
    #[must_use]
    pub const fn isolates_personal_oauth(self) -> bool {
        matches!(self, Self::OauthPersonal)
    }
}

/// Detected Antigravity runtime executable target on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityInstallTarget {
    /// Unified `agy` CLI (`agy acp`).
    AgyCli {
        /// Resolved path to the `agy` binary.
        executable: String,
    },
    /// Split `agy_acp_server` paired with `localharness_external`.
    AcpServer {
        /// Resolved path to `agy_acp_server`.
        executable: String,
        /// Companion `localharness_external` binary path.
        localharness_external: String,
    },
}

impl AntigravityInstallTarget {
    /// Returns the primary executable path launched by the harness.
    #[must_use]
    pub fn executable(&self) -> &str {
        match self {
            Self::AgyCli { executable } | Self::AcpServer { executable, .. } => executable,
        }
    }
}

/// Auth configuration inspected *only after* installation presence is confirmed (`BUG-11`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AntigravityAuthConfig {
    /// Whether an explicit Gemini/Vertex API key is configured.
    pub has_api_key: bool,
    /// Whether an OAuth credential file or token is present.
    pub has_oauth_token: bool,
    /// Optional explicit auth configuration validation error.
    pub validation_error: Option<String>,
}

/// Isolated filesystem roots derived deterministically via SHA-256 for an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityInstanceDirectories {
    /// 64-char SHA-256 hex digest of the logical instance key.
    pub instance_digest_hex: String,
    /// Isolated `<data_root>/providers/antigravity/<sha256>` directory assigned to `GEMINI_HOME`.
    pub gemini_home: String,
    /// Isolated `<temp_root>/antigravity-tmp/<sha256_12>` runtime temp directory.
    pub runtime_temp: String,
}

/// Result of [`probe_antigravity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityProbeOutcome {
    /// Neither `agy` nor `agy_acp_server` is installed (takes precedence over auth errors).
    NotInstalled {
        /// Explanation suitable for UI display.
        reason: String,
    },
    /// Binary exists on disk, but credentials are missing or invalid.
    NeedsAuth {
        /// Authentication setup hint.
        reason: String,
        /// Discovered binary target.
        target: AntigravityInstallTarget,
    },
    /// Binary and authentication are both valid.
    Ready {
        /// Discovered binary target.
        target: AntigravityInstallTarget,
        /// Isolated `GEMINI_HOME` / runtime temp directories.
        directories: AntigravityInstanceDirectories,
    },
}

/// Derives deterministic per-instance Antigravity `GEMINI_HOME` and temp paths using SHA-256.
///
/// # Examples
///
/// ```
/// use t3_providers::drivers::resolve_antigravity_instance_directories;
///
/// let dirs = resolve_antigravity_instance_directories("/var/t3", "/tmp", "antigravity:work");
/// assert_eq!(dirs.instance_digest_hex.len(), 64);
/// assert!(dirs.gemini_home.ends_with(&format!("providers/antigravity/{}", dirs.instance_digest_hex)));
/// assert!(dirs.runtime_temp.ends_with(&format!("antigravity-tmp/{}", &dirs.instance_digest_hex[..12])));
/// ```
#[must_use]
pub fn resolve_antigravity_instance_directories(
    data_root: &str,
    temp_root: &str,
    instance_key: &str,
) -> AntigravityInstanceDirectories {
    let digest_hex = hex::encode(Sha256::digest(instance_key.as_bytes()));
    let short_hex = digest_hex.get(..12).unwrap_or(&digest_hex);
    let data = data_root.trim_end_matches(['/', '\\']);
    let temp = temp_root.trim_end_matches(['/', '\\']);
    AntigravityInstanceDirectories {
        gemini_home: format!("{data}/providers/antigravity/{digest_hex}"),
        runtime_temp: format!("{temp}/antigravity-tmp/{short_hex}"),
        instance_digest_hex: digest_hex,
    }
}

/// Evaluates Antigravity installation presence **before** validating auth configuration.
///
/// Fixes `BUG-11`: when the binary is absent, missing or broken auth config never
/// masks [`AntigravityProbeOutcome::NotInstalled`] as `NeedsAuth`.
///
/// # Examples
///
/// ```
/// use t3_providers::drivers::{AntigravityAuthConfig, AntigravityProbeOutcome, probe_antigravity};
///
/// let broken_auth = AntigravityAuthConfig {
///     has_api_key: false,
///     has_oauth_token: false,
///     validation_error: Some("Missing GEMINI_API_KEY".to_owned()),
/// };
/// let outcome = probe_antigravity(None, &broken_auth, "antigravity", "/data", "/tmp");
/// assert!(matches!(outcome, AntigravityProbeOutcome::NotInstalled { .. }));
/// ```
#[must_use]
pub fn probe_antigravity(
    installed_target: Option<AntigravityInstallTarget>,
    auth: &AntigravityAuthConfig,
    instance_key: &str,
    data_root: &str,
    temp_root: &str,
) -> AntigravityProbeOutcome {
    // BUG-11 fix: verify installation before inspecting auth state so missing
    // credentials never report an uninstalled driver as `NeedsAuth`.
    let Some(target) = installed_target else {
        return AntigravityProbeOutcome::NotInstalled {
            reason: "Antigravity CLI binary not found on disk".to_owned(),
        };
    };

    if let Some(err) = auth
        .validation_error
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return AntigravityProbeOutcome::NeedsAuth {
            reason: err.to_owned(),
            target,
        };
    }

    if !auth.has_api_key && !auth.has_oauth_token {
        return AntigravityProbeOutcome::NeedsAuth {
            reason: "Antigravity authentication is not configured".to_owned(),
            target,
        };
    }

    let directories = resolve_antigravity_instance_directories(data_root, temp_root, instance_key);
    AntigravityProbeOutcome::Ready {
        target,
        directories,
    }
}

/// Reconciles cached [`ProviderSnapshotState`] with an [`AntigravityProbeOutcome`].
///
/// Fixes `BUG-11`: transitions to `NotInstalled` clear stale cached `models`,
/// `slash_commands`, `skills`, `usage_limits`, and `supports_text_generation`
/// rather than leaving prior data visible in UI selectors.
#[must_use]
pub fn reconcile_antigravity_snapshot(
    mut previous: ProviderSnapshotState,
    outcome: &AntigravityProbeOutcome,
) -> ProviderSnapshotState {
    match outcome {
        AntigravityProbeOutcome::NotInstalled { reason } => ProviderSnapshotState {
            installed: false,
            status: ProviderProbeStatus::NotInstalled,
            auth_configured: false,
            models: Vec::new(),
            slash_commands: Vec::new(),
            skills: Vec::new(),
            usage_limits: resolve_usage_limits_after_probe(
                previous.usage_limits.take(),
                None,
                ProbeAvailability::NotInstalled,
            ),
            supports_text_generation: false,
            message: Some(reason.clone()),
        },
        AntigravityProbeOutcome::NeedsAuth { reason, .. } => {
            previous.installed = true;
            previous.status = ProviderProbeStatus::NeedsAuth;
            previous.auth_configured = false;
            previous.usage_limits = resolve_usage_limits_after_probe(
                previous.usage_limits.take(),
                None,
                ProbeAvailability::NeedsAuth,
            );
            previous.message = Some(reason.clone());
            previous
        }
        AntigravityProbeOutcome::Ready { .. } => {
            previous.installed = true;
            previous.status = ProviderProbeStatus::Ready;
            previous.auth_configured = true;
            previous.message = None;
            previous
        }
    }
}
