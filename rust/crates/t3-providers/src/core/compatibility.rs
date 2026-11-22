//! Unified provider version compatibility floor and manifest policy resolution.
//!
//! Replaces per-driver hardcoded minimum version checks with one compatibility
//! lookup ([`resolve_provider_compatibility`]) and one refusal error format
//! ([`broken_version_detail`]), covering Antigravity (`< 1.5.0` refused),
//! `OpenCode`, Pi, Codex, Claude Code, Cursor, Grok, Kiro, Muse, and fx.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use regex::Regex;
use serde::{Deserialize, Serialize};
use t3_contracts::brand::ACTIVE_BRAND;

use super::compatibility_semver::{BUNDLED_COMPATIBILITY_JSON, parse_partial_version};

pub use super::compatibility_semver::{Semver, satisfies_semver_range};

/// Compatibility classification returned by manifest version policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompatibilityStatus {
    /// Fully tested and supported on this release.
    Supported,
    /// Works with degraded or legacy behavior.
    Graceful,
    /// Known unsupported version; user is warned before running.
    Unsupported,
    /// Known incompatible or dangerous version; execution must be refused.
    Broken,
    /// Version could not be matched or is unrecognized (for example, unlisted tags).
    Unknown,
}

/// One ordered semver range entry inside a [`ProviderCompatibilityPolicy`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityRangeRule {
    /// Semver comparator range expression (supports `||`, whitespace AND, `=`, `<`, `<=`, `>`, `>=`, `^`, `~`).
    pub range: String,
    /// Status assigned when the provider version matches [`Self::range`].
    pub status: CompatibilityStatus,
    /// Optional status-specific explanation shown alongside the advisory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Manifest compatibility policy scoped to one provider driver and host release range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCompatibilityPolicy {
    /// Canonical driver slug (`"codex"`, `"antigravity"`, `"opencode"`, `"pi"`, etc.).
    pub driver: String,
    /// Semver range of host T3 Code versions this policy applies to.
    pub t3_code_range: String,
    /// Optional regex whose first capture group extracts `X.Y.Z` from non-semver tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_pattern: Option<String>,
    /// Recommended version range communicated on downgrade/refusal or upgrade hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_range: Option<String>,
    /// Concrete recommended version when a single release is preferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_version: Option<String>,
    /// Ordered compatibility range rules evaluated top-to-bottom.
    pub ranges: Vec<CompatibilityRangeRule>,
}

/// Resolved provider compatibility advisory for a detected runtime version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCompatibilityAdvisory {
    /// Evaluated compatibility status.
    pub status: CompatibilityStatus,
    /// Optional rule message attached to the matched range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Recommended target version from the policy, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_version: Option<String>,
    /// Recommended target version range from the policy, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_range: Option<String>,
}

/// Errors raised while decoding manifest compatibility policies.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid compatibility manifest JSON: {0}")]
pub struct CompatibilityManifestError(String);

fn compile_cached_pattern(pattern: &str) -> Option<Regex> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Regex>>>> = OnceLock::new();
    let mutex = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = mutex.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(cached) = guard.get(pattern) {
        return cached.clone();
    }
    let compiled = Regex::new(pattern).ok();
    guard.insert(pattern.to_owned(), compiled.clone());
    compiled
}

/// Parses compatibility policies from either a full manifest object (`{"compatibility": [...]}`)
/// or a standalone JSON array.
pub fn parse_compatibility_policies(
    json: &str,
) -> Result<Vec<ProviderCompatibilityPolicy>, CompatibilityManifestError> {
    #[derive(Deserialize)]
    struct ManifestEnvelope {
        compatibility: Vec<ProviderCompatibilityPolicy>,
    }

    if json.trim_start().starts_with('[') {
        serde_json::from_str::<Vec<ProviderCompatibilityPolicy>>(json)
            .map_err(|err| CompatibilityManifestError(err.to_string()))
    } else {
        serde_json::from_str::<ManifestEnvelope>(json)
            .map(|envelope| envelope.compatibility)
            .map_err(|err| CompatibilityManifestError(err.to_string()))
    }
}

/// Returns the bundled default compatibility policies aligned with the Wave 3a manifest.
#[must_use]
pub fn default_compatibility_policies() -> Vec<ProviderCompatibilityPolicy> {
    parse_compatibility_policies(BUNDLED_COMPATIBILITY_JSON).unwrap_or_default()
}

