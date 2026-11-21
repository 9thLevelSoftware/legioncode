//! `op://` secret reference parsing and `BUG-15` TTL caching via [`TtlSecretCache`].

use std::fmt;
use std::str::FromStr;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, de};
use t3_protocol::auth::{DEFAULT_SECRET_CACHE_TTL_MS, TtlSecretCache, is_readable_secret};
use thiserror::Error;

/// Maximum permitted length for a secret reference path (`secretReference.ts:18`).
pub const MAX_SECRET_REF_LENGTH: usize = 1024;
/// Maximum permitted length for a secret provider identifier (`secretReference.ts:9`).
pub const MAX_SECRET_PROVIDER_ID_LENGTH: usize = 64;

fn op_ref_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(
            r"^op://([A-Za-z0-9._-](?:[A-Za-z0-9 ._-]*[A-Za-z0-9._-])?)/([A-Za-z0-9._-](?:[A-Za-z0-9 ._-]*[A-Za-z0-9._-])?)(?:/([A-Za-z0-9._-](?:[A-Za-z0-9 ._-]*[A-Za-z0-9._-])?))?/([A-Za-z0-9._-](?:[A-Za-z0-9 ._-]*[A-Za-z0-9._-])?)(?:\?([a-z-]+=[A-Za-z0-9_-]+))?$",
        );
        // INFALLIBLE: constant compile-time validated 1Password op:// URI pattern.
        compiled.expect("op:// secret reference regex must compile")
    })
}

fn provider_slug_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]*$");
        // INFALLIBLE: constant compile-time validated provider slug pattern.
        compiled.expect("secret provider slug regex must compile")
    })
}

/// Errors produced when parsing a 1Password [`SecretRef`] or [`SecretReference`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecretRefError {
    /// Reference input was empty.
    #[error("secret reference cannot be empty")]
    Empty,
    /// Reference exceeded [`MAX_SECRET_REF_LENGTH`] (`1024`).
    #[error("secret reference exceeds {max} characters")]
    TooLong {
        /// Maximum character limit.
        max: usize,
    },
    /// Reference did not match `op://<vault>/<item>[/<section>]/<field>[?<k>=<v>]`.
    #[error("Use a 1Password reference like op://vault/item/field (got '{reference}').")]
    InvalidSyntax {
        /// Rejected reference text.
        reference: String,
    },
    /// Provider slug violated `SecretProviderSlug` (`^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, max 64).
    #[error("invalid secret provider id '{provider}'")]
    InvalidProviderId {
        /// Rejected provider identifier.
        provider: String,
    },
}

/// Parsed 1Password secret reference (`op://<vault>/<item>[/<section>]/<field>[?<query>]`).
///
/// Matches `packages/secret-provider-onepassword/src/shared/definition.ts:26-40`, preventing
/// template injection (`{{`, `}}`, newlines) and whitespace-edge segments.
///
/// # Examples
///
/// ```
/// use t3_remote::secrets::SecretRef;
///
/// let parsed = SecretRef::parse("op://Engineering/GitHub Token/credential?attribute=otp")?;
/// assert_eq!(parsed.vault(), "Engineering");
/// assert_eq!(parsed.item(), "GitHub Token");
/// assert_eq!(parsed.section(), None);
/// assert_eq!(parsed.field(), "credential");
/// assert_eq!(parsed.query(), Some("attribute=otp"));
/// # Ok::<(), t3_remote::secrets::SecretRefError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SecretRef {
    raw: String,
    #[serde(skip)]
    vault: String,
    #[serde(skip)]
    item: String,
    #[serde(skip)]
    section: Option<String>,
    #[serde(skip)]
    field: String,
    #[serde(skip)]
    query: Option<String>,
}

impl SecretRef {
    /// Parses and validates an `op://` reference string.
    pub fn parse(raw: &str) -> Result<Self, SecretRefError> {
        if raw.is_empty() {
            return Err(SecretRefError::Empty);
        }
        if raw.chars().count() > MAX_SECRET_REF_LENGTH {
            return Err(SecretRefError::TooLong {
                max: MAX_SECRET_REF_LENGTH,
            });
        }
        let Some(caps) = op_ref_regex().captures(raw) else {
            return Err(SecretRefError::InvalidSyntax {
                reference: raw.to_owned(),
            });
        };
        let Some(vault) = caps.get(1).map(|m| m.as_str().to_owned()) else {
            return Err(SecretRefError::InvalidSyntax {
                reference: raw.to_owned(),
            });
        };
        let Some(item) = caps.get(2).map(|m| m.as_str().to_owned()) else {
            return Err(SecretRefError::InvalidSyntax {
                reference: raw.to_owned(),
            });
        };
        let section = caps.get(3).map(|m| m.as_str().to_owned());
        let Some(field) = caps.get(4).map(|m| m.as_str().to_owned()) else {
            return Err(SecretRefError::InvalidSyntax {
                reference: raw.to_owned(),
            });
        };
        let query = caps.get(5).map(|m| m.as_str().to_owned());
        Ok(Self {
            raw: raw.to_owned(),
            vault,
            item,
            section,
            field,
            query,
        })
    }

