//! Offline SQLite mutations for revoking pairing links, issuing scoped bearer sessions,
//! and revoking client sessions (including cascading T3 Connect device trust revocation).
//!
//! Mirrors `apps/server/src/cli/auth.ts`, `apps/server/src/cliAuthFormat.ts`,
//! `apps/server/src/auth/EnvironmentAuth.ts`, `apps/server/src/persistence/AuthPairingLinks.ts`,
//! and `apps/server/src/persistence/AuthSessions.ts`.
//!
//! # Examples
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use t3_persistence::auth_mutations::{
//!     IssueSessionInput, format_issued_session, issue_scoped_session, parse_cli_duration_ms,
//!     revoke_session,
//! };
//! use t3_persistence::sqlite::SqliteStore;
//!
//! let store = SqliteStore::open_in_memory()?;
//! let ttl_ms = parse_cli_duration_ms("1h")?;
//! let issued = issue_scoped_session(
//!     store.connection(),
//!     IssueSessionInput::default().with_ttl_ms(ttl_ms).with_label("CI"),
//! )?;
//! let rendered = format_issued_session(&issued, false, false)?;
//! assert!(rendered.contains(&issued.token));
//! assert!(revoke_session(store.connection(), &issued.session_id)?);
//! # Ok(())
//! # }
//! ```

mod format;
mod store;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth_records::{AuthRecordsError, ClientMetadataRow};
use crate::ownership::format_epoch_millis_utc;
use crate::pairing_tokens::{
    DEFAULT_SESSION_TTL_MS, STANDARD_CLIENT_SCOPES, generate_bearer_token, sha256_hex,
};

pub use format::{format_issued_pairing_credential, format_issued_session, parse_cli_duration_ms};
use store::sqlite_err;
pub(crate) use store::{TxGuard, current_epoch_ms, ensure_schema, revoke_connect_device_tx};

/// Default subject stamped onto CLI-issued bearer sessions (`EnvironmentAuth.ts:60`).
pub const DEFAULT_CLI_SESSION_SUBJECT: &str = "cli-issued-session";

/// Parameters for [`issue_scoped_session`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IssueSessionInput {
    /// Granted authorization scopes (`STANDARD_CLIENT_SCOPES` when empty).
    pub scopes: Vec<String>,
    /// Optional lifetime in milliseconds (`30` days when `None`).
    pub ttl_ms: Option<u64>,
    /// Optional human-readable client label.
    pub label: Option<String>,
    /// Optional session subject (`"cli-issued-session"` when `None`).
    pub subject: Option<String>,
}

impl IssueSessionInput {
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

    /// Sets an explicit TTL in milliseconds.
    #[must_use]
    pub const fn with_ttl_ms(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = Some(ttl_ms);
        self
    }

    /// Sets an optional client label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets an explicit session subject.
    #[must_use]
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }
}

/// Bearer access session returned by [`issue_scoped_session`] (`EnvironmentAuth.ts:1342-1351`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedSession {
    /// Persisted `auth_sessions.session_id`.
    pub session_id: String,
    /// Raw secret bearer token (`t3s_...`).
    pub token: String,
    /// Session method (`"bearer-access-token"`).
    pub method: String,
    /// Granted authorization scopes.
    pub scopes: Vec<String>,
    /// Session subject.
    pub subject: String,
    /// Recorded client device metadata (`device_type = "bot"`).
    pub client: ClientMetadataRow,
    /// ISO-8601 UTC expiration timestamp.
    pub expires_at: String,
}

/// Revokes an active, unconsumed pairing link by `id` (`AuthPairingLinks.ts:320-339`).
///
/// Marks `connect_denied = requires_connect_approval` for waiting approval requests and leaves
/// already-approved Connect device links untouched.
pub fn revoke_pairing_link(conn: &Connection, id: &str) -> Result<bool, AuthRecordsError> {
    revoke_pairing_link_at(conn, id, current_epoch_ms())
}