/// Resolves compatibility status for `driver` and optional reported `version`.
///
/// Checks `policies` first for a policy matching `driver` and `t3_code_version`, falling back
/// to `fallback_policies` only when `policies` carries no matching policy entry.
#[must_use]
pub fn resolve_provider_compatibility(
    policies: &[ProviderCompatibilityPolicy],
    fallback_policies: Option<&[ProviderCompatibilityPolicy]>,
    driver: &str,
    version: Option<&str>,
    t3_code_version: &str,
) -> Option<ProviderCompatibilityAdvisory> {
    resolve_against_slice(policies, driver, version, t3_code_version).or_else(|| {
        fallback_policies.and_then(|fb| resolve_against_slice(fb, driver, version, t3_code_version))
    })
}

fn resolve_against_slice(
    policies: &[ProviderCompatibilityPolicy],
    driver: &str,
    version: Option<&str>,
    t3_code_version: &str,
) -> Option<ProviderCompatibilityAdvisory> {
    let policy = policies.iter().find(|p| {
        p.driver == driver && satisfies_semver_range(t3_code_version, &p.t3_code_range)
    })?;
    let unprefixed = version.map(|v| v.trim().trim_start_matches('v'));
    let compiled = policy
        .version_pattern
        .as_deref()
        .and_then(compile_cached_pattern);
    let extracted = unprefixed.map(|raw| {
        if let Some(re) = compiled.as_ref()
            && let Some(caps) = re.captures(raw)
            && let Some(group) = caps.get(1)
        {
            return group.as_str();
        }
        raw
    });
    let matched_rule = extracted
        .filter(|candidate| Semver::parse_strict(candidate).is_some())
        .and_then(|stable| {
            policy
                .ranges
                .iter()
                .find(|entry| satisfies_semver_range(stable, &entry.range))
        });

    Some(ProviderCompatibilityAdvisory {
        status: matched_rule.map_or(CompatibilityStatus::Unknown, |r| r.status),
        message: matched_rule.and_then(|r| r.message.clone()),
        recommended_version: policy.recommended_version.clone(),
        recommended_range: policy.recommended_range.clone(),
    })
}

/// Formats the single unified runtime refusal message for versions rated `broken`.
#[must_use]
pub fn broken_version_detail(
    provider_name: &str,
    version: &str,
    advisory: Option<&ProviderCompatibilityAdvisory>,
    action: Option<&str>,
) -> Option<String> {
    let advisory = advisory?;
    if advisory.status != CompatibilityStatus::Broken {
        return None;
    }
    let target = advisory
        .recommended_version
        .as_deref()
        .or(advisory.recommended_range.as_deref())
        .unwrap_or("a newer version");
    let update_action = action.map_or_else(|| format!("Update {provider_name}"), str::to_owned);
    Some(format!(
        "{provider_name} {version} does not work with this {} release. {update_action} to {target}, then retry.",
        ACTIVE_BRAND.product_name
    ))
}

/// Extracts the minimum non-broken version lower bound from the manifest policy for `driver`.
#[must_use]
pub fn minimum_supported_version_from_manifest(
    policies: &[ProviderCompatibilityPolicy],
    fallback_policies: Option<&[ProviderCompatibilityPolicy]>,
    driver: &str,
    t3_code_version: &str,
) -> Option<Semver> {
    let policy = policies
        .iter()
        .chain(fallback_policies.into_iter().flatten())
        .find(|p| {
            p.driver == driver && satisfies_semver_range(t3_code_version, &p.t3_code_range)
        })?;
    policy
        .ranges
        .iter()
        .filter(|rule| {
            matches!(
                rule.status,
                CompatibilityStatus::Supported | CompatibilityStatus::Graceful
            )
        })
        .flat_map(|rule| rule.range.split("||"))
        .filter_map(|clause| {
            clause.split_whitespace().find_map(|tok| {
                let raw = tok
                    .strip_prefix(">=")
                    .or_else(|| tok.strip_prefix('='))
                    .or_else(|| tok.strip_prefix('^'))
                    .or_else(|| tok.strip_prefix('~'))?;
                let (ma, mi, pa) = parse_partial_version(raw)?;
                Some(Semver {
                    major: ma,
                    minor: mi.unwrap_or(0),
                    patch: pa.unwrap_or(0),
                })
            })
        })
        .min()
}
