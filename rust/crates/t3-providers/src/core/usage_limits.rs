//! Provider usage-limit window normalization, sorting, and probe reconciliation.
//!
//! Mirrors `packages/provider-core/src/server/usageLimits.ts` so rate-limit and
//! quota bars across providers sort deterministically (`session`, `weekly`,
//! `monthly`, `other`) and stale cached limits clear when probes transition to
//! `NotInstalled` or `NeedsAuth`.

use std::cmp::Ordering;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Granularity bucket for a provider rate-limit or usage window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageWindowKind {
    /// Short rolling session quota (for example 5-hour Codex/Claude window).
    Session,
    /// Rolling 7-day quota window.
    Weekly,
    /// Monthly billing/quota window.
    Monthly,
    /// Any provider-specific window outside standard buckets.
    Other,
}

impl UsageWindowKind {
    const fn sort_rank(self) -> u8 {
        match self {
            Self::Session => 0,
            Self::Weekly => 1,
            Self::Monthly => 2,
            Self::Other => 3,
        }
    }
}

/// Single quota or rate-limit window reported by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    /// Stable provider window identifier.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Window period classification.
    pub kind: UsageWindowKind,
    /// Normalized utilization percentage clamped into `0.0..=100.0`.
    pub used_percent: f64,
    /// Optional ISO-8601 reset timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// Ordered usage-limit snapshot exposed on a server provider record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitsSnapshot {
    /// Normalized and deterministically sorted usage windows.
    #[serde(default)]
    pub windows: Vec<UsageWindow>,
    /// ISO-8601 timestamp when limits were last refreshed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Probe readiness hint passed to [`resolve_usage_limits_after_probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAvailability {
    /// Provider CLI/service is installed and authenticated.
    Ready,
    /// Provider is installed but requires user authentication (`NeedsAuth`).
    NeedsAuth,
    /// Provider executable is absent (`NotInstalled`).
    NotInstalled,
    /// Transient probe failure unrelated to installation or auth loss.
    TransientError,
}

/// Clamps an arbitrary floating-point utilization percentage into `[0.0, 100.0]`,
/// treating `NaN` or negative infinity as `0.0` and `+inf` as `100.0`.
///
/// # Examples
///
/// ```
/// use t3_providers::core::clamp_percent;
///
/// assert_eq!(clamp_percent(-12.5), 0.0);
/// assert_eq!(clamp_percent(42.5), 42.5);
/// assert_eq!(clamp_percent(180.0), 100.0);
/// assert_eq!(clamp_percent(f64::NAN), 0.0);
/// ```
#[must_use]
pub fn clamp_percent(raw: f64) -> f64 {
    if raw.is_nan() {
        return 0.0;
    }
    raw.clamp(0.0, 100.0)
}

/// Sorts windows by `kind` priority (`Session`, `Weekly`, `Monthly`, `Other`),
/// breaking ties lexicographically by `id`, and clamps every `used_percent`.
#[must_use]
pub fn normalize_usage_windows(windows: Vec<UsageWindow>) -> Vec<UsageWindow> {
    let mut normalized: Vec<UsageWindow> = windows
        .into_iter()
        .map(|mut win| {
            win.used_percent = clamp_percent(win.used_percent);
            win
        })
        .collect();
    normalized.sort_by(compare_windows);
    normalized
}

fn compare_windows(a: &UsageWindow, b: &UsageWindow) -> Ordering {
    a.kind
        .sort_rank()
        .cmp(&b.kind.sort_rank())
        .then_with(|| a.id.cmp(&b.id))
}

/// Merges incremental usage window updates into an optional existing snapshot.
///
/// Later windows with matching `id` replace earlier ones while preserving all
/// unaffected windows and sorting the result deterministically.
#[must_use]
pub fn apply_usage_limits_update(
    existing: Option<&UsageLimitsSnapshot>,
    incoming: Vec<UsageWindow>,
    updated_at: Option<String>,
) -> UsageLimitsSnapshot {
    let mut by_id: IndexMap<String, UsageWindow> = IndexMap::new();
    if let Some(prev) = existing {
        for win in &prev.windows {
            by_id.insert(win.id.clone(), win.clone());
        }
    }
    for mut win in incoming {
        win.used_percent = clamp_percent(win.used_percent);
        by_id.insert(win.id.clone(), win);
    }

    let windows = normalize_usage_windows(by_id.into_values().collect());
    let resolved_time = updated_at.or_else(|| existing.and_then(|s| s.updated_at.clone()));
    UsageLimitsSnapshot {
        windows,
        updated_at: resolved_time,
    }
}

/// Reconciles cached usage limits against a fresh provider probe outcome.
///
/// Clears stale cached usage limits whenever a probe determines the provider
/// is [`ProbeAvailability::NotInstalled`] or [`ProbeAvailability::NeedsAuth`].
/// Preserves existing limits only when `fresh` is `None` during a
/// [`ProbeAvailability::TransientError`] or a non-usage `Ready` refresh.
#[must_use]
pub fn resolve_usage_limits_after_probe(
    previous: Option<UsageLimitsSnapshot>,
    fresh: Option<UsageLimitsSnapshot>,
    availability: ProbeAvailability,
) -> Option<UsageLimitsSnapshot> {
    match availability {
        ProbeAvailability::NotInstalled | ProbeAvailability::NeedsAuth => None,
        ProbeAvailability::Ready | ProbeAvailability::TransientError => {
            fresh.or(previous).map(|mut snap| {
                snap.windows = normalize_usage_windows(snap.windows);
                snap
            })
        }
    }
}