/// Deterministic timestamp variant of [`revoke_pairing_link`].
pub fn revoke_pairing_link_at(
    conn: &Connection,
    id: &str,
    now_ms: i64,
) -> Result<bool, AuthRecordsError> {
    ensure_schema(conn)?;
    let now_iso = format_epoch_millis_utc(now_ms);
    match conn.query_row(
        "UPDATE auth_pairing_links
         SET revoked_at = ?1,
             connect_denied = requires_connect_approval
         WHERE id = ?2
           AND revoked_at IS NULL
           AND consumed_at IS NULL
           AND NOT (
             requires_connect_approval = 1
             AND proof_key_thumbprint IN (SELECT proof_key_thumbprint FROM auth_connect_devices)
           )
         RETURNING id",
        params![now_iso, id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(source) => Err(sqlite_err("revoke_pairing_link", source)),
    }
}

/// Issues and persists a scoped bearer session into `auth_sessions` (`EnvironmentAuth.ts:1328-1355`).
pub fn issue_scoped_session(
    conn: &Connection,
    input: IssueSessionInput,
) -> Result<IssuedSession, AuthRecordsError> {
    issue_scoped_session_at(conn, input, current_epoch_ms())
}

/// Deterministic timestamp variant of [`issue_scoped_session`].
pub fn issue_scoped_session_at(
    conn: &Connection,
    input: IssueSessionInput,
    now_ms: i64,
) -> Result<IssuedSession, AuthRecordsError> {
    ensure_schema(conn)?;
    let scopes: Vec<String> = if input.scopes.is_empty() {
        STANDARD_CLIENT_SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect()
    } else {
        input.scopes
    };
    let subject = input
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_CLI_SESSION_SUBJECT)
        .to_owned();
    let label = input
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let ttl_ms = match input.ttl_ms {
        Some(ttl) => i64::try_from(ttl).unwrap_or(i64::MAX),
        None => DEFAULT_SESSION_TTL_MS,
    };

    let session_id = Uuid::new_v4().to_string();
    let token = generate_bearer_token();
    let token_hash = sha256_hex(token.as_bytes());
    let method = "bearer-access-token".to_owned();
    let issued_at = format_epoch_millis_utc(now_ms);
    let expires_at = format_epoch_millis_utc(now_ms.saturating_add(ttl_ms));
    let scopes_json = serde_json::to_string(&scopes).map_err(|source| AuthRecordsError::Json {
        operation: "issue_scoped_session:scopes",
        source,
    })?;
    let client = ClientMetadataRow {
        label: label.clone(),
        device_type: "bot".to_owned(),
        ..ClientMetadataRow::default()
    };

    conn.execute(
        "INSERT INTO auth_sessions (
           session_id, subject, scopes, method, client_label, client_device_type,
           issued_at, expires_at, revoked_at, bearer_token_hash
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'bot', ?6, ?7, NULL, ?8)",
        params![
            session_id,
            subject,
            scopes_json,
            method,
            label,
            issued_at,
            expires_at,
            token_hash,
        ],
    )
    .map_err(|source| sqlite_err("issue_scoped_session:insert", source))?;

    Ok(IssuedSession {
        session_id,
        token,
        method,
        scopes,
        subject,
        client,
        expires_at,
    })
}

/// Revokes an active session, cascading to T3 Connect device trust and sibling device sessions
/// when `auth_sessions.proof_key_thumbprint` is populated (`auth.ts:356`, `EnvironmentAuth.ts:1443`).
pub fn revoke_session(conn: &Connection, session_id: &str) -> Result<bool, AuthRecordsError> {
    revoke_session_at(conn, session_id, current_epoch_ms())
}

