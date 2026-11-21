//! Public SQLite operations for issuing pairing tokens, exchanging them into sessions,
//! and verifying bearer tokens (`BUG-04`).

use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use uuid::Uuid;

use crate::ownership::{format_epoch_millis_utc, parse_iso8601_utc_millis};

use super::crypto::{
    generate_bearer_token, generate_pairing_credential, normalize_pairing_credential, sha256_hex,
};
use super::store::{
    TxGuard, auth_sessions_columns, consume_available_link, ensure_pairing_schema, json_err,
    open_rw_store, sqlite_err, table_exists,
};
use super::{
    DEFAULT_SESSION_TTL_MS, ExchangedSession, IssuePairingTokenInput, IssuedPairingToken,
    PairingTokenError, VerifiedBearerSession, role_for_scopes,
};

/// Issues and persists a one-time pairing credential in `db_path`.
pub fn issue_pairing_token(
    db_path: impl AsRef<Path>,
    input: &IssuePairingTokenInput,
    now_ms: i64,
) -> Result<IssuedPairingToken, PairingTokenError> {
    let store = open_rw_store(db_path.as_ref())?;
    let issued = issue_pairing_token_with_conn(store.connection(), input, now_ms)?;
    store
        .close()
        .map_err(|source| PairingTokenError::OpenDatabase {
            path: db_path.as_ref().to_path_buf(),
            source: Box::new(source),
        })?;
    Ok(issued)
}

/// Issues and persists a one-time pairing credential using an existing SQLite connection.
pub fn issue_pairing_token_with_conn(
    conn: &Connection,
    input: &IssuePairingTokenInput,
    now_ms: i64,
) -> Result<IssuedPairingToken, PairingTokenError> {
    if input.ttl_ms <= 0 {
        return Err(PairingTokenError::InvalidTtl {
            ttl_ms: input.ttl_ms,
        });
    }
    ensure_pairing_schema(conn)?;

    let id = Uuid::new_v4().to_string();
    let credential = generate_pairing_credential();
    let created_at_iso = format_epoch_millis_utc(now_ms);
    let expires_at_ms = now_ms.saturating_add(input.ttl_ms);
    let expires_at_iso = format_epoch_millis_utc(expires_at_ms);
    let scopes_json =
        serde_json::to_string(&input.scopes).map_err(|e| json_err("issue:scopes", e))?;
    let label = input
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    conn.execute(
        "INSERT INTO auth_pairing_links (
           id, credential, method, scopes, subject, label, created_at, expires_at,
           consumed_at, revoked_at, proof_key_thumbprint
         ) VALUES (?1, ?2, 'one-time-token', ?3, 'one-time-token', ?4, ?5, ?6, NULL, NULL, NULL)",
        params![
            id,
            credential,
            scopes_json,
            label,
            created_at_iso,
            expires_at_iso
        ],
    )
    .map_err(|e| sqlite_err("issue_pairing_token:insert", e))?;

    Ok(IssuedPairingToken {
        credential,
        id,
        expires_at_iso,
        expires_at_ms,
    })
}

/// Atomically exchanges a valid, unconsumed, unexpired `presented_token` in `db_path` for a new
/// bearer session persisted into `auth_sessions`.
pub fn exchange_pairing_token(
    db_path: impl AsRef<Path>,
    presented_token: &str,
    client_label: Option<&str>,
    now_ms: i64,
) -> Result<ExchangedSession, PairingTokenError> {
    let store = open_rw_store(db_path.as_ref())?;
    let exchanged = exchange_pairing_token_with_conn(
        store.connection(),
        presented_token,
        client_label,
        now_ms,
    )?;
    store
        .close()
        .map_err(|source| PairingTokenError::OpenDatabase {
            path: db_path.as_ref().to_path_buf(),
            source: Box::new(source),
        })?;
    Ok(exchanged)
}

