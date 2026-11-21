//! SQLite queries for active pairing links, active sessions, and peer environment links.

use rusqlite::{Connection, params};

use super::{
    AuthRecordsError, AuthSessionRow, ClientMetadataRow, PairingLinkRow, PeerLinkRecord, json_err,
    sqlite_err,
};

/// Lists active non-bootstrap pairing links ordered by `created_at ASC, id ASC`.
///
/// Filters `revoked_at IS NULL AND consumed_at IS NULL AND expires_at > now_iso AND subject != 'internal-administrative-bootstrap'`
/// and parses `scopes` from either a JSON string array or a whitespace-separated token string.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::auth_records::list_active_pairing_links;
/// use t3_persistence::sqlite::SqliteStore;
///
/// let store = SqliteStore::open_in_memory()?;
/// store.connection().execute(
///     "INSERT INTO auth_pairing_links (
///        id, credential, method, scopes, subject, label, created_at, expires_at
///      ) VALUES ('p1', 'tok-1', 'one-time-token', '[\"owner\"]', 'cli-user', 'Laptop', '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z')",
///     [],
/// )?;
/// let rows = list_active_pairing_links(store.connection(), "2026-11-20T12:00:00Z")?;
/// assert_eq!(rows.len(), 1);
/// assert_eq!(rows[0].scopes, vec!["owner"]);
/// # Ok(())
/// # }
/// ```
pub fn list_active_pairing_links(
    conn: &Connection,
    now_iso: &str,
) -> Result<Vec<PairingLinkRow>, AuthRecordsError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, credential, method, scopes, subject, label, created_at, expires_at,
                    consumed_at, revoked_at, proof_key_thumbprint
             FROM auth_pairing_links
             WHERE revoked_at IS NULL
               AND consumed_at IS NULL
               AND expires_at > ?1
               AND subject != 'internal-administrative-bootstrap'
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|source| sqlite_err("list_active_pairing_links:prepare", source))?;

    let raw_rows = stmt
        .query_map([now_iso], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(|source| sqlite_err("list_active_pairing_links:query", source))?;

    let mut out = Vec::new();
    for item in raw_rows {
        let (
            id,
            credential,
            method,
            scopes_raw,
            subject,
            label,
            created_at,
            expires_at,
            consumed_at,
            revoked_at,
            proof_key_thumbprint,
        ) = item.map_err(|source| sqlite_err("list_active_pairing_links:row", source))?;
        out.push(PairingLinkRow {
            id,
            credential,
            method,
            scopes: parse_scopes(&scopes_raw)?,
            subject,
            label,
            created_at,
            expires_at,
            consumed_at,
            revoked_at,
            proof_key_thumbprint,
        });
    }
    Ok(out)
}

/// Lists active auth sessions (`revoked_at IS NULL AND expires_at > now_iso`) ordered by
/// `issued_at ASC, session_id ASC`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::auth_records::list_active_sessions;
/// use t3_persistence::sqlite::SqliteStore;
///
/// let store = SqliteStore::open_in_memory()?;
/// store.connection().execute(
///     "INSERT INTO auth_sessions (
///        session_id, subject, scopes, method, issued_at, expires_at
///      ) VALUES ('s1', 'cli-user', 'read write', 'bearer-token', '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z')",
///     [],
/// )?;
/// let rows = list_active_sessions(store.connection(), "2026-11-20T12:00:00Z")?;
/// assert_eq!(rows.len(), 1);
/// assert_eq!(rows[0].scopes, vec!["read", "write"]);
/// # Ok(())
/// # }
/// ```
pub fn list_active_sessions(
    conn: &Connection,
    now_iso: &str,
) -> Result<Vec<AuthSessionRow>, AuthRecordsError> {
    let mut stmt = conn
        .prepare(
            "SELECT session_id, subject, scopes, method,
                    client_label, client_ip_address, client_user_agent, client_device_type,
                    client_os, client_browser, client_surface, client_app_version,
                    issued_at, expires_at, last_connected_at, revoked_at
             FROM auth_sessions
             WHERE revoked_at IS NULL
               AND expires_at > ?1
             ORDER BY issued_at ASC, session_id ASC",
        )
        .map_err(|source| sqlite_err("list_active_sessions:prepare", source))?;

    let raw_rows = stmt
        .query_map([now_iso], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                ClientMetadataRow {
                    label: row.get(4)?,
                    ip_address: row.get(5)?,
                    user_agent: row.get(6)?,
                    device_type: row.get(7)?,
                    os: row.get(8)?,
                    browser: row.get(9)?,
                    surface: row.get(10)?,
                    app_version: row.get(11)?,
                },
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<String>>(15)?,
            ))
        })
        .map_err(|source| sqlite_err("list_active_sessions:query", source))?;

    let mut out = Vec::new();
    for item in raw_rows {
        let (
            session_id,
            subject,
            scopes_raw,
            method,
            client,
            issued_at,
            expires_at,
            last_connected_at,
            revoked_at,
        ) = item.map_err(|source| sqlite_err("list_active_sessions:row", source))?;
        out.push(AuthSessionRow {
            session_id,
            subject,
            scopes: parse_scopes(&scopes_raw)?,
            method,
            client,
            issued_at,
            expires_at,
            last_connected_at,
            revoked_at,
            connected: false,
        });
    }
    Ok(out)
}

