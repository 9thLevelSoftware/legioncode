//! Antigravity managed binary version reference-counted lease tracking.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

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
