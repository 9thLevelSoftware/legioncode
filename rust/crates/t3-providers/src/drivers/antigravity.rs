//! Antigravity ACP driver: install-first probing (`BUG-11`), SHA-256
//! `GEMINI_HOME` derivation, and version lease refcounting.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};
use t3_protocol::provider_env::SanitizedEnv;

use crate::core::{ProbeAvailability, resolve_usage_limits_after_probe};

use super::{ProviderLaunchSpec, ProviderProbeStatus, ProviderSnapshotState, build_child_env};

/// Environment flag forcing Antigravity ACP file-backed session storage.
pub const ANTIGRAVITY_FORCE_FILE_STORAGE_ENV: &str = "AGY_ACP_FORCE_FILE_STORAGE";

/// Ambient Gemini/Google variables stripped before injecting the isolated `GEMINI_HOME`.
pub const ANTIGRAVITY_REMOVED_ENV_KEYS: &[&str] = &[
    "GEMINI_HOME",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_GENAI_USE_VERTEXAI",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

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

/// Builds a sanitized Antigravity ACP subprocess launch specification (`BUG-12`).
#[must_use]
pub fn build_antigravity_launch_spec(
    env: &SanitizedEnv,
    target: &AntigravityInstallTarget,
    directories: &AntigravityInstanceDirectories,
    cwd: Option<&str>,
) -> ProviderLaunchSpec {
    let mut overrides = BTreeMap::new();
    overrides.insert("GEMINI_HOME".to_owned(), directories.gemini_home.clone());
    overrides.insert("TMPDIR".to_owned(), directories.runtime_temp.clone());
    overrides.insert(
        ANTIGRAVITY_FORCE_FILE_STORAGE_ENV.to_owned(),
        "1".to_owned(),
    );
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

/// Thread-safe reference-counted lease registry protecting active Antigravity
/// binary versions from background pruning while sessions are still running.
#[derive(Debug, Clone, Default)]
pub struct AntigravityVersionLeases {
    counts: Arc<Mutex<BTreeMap<String, usize>>>,
}

impl AntigravityVersionLeases {
    /// Creates an empty version lease registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires a lease on `version` and returns an RAII guard that releases it on drop.
    #[must_use]
    pub fn acquire(&self, version: &str) -> AntigravityVersionLeaseGuard {
        let key = version.trim().to_owned();
        let mut guard = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = guard.entry(key.clone()).or_insert(0);
        *entry = entry.saturating_add(1);
        AntigravityVersionLeaseGuard {
            counts: Arc::clone(&self.counts),
            version: key,
            released: false,
        }
    }

    /// Returns the active reference count for `version`.
    #[must_use]
    pub fn lease_count(&self, version: &str) -> usize {
        let guard = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        guard.get(version.trim()).copied().unwrap_or(0)
    }

    /// Returns `true` when at least one active session holds a lease on `version`.
    #[must_use]
    pub fn is_leased(&self, version: &str) -> bool {
        self.lease_count(version) > 0
    }

    /// Filters `installed_versions` to stale versions safe to prune (neither `active`
    /// nor currently leased).
    #[must_use]
    pub fn prunable_versions(
        &self,
        installed_versions: &[String],
        active_version: Option<&str>,
    ) -> Vec<String> {
        let guard = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        installed_versions
            .iter()
            .filter(|v| {
                let ver = v.trim();
                active_version != Some(ver) && guard.get(ver).copied().unwrap_or(0) == 0
            })
            .cloned()
            .collect()
    }
}

/// RAII lease guard decremented automatically when dropped or released early.
#[derive(Debug)]
pub struct AntigravityVersionLeaseGuard {
    counts: Arc<Mutex<BTreeMap<String, usize>>>,
    version: String,
    released: bool,
}

impl AntigravityVersionLeaseGuard {
    /// Explicitly releases this version lease immediately.
    pub fn release(mut self) {
        self.decrement();
    }

    fn decrement(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut guard = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = guard.get_mut(&self.version) {
            if *count <= 1 {
                guard.remove(&self.version);
            } else {
                *count -= 1;
            }
        }
    }
}

impl Drop for AntigravityVersionLeaseGuard {
    fn drop(&mut self) {
        self.decrement();
    }
}
