//! HTTP and WebSocket credential precedence, session lease tracking, and TTL secret caching.
//!
//! Fixes BUG-03, BUG-04, and BUG-15 across `EnvironmentAuth`, `SessionStore`, and
//! `SecretResolver`.
//!
//! # Examples
//!
//! ```
//! use legion_contracts::permissions::EnvironmentScope;
//! use legion_protocol::auth::{
//!     CredentialPrecedence, CredentialSource, NewSessionLease, RequestCredentials,
//!     SessionLeaseRegistry, SessionVerificationError, TtlSecretCache,
//! };
//!
//! // BUG-03: Authorization header wins over ambient session cookies.
//! let cred = CredentialPrecedence::select(&RequestCredentials {
//!     authorization: Some("Bearer header-tok"),
//!     primary_cookie: Some("cookie-tok"),
//!     ..Default::default()
//! })?;
//! assert_eq!(cred.source, CredentialSource::AuthorizationBearer);
//! assert_eq!(cred.token, "header-tok");
//!
//! // Malformed Authorization header never falls back to cookies.
//! assert!(CredentialPrecedence::select(&RequestCredentials {
//!     authorization: Some("Basic bad"),
//!     primary_cookie: Some("cookie-tok"),
//!     ..Default::default()
//! }).is_err());
//!
//! // BUG-04: verify_session enforces db_expires_at_ms > now_ms.
//! let registry = SessionLeaseRegistry::new();
//! registry.register(NewSessionLease {
//!     session_id: "sess-1".to_owned(),
//!     subject: "user-1".to_owned(),
//!     scopes: vec![EnvironmentScope::OrchestrationRead],
//!     proof_key_thumbprint: None,
//!     renewable: false,
//!     issued_at_ms: 1_000,
//!     db_expires_at_ms: 2_000,
//!     label: None,
//! })?;
//! assert!(matches!(
//!     registry.verify_session("sess-1", 5_000, 2_000),
//!     Err(SessionVerificationError::DatabaseLeaseExpired { expired_at_ms: 2_000 })
//! ));
//!
//! // BUG-15: TtlSecretCache skips None and blank entries.
//! let cache = TtlSecretCache::new(60_000);
//! assert!(!cache.store_if_readable("keychain", "api-key", "v1", Some("   "), 1_000));
//! assert!(cache.get_if_valid("keychain", "api-key", "v1", 1_001).is_none());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod lease_registry;
mod lease_state;
mod lease_types;
mod precedence;
mod secret_cache;

pub use lease_registry::SessionLeaseRegistry;
pub use lease_types::{
    ActiveSessionSummary, ConnectionInvalidationReason, ConnectionState, NewSessionLease,
    PEER_LINK_RENEWAL_STEP_MS, PEER_LINK_SESSION_TTL_MS, PeerLeaseTouch, RenewDpopSessionInput,
    SessionLease, SessionRenewalError, SessionVerificationError,
};
pub use precedence::{
    CredentialPrecedence, CredentialSelectionError, CredentialSource, DpopBindingError,
    RequestCredentials, SelectedCredential, extract_cookie_value, verify_dpop_credential_binding,
};
pub use secret_cache::{
    DEFAULT_SECRET_CACHE_TTL_MS, SecretCacheKey, TtlSecretCache, is_readable_secret,
};
