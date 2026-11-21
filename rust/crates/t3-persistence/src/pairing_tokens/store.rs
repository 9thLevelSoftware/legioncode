//! Internal SQLite storage, schema migrations, transaction guard, and atomic link consumption
//! for `pairing_tokens`.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection;

use crate::ownership::parse_iso8601_utc_millis;
use crate::sqlite::{SqliteOpenOptions, SqliteStore, auth_peer_schema};

use super::PairingTokenError;

pub(super) fn sqlite_err(operation: &'static str, source: rusqlite::Error) -> PairingTokenError {
    PairingTokenError::Sqlite { operation, source }
}

pub(super) fn json_err(operation: &'static str, source: serde_json::Error) -> PairingTokenError {
    PairingTokenError::Json { operation, source }
}

/// Ensures `auth_pairing_links`, `auth_sessions`, and `bearer_token_hash` index exist on `conn`.
pub fn ensure_pairing_schema(conn: &Connection) -> Result<(), PairingTokenError> {
    auth_peer_schema::apply_auth_and_peer_ddl(conn)
        .map_err(|source| sqlite_err("apply_auth_and_peer_ddl", source))
}

pub(super) struct ConsumedPairingLink {
    pub(super) credential: String,
    pub(super) scopes_json: String,
    pub(super) subject: String,
    pub(super) label: Option<String>,
}

pub(super) fn consume_available_link(
    conn: &Connection,
    normalized_credential: &str,
    now_iso: &str,
    now_ms: i64,
) -> Result<ConsumedPairingLink, PairingTokenError> {
    let updated = conn.query_row(
        "UPDATE auth_pairing_links
         SET consumed_at = ?1
         WHERE credential = ?2
           AND revoked_at IS NULL
           AND consumed_at IS NULL
           AND expires_at > ?1
         RETURNING credential, scopes, subject, label, expires_at",
        rusqlite::params![now_iso, normalized_credential],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    );

    match updated {
        Ok((credential, scopes_json, subject, label, expires_at_iso)) => {
            if parse_iso8601_utc_millis(&expires_at_iso).is_none_or(|exp| now_ms >= exp) {
                return Err(PairingTokenError::TokenExpired { expires_at_iso });
            }
            Ok(ConsumedPairingLink {
                credential,
                scopes_json,
                subject,
                label,
            })
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Err(classify_rejected_link(conn, normalized_credential, now_ms)?)
        }
        Err(source) => Err(sqlite_err("exchange_pairing_token:consume", source)),
    }
}

fn classify_rejected_link(
    conn: &Connection,
    normalized_credential: &str,
    now_ms: i64,
) -> Result<PairingTokenError, PairingTokenError> {
    let state = match conn.query_row(
        "SELECT revoked_at, consumed_at, expires_at
         FROM auth_pairing_links
         WHERE credential = ?1
         LIMIT 1",
        [normalized_credential],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    ) {
        Ok(state) => state,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(PairingTokenError::UnknownToken),
        Err(source) => return Err(sqlite_err("exchange_pairing_token:diagnose", source)),
    };
    let (revoked_at, consumed_at, expires_at_iso) = state;
    if revoked_at.is_some() {
        return Ok(PairingTokenError::TokenRevoked);
    }
    if consumed_at.is_some() {
        return Ok(PairingTokenError::AlreadyConsumed);
    }
    if parse_iso8601_utc_millis(&expires_at_iso).is_none_or(|exp| now_ms >= exp) {
        return Ok(PairingTokenError::TokenExpired { expires_at_iso });
    }
    Ok(PairingTokenError::UnknownToken)
}

pub(super) fn open_rw_store(db_path: &Path) -> Result<SqliteStore, PairingTokenError> {
    SqliteStore::open_with_options(
        db_path,
        SqliteOpenOptions {
            ensure_core_schema: false,
        },
    )
    .map_err(|source| PairingTokenError::OpenDatabase {
        path: db_path.to_path_buf(),
        source: Box::new(source),
    })
}

pub(super) fn table_exists(conn: &Connection, table: &str) -> Result<bool, PairingTokenError> {
    match conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(source) => Err(sqlite_err("check_table_exists", source)),
    }
}

pub(super) fn auth_sessions_columns(
    conn: &Connection,
) -> Result<HashSet<String>, PairingTokenError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(auth_sessions)")
        .map_err(|e| sqlite_err("pragma_auth_sessions_prepare", e))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| sqlite_err("pragma_auth_sessions_query", e))?;
    let mut cols = HashSet::new();
    for row in rows {
        cols.insert(row.map_err(|e| sqlite_err("pragma_auth_sessions_row", e))?);
    }
    Ok(cols)
}

pub(super) struct TxGuard<'a> {
    conn: &'a Connection,
    savepoint: bool,
    committed: bool,
}

impl<'a> TxGuard<'a> {
    pub(super) fn begin(conn: &'a Connection) -> Result<Self, PairingTokenError> {
        let savepoint = !conn.is_autocommit();
        let sql = if savepoint {
            "SAVEPOINT t3_pairing_tokens;"
        } else {
            "BEGIN IMMEDIATE;"
        };
        conn.execute_batch(sql)
            .map_err(|e| sqlite_err("begin_transaction", e))?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    pub(super) fn commit(mut self) -> Result<(), PairingTokenError> {
        let sql = if self.savepoint {
            "RELEASE SAVEPOINT t3_pairing_tokens;"
        } else {
            "COMMIT;"
        };
        self.conn
            .execute_batch(sql)
            .map_err(|e| sqlite_err("commit_transaction", e))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for TxGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let sql = if self.savepoint {
                "ROLLBACK TO SAVEPOINT t3_pairing_tokens; RELEASE SAVEPOINT t3_pairing_tokens;"
            } else {
                "ROLLBACK;"
            };
            let _ = self.conn.execute_batch(sql);
        }
    }
}