/// Lists all rows from `peer_environment_links` ordered by `label ASC, environment_id ASC`.
pub fn list_peer_links(conn: &Connection) -> Result<Vec<PeerLinkRecord>, AuthRecordsError> {
    let mut stmt = conn
        .prepare(
            "SELECT environment_id, secret_name, label, urls_json, access,
                    linked_at, expires_at, last_reached_at, last_error, fenced
             FROM peer_environment_links
             ORDER BY label ASC, environment_id ASC",
        )
        .map_err(|source| sqlite_err("list_peer_links:prepare", source))?;

    let raw_rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })
        .map_err(|source| sqlite_err("list_peer_links:query", source))?;

    let mut out = Vec::new();
    for item in raw_rows {
        let (
            environment_id,
            secret_name,
            label,
            urls_json,
            access,
            linked_at,
            expires_at,
            last_reached_at,
            last_error,
            fenced,
        ) = item.map_err(|source| sqlite_err("list_peer_links:row", source))?;
        let urls: Vec<String> = serde_json::from_str(&urls_json)
            .map_err(|source| json_err("list_peer_links:urls_json", source))?;
        out.push(PeerLinkRecord {
            environment_id,
            secret_name,
            label,
            urls,
            access,
            linked_at,
            expires_at,
            last_reached_at,
            last_error,
            fenced: fenced != 0,
        });
    }
    Ok(out)
}

/// Inserts or updates a row in `peer_environment_links`, matching `PeerLinks.ts:550-569`.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::auth_records::{PeerLinkRecord, insert_peer_link, list_peer_links, unlink_peer_link};
/// use t3_persistence::sqlite::SqliteStore;
///
/// let store = SqliteStore::open_in_memory()?;
/// let record = PeerLinkRecord {
///     environment_id: "env-1".to_owned(),
///     secret_name: "peer-link-secret-1".to_owned(),
///     label: "Workstation".to_owned(),
///     urls: vec!["https://peer.example.ts.net".to_owned()],
///     access: "full".to_owned(),
///     linked_at: "2026-11-20T00:00:00Z".to_owned(),
///     expires_at: "2026-12-20T00:00:00Z".to_owned(),
///     last_reached_at: None,
///     last_error: None,
///     fenced: true,
/// };
/// insert_peer_link(store.connection(), &record)?;
/// assert_eq!(list_peer_links(store.connection())?.len(), 1);
/// assert_eq!(unlink_peer_link(store.connection(), "env-1")?, Some("peer-link-secret-1".to_owned()));
/// # Ok(())
/// # }
/// ```
pub fn insert_peer_link(
    conn: &Connection,
    record: &PeerLinkRecord,
) -> Result<(), AuthRecordsError> {
    let urls_json = serde_json::to_string(&record.urls)
        .map_err(|source| json_err("insert_peer_link:urls_json", source))?;
    let fenced_flag: i64 = i64::from(record.fenced);
    conn.execute(
        "INSERT INTO peer_environment_links (
           environment_id, secret_name, label, urls_json, access, linked_at,
           expires_at, last_reached_at, last_error, fenced
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (environment_id) DO UPDATE SET
           secret_name = excluded.secret_name,
           fenced = excluded.fenced,
           label = excluded.label,
           urls_json = excluded.urls_json,
           access = excluded.access,
           linked_at = excluded.linked_at,
           expires_at = excluded.expires_at,
           last_reached_at = excluded.last_reached_at,
           last_error = excluded.last_error",
        params![
            record.environment_id,
            record.secret_name,
            record.label,
            urls_json,
            record.access,
            record.linked_at,
            record.expires_at,
            record.last_reached_at,
            record.last_error,
            fenced_flag,
        ],
    )
    .map_err(|source| sqlite_err("insert_peer_link", source))?;
    Ok(())
}

/// Removes `environment_id` from `peer_environment_links`, returning its `secret_name` if deleted.
pub fn unlink_peer_link(
    conn: &Connection,
    environment_id: &str,
) -> Result<Option<String>, AuthRecordsError> {
    match conn.query_row(
        "DELETE FROM peer_environment_links WHERE environment_id = ?1 RETURNING secret_name",
        [environment_id],
        |row| row.get::<_, String>(0),
    ) {
        Ok(secret_name) => Ok(Some(secret_name)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(source) => Err(sqlite_err("unlink_peer_link", source)),
    }
}

fn parse_scopes(raw: &str) -> Result<Vec<String>, AuthRecordsError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map_err(|source| json_err("parse_scopes", source))
    } else {
        Ok(trimmed.split_whitespace().map(str::to_owned).collect())
    }
}