    /// Full `op://...` string slice.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// 1Password vault name or ID.
    pub fn vault(&self) -> &str {
        &self.vault
    }

    /// 1Password item name or ID.
    pub fn item(&self) -> &str {
        &self.item
    }

    /// Optional 1Password section name or ID.
    pub fn section(&self) -> Option<&str> {
        self.section.as_deref()
    }

    /// 1Password field name or ID.
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Optional query parameter (`attribute=otp`, etc.).
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl FromStr for SecretRef {
    type Err = SecretRefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(de::Error::custom)
    }
}

/// Wire contract secret reference pointing at a configured provider (`secretReference.ts:23-26`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretReference {
    /// Configured provider identifier (`SecretProviderId`).
    pub provider: String,
    /// Provider-specific reference path (`<= 1024` chars, single line).
    pub reference: String,
}

impl SecretReference {
    /// Validates and constructs a [`SecretReference`].
    pub fn new(
        provider: impl AsRef<str>,
        reference: impl AsRef<str>,
    ) -> Result<Self, SecretRefError> {
        let p = provider.as_ref().trim();
        if p.is_empty()
            || p.chars().count() > MAX_SECRET_PROVIDER_ID_LENGTH
            || !provider_slug_regex().is_match(p)
        {
            return Err(SecretRefError::InvalidProviderId {
                provider: provider.as_ref().to_owned(),
            });
        }
        let r = reference.as_ref().trim();
        if r.is_empty() {
            return Err(SecretRefError::Empty);
        }
        if r.chars().count() > MAX_SECRET_REF_LENGTH {
            return Err(SecretRefError::TooLong {
                max: MAX_SECRET_REF_LENGTH,
            });
        }
        if r.contains('\r') || r.contains('\n') {
            return Err(SecretRefError::InvalidSyntax {
                reference: r.to_owned(),
            });
        }
        Ok(Self {
            provider: p.to_owned(),
            reference: r.to_owned(),
        })
    }
}

/// TTL-bounded resolver wrapper integrating [`SecretRef`] with [`TtlSecretCache`] (`BUG-15`).
///
/// Errors, `Ok(None)`, and blank/whitespace-only secret values are never memoized, ensuring
/// temporary 1Password CLI locks or timeouts recover on the next call.
#[derive(Debug, Clone)]
pub struct CachedSecretResolver {
    cache: TtlSecretCache,
}

impl Default for CachedSecretResolver {
    fn default() -> Self {
        Self::new(DEFAULT_SECRET_CACHE_TTL_MS)
    }
}

impl CachedSecretResolver {
    /// Creates a resolver caching readable secrets for `ttl_ms` milliseconds.
    pub fn new(ttl_ms: i64) -> Self {
        Self {
            cache: TtlSecretCache::new(ttl_ms),
        }
    }

    /// Returns a reference to the underlying [`TtlSecretCache`].
    pub fn cache(&self) -> &TtlSecretCache {
        &self.cache
    }

    /// Resolves `(provider, secret_ref)` using `fetcher` on cache miss, expiry, config change, or `fresh`.
    ///
    /// Never caches `Err`, `Ok(None)`, or whitespace-only values (`BUG-15`).
    pub fn resolve<F, E>(
        &self,
        provider: &str,
        secret_ref: &SecretRef,
        config_key: &str,
        now_ms: i64,
        fresh: bool,
        fetcher: F,
    ) -> Result<Option<String>, E>
    where
        F: FnOnce(&SecretRef) -> Result<Option<String>, E>,
    {
        if !fresh
            && let Some(hit) =
                self.cache
                    .get_if_valid(provider, secret_ref.as_str(), config_key, now_ms)
        {
            return Ok(Some(hit));
        }

        let resolved = fetcher(secret_ref)?;
        if is_readable_secret(resolved.as_deref()) {
            let _ = self.cache.store_if_readable(
                provider,
                secret_ref.as_str(),
                config_key,
                resolved.as_deref(),
                now_ms,
            );
            return Ok(resolved);
        }
        Ok(None)
    }

    /// Invalidates any cached value for `(provider, reference)`.
    pub fn invalidate(&self, provider: &str, reference: &str) -> bool {
        self.cache.invalidate(provider, reference)
    }

    /// Clears all cached entries.
    pub fn clear(&self) {
        self.cache.clear();
    }
}
