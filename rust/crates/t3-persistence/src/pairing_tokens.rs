//! One-time pairing link issuance, atomic exchange into `auth_sessions`, and `BUG-04`-safe
//! bearer session verification against `statev2.sqlite`.
//!
//! Mirrors `apps/server/src/auth/EnvironmentAuth.ts`,
//! `apps/server/src/auth/PairingGrantStore.ts`, `apps/server/src/auth/SessionStore.ts`,
//! `apps/server/src/persistence/AuthPairingLinks.ts`, and
//! `apps/server/src/persistence/AuthSessions.ts`.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_persistence::pairing_tokens::{
//!     IssuePairingTokenInput, exchange_pairing_token, issue_pairing_token, verify_bearer_session,
//! };
//!
//! let dir = tempfile::tempdir()?;
//! let db_path = dir.path().join("statev2.sqlite");
//! let now_ms = 1_700_000_000_000;
//! let issued = issue_pairing_token(
//!     &db_path,
//!     &IssuePairingTokenInput::default().with_label("Laptop"),
//!     now_ms,
//! )?;
//! let session = exchange_pairing_token(&db_path, &issued.credential, Some("Mac"), now_ms + 1_000)?;
//! let verified = verify_bearer_session(&db_path, &session.token, now_ms + 2_000)?;
//! assert_eq!(verified.session_id, session.session_id);
//! assert_eq!(verified.role, "client");
//! # Ok(())
//! # }
//! ```

mod crypto;
mod queries;
mod store;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::sqlite::SqliteStoreError;

pub(crate) use crypto::{generate_bearer_token, sha256_hex};
pub use queries::{
    exchange_pairing_token, exchange_pairing_token_with_conn, issue_pairing_token,
    issue_pairing_token_with_conn, verify_bearer_session, verify_bearer_session_with_conn,
};
pub use store::ensure_pairing_schema;

/// Default one-time pairing credential lifetime (`5` minutes, `PairingGrantStore.ts:326`).
pub const DEFAULT_PAIRING_TOKEN_TTL_MS: i64 = 5 * 60 * 1_000;

/// Default issued bearer session lifetime (`30` days, `SessionStore.ts:483`).
pub const DEFAULT_SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Ambiguity-free Crockford-style pairing alphabet (`packages/contracts/src/auth.ts:293`).
pub const PAIRING_CODE_ALPHABET: &str = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Character count for generated pairing codes (`packages/contracts/src/auth.ts:294`).
pub const PAIRING_CODE_LENGTH: usize = 12;

/// Default scopes granted to standard client pairing links (`packages/contracts/src/auth.ts:156-173`).
pub const STANDARD_CLIENT_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "settings:write",
    "providers:manage",
    "environment:maintain",
    "preview:operate",
    "diagnostics:read",
    "terminal:read",
    "terminal:operate",
    "source-control:write",
    "filesystem:read",
    "filesystem:write",
    "relay:read",
    "access:read",
    "access:write",
    "relay:write",
];

/// Administrative scopes including `relay:configure` (`packages/contracts/src/auth.ts:175-178`).
pub const ADMINISTRATIVE_CLIENT_SCOPES: &[&str] = &[
    "orchestration:read",
    "orchestration:operate",
    "settings:write",
    "providers:manage",
    "environment:maintain",
    "preview:operate",
    "diagnostics:read",
    "terminal:read",
    "terminal:operate",
    "source-control:write",
    "filesystem:read",
    "filesystem:write",
    "relay:read",
    "access:read",
    "access:write",
    "relay:write",
    "relay:configure",
];

