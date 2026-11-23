//! Shared SQLite transaction guard, schema check, and Connect-device cascade helpers for auth mutations.

use std::time::SystemTime;

use rusqlite::{Connection, params};

use crate::auth_records::AuthRecordsError;
use crate::sqlite::auth_peer_schema;

pub(crate) fn revoke_connect_device_tx(
    conn: &Connection,
    proof_key_thumbprint: &str,
    now_iso: &str,
) -> Result<bool, AuthRecordsError> {
    crate::pairing_tokens::revoke_unspent_links(
        conn,
        Some(proof_key_thumbprint),
        None,
        false,
        now_iso,
    )
    .map_err(|source| sqlite_err("revoke_connect_device:links", source))?;

    let removed_devices = conn
        .execute(
            "DELETE FROM auth_connect_devices WHERE proof_key_thumbprint = ?1",
            [proof_key_thumbprint],
        )
        .map_err(|source| sqlite_err("revoke_connect_device:device", source))?;

    let revoked_sessions = conn
        .execute(
            "UPDATE auth_sessions
             SET revoked_at = ?1
             WHERE proof_key_thumbprint = ?2
               AND revoked_at IS NULL",
            params![now_iso, proof_key_thumbprint],
        )
        .map_err(|source| sqlite_err("revoke_connect_device:sessions", source))?;

    Ok(removed_devices > 0 || revoked_sessions > 0)
}

pub(crate) fn revoke_other_connect_devices_tx(
    conn: &Connection,
    keep_proof_key_thumbprint: Option<&str>,
    now_iso: &str,
) -> Result<(usize, usize), AuthRecordsError> {
    // "Revoke all others" promises every other way in is gone, so unspent unbound codes go too.
    let revoked_links = crate::pairing_tokens::revoke_unspent_links(
        conn,
        None,
        keep_proof_key_thumbprint,
        true,
        now_iso,
    )
    .map_err(|source| sqlite_err("revoke_other_connect_devices:links", source))?;

    let removed_devices = conn
        .execute(
            "DELETE FROM auth_connect_devices
             WHERE (?1 IS NULL OR proof_key_thumbprint != ?1)",
            [keep_proof_key_thumbprint],
        )
        .map_err(|source| sqlite_err("revoke_other_connect_devices:devices", source))?;

    Ok((revoked_links.len(), removed_devices))
}

pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), AuthRecordsError> {
    auth_peer_schema::apply_auth_and_peer_ddl(conn)
        .map_err(|source| sqlite_err("apply_auth_and_peer_ddl", source))
}

pub(crate) fn current_epoch_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

pub(crate) struct TxGuard<'a> {
    conn: &'a Connection,
    savepoint: bool,
    committed: bool,
}

impl<'a> TxGuard<'a> {
    pub(crate) fn begin(conn: &'a Connection) -> Result<Self, AuthRecordsError> {
        let savepoint = !conn.is_autocommit();
        let sql = if savepoint {
            "SAVEPOINT legion_auth_mutations;"
        } else {
            "BEGIN IMMEDIATE;"
        };
        conn.execute_batch(sql)
            .map_err(|source| sqlite_err("begin_transaction", source))?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    pub(crate) fn commit(mut self) -> Result<(), AuthRecordsError> {
        let sql = if self.savepoint {
            "RELEASE SAVEPOINT legion_auth_mutations;"
        } else {
            "COMMIT;"
        };
        self.conn
            .execute_batch(sql)
            .map_err(|source| sqlite_err("commit_transaction", source))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for TxGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let sql = if self.savepoint {
                "ROLLBACK TO SAVEPOINT legion_auth_mutations; RELEASE SAVEPOINT legion_auth_mutations;"
            } else {
                "ROLLBACK;"
            };
            let _ = self.conn.execute_batch(sql);
        }
    }
}

pub(super) fn sqlite_err(operation: &'static str, source: rusqlite::Error) -> AuthRecordsError {
    AuthRecordsError::Sqlite { operation, source }
}