/// Atomically exchanges a pairing credential on an existing SQLite connection.
pub fn exchange_pairing_token_with_conn(
    conn: &Connection,
    presented_token: &str,
    client_label: Option<&str>,
    now_ms: i64,
) -> Result<ExchangedSession, PairingTokenError> {
    let normalized = normalize_pairing_credential(presented_token);
    if normalized.is_empty() {
        return Err(PairingTokenError::UnknownToken);
    }
    ensure_pairing_schema(conn)?;

    let tx = TxGuard::begin(conn)?;
    let now_iso = format_epoch_millis_utc(now_ms);
    let consumed = consume_available_link(conn, &normalized, &now_iso, now_ms)?;

    let scopes: Vec<String> =
        serde_json::from_str(&consumed.scopes_json).map_err(|e| json_err("exchange:scopes", e))?;
    let role = role_for_scopes(&scopes);
    let session_id = Uuid::new_v4().to_string();
    let token = generate_bearer_token();
    let token_hash = sha256_hex(token.as_bytes());
    let expires_at_ms = now_ms.saturating_add(DEFAULT_SESSION_TTL_MS);
    let expires_at_iso = format_epoch_millis_utc(expires_at_ms);
    let resolved_label = client_label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or(consumed.label);

    conn.execute(
        "INSERT INTO auth_sessions (
           session_id, subject, scopes, method, client_label, client_device_type,
           issued_at, expires_at, revoked_at, bearer_token_hash
         ) VALUES (?1, ?2, ?3, 'bearer-access-token', ?4, 'unknown', ?5, ?6, NULL, ?7)",
        params![
            session_id,
            consumed.subject,
            consumed.scopes_json,
            resolved_label,
            now_iso,
            expires_at_iso,
            token_hash,
        ],
    )
    .map_err(|e| sqlite_err("exchange_pairing_token:insert_session", e))?;

    tx.commit()?;
    Ok(ExchangedSession {
        session_id,
        token,
        credential: consumed.credential,
        scopes,
        role,
        expires_at_iso,
    })
}

/// Verifies a bearer session `token` against `auth_sessions` in `db_path`, enforcing both
/// `revoked_at IS NULL` and `expires_at > now_ms` (`BUG-04`).
pub fn verify_bearer_session(
    db_path: impl AsRef<Path>,
    token: &str,
    now_ms: i64,
) -> Result<VerifiedBearerSession, PairingTokenError> {
    let db_path = db_path.as_ref();
    if db_path != Path::new(":memory:") && !db_path.exists() {
        return Err(PairingTokenError::UnknownToken);
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| sqlite_err("verify_bearer_session:open", e))?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")
        .map_err(|e| sqlite_err("verify_bearer_session:busy_timeout", e))?;
    let verified = verify_bearer_session_with_conn(&conn, token, now_ms);
    if let Err((_, source)) = conn.close()
        && verified.is_ok()
    {
        return Err(sqlite_err("verify_bearer_session:close", source));
    }
    verified
}

/// Verifies a bearer session `token` against `auth_sessions` on an existing connection, enforcing
/// both `revoked_at IS NULL` and `expires_at > now_ms` (`BUG-04`).
pub fn verify_bearer_session_with_conn(
    conn: &Connection,
    token: &str,
    now_ms: i64,
) -> Result<VerifiedBearerSession, PairingTokenError> {
    let trimmed = token.trim();
    if trimmed.is_empty() || !table_exists(conn, "auth_sessions")? {
        return Err(PairingTokenError::UnknownToken);
    }
    let cols = auth_sessions_columns(conn)?;
    if !cols.contains("bearer_token_hash") {
        return Err(PairingTokenError::UnknownToken);
    }

    let token_hash = sha256_hex(trimmed.as_bytes());
    let row = match conn.query_row(
        "SELECT session_id, subject, scopes, method, client_label, issued_at, expires_at, revoked_at
         FROM auth_sessions
         WHERE bearer_token_hash = ?1
         LIMIT 1",
        [token_hash],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        },
    ) {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Err(PairingTokenError::UnknownToken),
        Err(source) => return Err(sqlite_err("verify_bearer_session:select", source)),
    };

    let (
        session_id,
        subject,
        scopes_json,
        method,
        client_label,
        issued_at_iso,
        expires_at_iso,
        revoked_at,
    ) = row;
    if let Some(revoked_at_iso) = revoked_at {
        return Err(PairingTokenError::SessionRevoked { revoked_at_iso });
    }
    let Some(expires_at_ms) = parse_iso8601_utc_millis(&expires_at_iso) else {
        return Err(PairingTokenError::SessionExpired {
            expires_at_iso,
            expires_at_ms: 0,
        });
    };
    if now_ms >= expires_at_ms {
        return Err(PairingTokenError::SessionExpired {
            expires_at_iso,
            expires_at_ms,
        });
    }

    let scopes: Vec<String> =
        serde_json::from_str(&scopes_json).map_err(|e| json_err("verify:scopes", e))?;
    let role = role_for_scopes(&scopes);
    Ok(VerifiedBearerSession {
        session_id,
        subject,
        scopes,
        role,
        method,
        client_label,
        issued_at_iso,
        expires_at_iso,
        expires_at_ms,
    })
}