/// Errors produced when issuing/exchanging pairing links or verifying bearer sessions.
#[derive(Debug, Error)]
pub enum PairingTokenError {
    /// Failed to open or close the SQLite database file.
    #[error("failed to open auth database '{}': {source}", path.display())]
    OpenDatabase {
        /// Target SQLite file path.
        path: PathBuf,
        /// Underlying [`SqliteStoreError`].
        #[source]
        source: Box<SqliteStoreError>,
    },
    /// Underlying SQLite query or statement failure.
    #[error("pairing token SQLite error during '{operation}': {source}")]
    Sqlite {
        /// Operation label.
        operation: &'static str,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
    /// JSON encoding/decoding failure for `scopes`.
    #[error("pairing token JSON error during '{operation}': {source}")]
    Json {
        /// Operation label.
        operation: &'static str,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Pairing link TTL must be positive (`> 0`).
    #[error("Pairing token TTL must be positive milliseconds (got {ttl_ms}).")]
    InvalidTtl {
        /// Invalid TTL value provided by caller.
        ttl_ms: i64,
    },
    /// Pairing credential or bearer token was not recognized.
    #[error("That pairing code or session token is unknown.")]
    UnknownToken,
    /// Pairing credential was already exchanged in an earlier transaction.
    #[error("That pairing code has already been used.")]
    AlreadyConsumed,
    /// Pairing credential was explicitly revoked.
    #[error("That pairing code was revoked.")]
    TokenRevoked,
    /// Pairing credential passed its expiration timestamp.
    #[error("That pairing code expired at {expires_at_iso}.")]
    TokenExpired {
        /// Recorded ISO-8601 UTC expiration timestamp.
        expires_at_iso: String,
    },
    /// Bearer session row carries a `revoked_at` timestamp (`BUG-04`).
    #[error("Session was revoked at {revoked_at_iso}.")]
    SessionRevoked {
        /// Recorded ISO-8601 UTC revocation timestamp.
        revoked_at_iso: String,
    },
    /// Bearer session passed its `expires_at` deadline (`BUG-04`).
    #[error("Session expired at {expires_at_iso}.")]
    SessionExpired {
        /// Recorded ISO-8601 UTC expiration timestamp.
        expires_at_iso: String,
        /// Parsed expiration timestamp in Unix epoch milliseconds.
        expires_at_ms: i64,
    },
}

/// Parameters for [`issue_pairing_token`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuePairingTokenInput {
    /// Authorization scopes granted once the pairing credential is exchanged.
    pub scopes: Vec<String>,
    /// Lifetime in milliseconds from `now_ms` (`> 0`).
    pub ttl_ms: i64,
    /// Optional human-readable client label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Default for IssuePairingTokenInput {
    fn default() -> Self {
        Self {
            scopes: STANDARD_CLIENT_SCOPES
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
            ttl_ms: DEFAULT_PAIRING_TOKEN_TTL_MS,
            label: None,
        }
    }
}

impl IssuePairingTokenInput {
    /// Overrides the granted scope list.
    #[must_use]
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Overrides the token lifetime in milliseconds.
    #[must_use]
    pub const fn with_ttl_ms(mut self, ttl_ms: i64) -> Self {
        self.ttl_ms = ttl_ms;
        self
    }

    /// Sets an optional pairing label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// Newly issued one-time pairing credential persisted in `auth_pairing_links`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedPairingToken {
    /// 12-character one-time pairing code.
    pub credential: String,
    /// Unique pairing link identifier (`auth_pairing_links.id`).
    pub id: String,
    /// ISO-8601 UTC expiration timestamp.
    pub expires_at_iso: String,
    /// Unix epoch millisecond expiration timestamp.
    pub expires_at_ms: i64,
}

/// Session returned by [`exchange_pairing_token`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangedSession {
    /// Persisted `auth_sessions.session_id`.
    pub session_id: String,
    /// Secret bearer token for subsequent authenticated requests.
    pub token: String,
    /// Canonical one-time pairing credential consumed to create this session.
    pub credential: String,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Derived role (`"admin"` when administrative scopes are present, else `"client"`).
    pub role: String,
    /// ISO-8601 UTC expiration timestamp for the issued session.
    pub expires_at_iso: String,
}

/// Active bearer session returned by [`verify_bearer_session`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedBearerSession {
    /// Persisted `auth_sessions.session_id`.
    pub session_id: String,
    /// Subject bound to the session.
    pub subject: String,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Derived session role (`"admin"` or `"client"`).
    pub role: String,
    /// Session issuance method (`"bearer-access-token"`).
    pub method: String,
    /// Recorded client device label, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_label: Option<String>,
    /// ISO-8601 UTC issuance timestamp.
    pub issued_at_iso: String,
    /// ISO-8601 UTC expiration timestamp.
    pub expires_at_iso: String,
    /// Unix epoch millisecond expiration timestamp.
    pub expires_at_ms: i64,
}

/// Derives `"admin"` when `scopes` includes administrative/owner privileges, else `"client"`.
pub fn role_for_scopes(scopes: &[String]) -> String {
    if scopes
        .iter()
        .any(|scope| matches!(scope.as_str(), "relay:configure" | "owner" | "admin"))
    {
        "admin".to_owned()
    } else {
        "client".to_owned()
    }
}
