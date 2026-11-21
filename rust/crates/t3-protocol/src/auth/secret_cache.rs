//! TTL secret resolution cache that never memoizes failed or empty reads.
//!
//! Fixes BUG-15 in `packages/secret-provider-core/src/server/SecretResolver.ts`
//! and `apps/server/src/secrets/settingsSecretValue.ts`:
//! - Cached entries expire deterministically after `ttl_ms`.
//! - `Err` results, `Ok(None)` unresolved reads, and whitespace-only values are
//!   never cached, so a subsequent lookup retries the provider cleanly.
//! - Config changes invalidate stale entries for the same `(provider, reference)` pair.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, PoisonError, RwLock};

use t3_shared::concurrency::KeyedLock;

/// Default secret cache TTL in milliseconds (`5` minutes).
pub const DEFAULT_SECRET_CACHE_TTL_MS: i64 = 5 * 60 * 1000;

/// Cache key identifying a secret reference within a provider namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretCacheKey {
    /// Secret provider identifier (for example `"1password"` or `"keychain"`).
    pub provider: String,
    /// Provider-specific secret reference path or key.
    pub reference: String,
}

impl SecretCacheKey {
    /// Builds a [`SecretCacheKey`] from provider and reference identifiers.
    pub fn new(provider: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            reference: reference.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedSecretEntry {
    secret: String,
    config_key: String,
    expires_at_ms: i64,
}

/// Returns `true` when `value` contains a non-blank readable secret string.
pub fn is_readable_secret(value: Option<&str>) -> bool {
    value.is_some_and(|raw| !raw.trim().is_empty())
}

/// Concurrent per-key TTL cache for resolved secrets.
///
/// Errors, unresolved (`None`), and blank secret values are never stored.
#[derive(Clone)]
pub struct TtlSecretCache {
    ttl_ms: i64,
    entries: Arc<RwLock<HashMap<SecretCacheKey, CachedSecretEntry>>>,
    locks: KeyedLock<SecretCacheKey>,
}

impl fmt::Debug for TtlSecretCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtlSecretCache")
            .field("ttl_ms", &self.ttl_ms)
            .field("cached_count", &self.len())
            .finish_non_exhaustive()
    }
}

impl Default for TtlSecretCache {
    fn default() -> Self {
        Self::new(DEFAULT_SECRET_CACHE_TTL_MS)
    }
}

impl TtlSecretCache {
    /// Creates a [`TtlSecretCache`] whose resolved entries live for `ttl_ms` milliseconds.
    ///
    /// Non-positive `ttl_ms` values are clamped to `0`, disabling retention.
    pub fn new(ttl_ms: i64) -> Self {
        Self {
            ttl_ms: ttl_ms.max(0),
            entries: Arc::new(RwLock::new(HashMap::new())),
            locks: KeyedLock::new(),
        }
    }

    /// Returns the configured TTL in milliseconds.
    pub fn ttl_ms(&self) -> i64 {
        self.ttl_ms
    }

    /// Returns a cached secret when present, unexpired (`now_ms < expires_at_ms`),
    /// and matching `config_key`.
    pub fn get_if_valid(
        &self,
        provider: &str,
        reference: &str,
        config_key: &str,
        now_ms: i64,
    ) -> Option<String> {
        if self.ttl_ms <= 0 {
            return None;
        }
        let key = SecretCacheKey::new(provider, reference);
        let guard = self.entries.read().unwrap_or_else(PoisonError::into_inner);
        let entry = guard.get(&key)?;
        if entry.config_key != config_key || now_ms >= entry.expires_at_ms {
            return None;
        }
        if entry.secret.trim().is_empty() {
            return None;
        }
        Some(entry.secret.clone())
    }

    /// Stores `value` only if `ttl_ms > 0` and `value` is readable (non-empty after trim).
    ///
    /// Returns `true` if cached, `false` when skipped (`None` or blank). Never
    /// overwrites an existing cached value with a blank or `None` result.
    pub fn store_if_readable(
        &self,
        provider: &str,
        reference: &str,
        config_key: &str,
        value: Option<&str>,
        now_ms: i64,
    ) -> bool {
        if self.ttl_ms <= 0 {
            return false;
        }
        let Some(secret) = value.filter(|raw| !raw.trim().is_empty()) else {
            return false;
        };
        let key = SecretCacheKey::new(provider, reference);
        let expires_at_ms = now_ms.saturating_add(self.ttl_ms);
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key,
                CachedSecretEntry {
                    secret: secret.to_owned(),
                    config_key: config_key.to_owned(),
                    expires_at_ms,
                },
            );
        true
    }

    /// Resolves `(provider, reference)` via `fetcher` when absent, expired, config-stale, or `fresh`.
    ///
    /// Never caches `Err`, `Ok(None)`, or whitespace-only secret strings, and
    /// preserves any still-valid prior entry when a `fresh` refresh fails.
    pub async fn resolve_with<F, Fut, E>(
        &self,
        provider: &str,
        reference: &str,
        config_key: &str,
        now_ms: i64,
        fresh: bool,
        fetcher: F,
    ) -> Result<Option<String>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<String>, E>>,
    {
        if !fresh && let Some(cached) = self.get_if_valid(provider, reference, config_key, now_ms) {
            return Ok(Some(cached));
        }

        let key = SecretCacheKey::new(provider, reference);
        self.locks
            .with_lock(key, async {
                if !fresh
                    && let Some(cached) = self.get_if_valid(provider, reference, config_key, now_ms)
                {
                    return Ok(Some(cached));
                }

                match fetcher().await? {
                    Some(secret) if !secret.trim().is_empty() => {
                        self.store_if_readable(
                            provider,
                            reference,
                            config_key,
                            Some(&secret),
                            now_ms,
                        );
                        Ok(Some(secret))
                    }
                    Some(_) | None => Ok(None),
                }
            })
            .await
    }

    /// Removes any cached entry for `(provider, reference)`.
    pub fn invalidate(&self, provider: &str, reference: &str) -> bool {
        let key = SecretCacheKey::new(provider, reference);
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key)
            .is_some()
    }

    /// Removes all entries from the cache.
    pub fn clear(&self) {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Returns the number of entries currently stored (including not-yet-purged expired ones).
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Returns `true` when no entries are currently stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