/// Deterministic timestamp variant of [`revoke_session`].
pub fn revoke_session_at(
    conn: &Connection,
    session_id: &str,
    now_ms: i64,
) -> Result<bool, AuthRecordsError> {
    ensure_schema(conn)?;
    let now_iso = format_epoch_millis_utc(now_ms);
    let tx = TxGuard::begin(conn)?;
    let thumbprint = match conn.query_row(
        "SELECT proof_key_thumbprint FROM auth_sessions WHERE session_id = ?1 AND proof_key_thumbprint IS NOT NULL",
        [session_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(value) => Some(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(source) => return Err(sqlite_err("revoke_session:proof_key", source)),
    };

    let revoked = if let Some(key) = thumbprint {
        revoke_connect_device_tx(conn, &key, &now_iso)?
    } else {
        match conn.query_row(
            "UPDATE auth_sessions SET revoked_at = ?1 WHERE session_id = ?2 AND revoked_at IS NULL RETURNING session_id",
            params![now_iso, session_id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(_) => true,
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(source) => return Err(sqlite_err("revoke_session:update", source)),
        }
    };

    tx.commit()?;
    Ok(revoked)
}

/// Counts returned by [`revoke_other_sessions`] and [`revoke_other_connect_devices_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevokeOtherSessionsOutcome {
    /// Active sessions marked revoked (`session_id != keep_session_id`).
    pub revoked_sessions: usize,
    /// Unspent pairing links/codes revoked (including unbound pairing & approval codes).
    pub revoked_pairing_links: usize,
    /// T3 Connect trusted devices removed from `auth_connect_devices`.
    pub removed_connect_devices: usize,
}

/// Revokes every active session except `keep_session_id`, removes other trusted Connect devices,
/// and revokes all unspent pairing tokens/codes (`ebf7b9a4c5`).
pub fn revoke_other_sessions(
    conn: &Connection,
    keep_session_id: &str,
) -> Result<RevokeOtherSessionsOutcome, AuthRecordsError> {
    revoke_other_sessions_at(conn, keep_session_id, current_epoch_ms())
}

/// Deterministic timestamp variant of [`revoke_other_sessions`].
pub fn revoke_other_sessions_at(
    conn: &Connection,
    keep_session_id: &str,
    now_ms: i64,
) -> Result<RevokeOtherSessionsOutcome, AuthRecordsError> {
    ensure_schema(conn)?;
    let now_iso = format_epoch_millis_utc(now_ms);
    let tx = TxGuard::begin(conn)?;

    let keep_thumbprint = match conn.query_row(
        "SELECT proof_key_thumbprint
         FROM auth_sessions
         WHERE session_id = ?1
           AND proof_key_thumbprint IS NOT NULL",
        [keep_session_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(value) => Some(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(source) => return Err(sqlite_err("revoke_other_sessions:keep_thumbprint", source)),
    };

    let (revoked_pairing_links, removed_connect_devices) =
        store::revoke_other_connect_devices_tx(conn, keep_thumbprint.as_deref(), &now_iso)?;

    let revoked_sessions = conn
        .execute(
            "UPDATE auth_sessions
             SET revoked_at = ?1
             WHERE session_id != ?2
               AND revoked_at IS NULL",
            params![now_iso, keep_session_id],
        )
        .map_err(|source| sqlite_err("revoke_other_sessions:sessions", source))?;

    tx.commit()?;
    Ok(RevokeOtherSessionsOutcome {
        revoked_sessions,
        revoked_pairing_links,
        removed_connect_devices,
    })
}

/// Revokes all trusted Connect devices except `keep_proof_key_thumbprint`, their sessions,
/// and all unspent pairing links/codes (`PairingGrantStore.revokeConnectDevicesExcept`, #39495).
pub fn revoke_other_connect_devices_at(
    conn: &Connection,
    keep_proof_key_thumbprint: Option<&str>,
    now_ms: i64,
) -> Result<RevokeOtherSessionsOutcome, AuthRecordsError> {
    ensure_schema(conn)?;
    let now_iso = format_epoch_millis_utc(now_ms);
    let tx = TxGuard::begin(conn)?;

    let (revoked_pairing_links, removed_connect_devices) =
        store::revoke_other_connect_devices_tx(conn, keep_proof_key_thumbprint, &now_iso)?;

    let revoked_sessions = conn
        .execute(
            "UPDATE auth_sessions
             SET revoked_at = ?1
             WHERE proof_key_thumbprint IS NOT NULL
               AND proof_key_thumbprint IS NOT ?2
               AND revoked_at IS NULL",
            params![now_iso, keep_proof_key_thumbprint],
        )
        .map_err(|source| sqlite_err("revoke_other_connect_devices:sessions", source))?;

    tx.commit()?;
    Ok(RevokeOtherSessionsOutcome {
        revoked_sessions,
        revoked_pairing_links,
        removed_connect_devices,
    })
}
