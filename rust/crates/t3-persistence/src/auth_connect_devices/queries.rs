//! SQLite queries for waiting T3 Connect approval links and trusted devices.

use std::collections::HashSet;

use rusqlite::Connection;

use crate::auth_records::{AuthRecordsError, ClientMetadataRow};

use super::{TrustedConnectDevice, connect_device_verification_code};

pub(super) struct WaitingLinkRow {
    pub(super) pairing_link_id: String,
    pub(super) proof_key_thumbprint: String,
    pub(super) client: Option<ClientMetadataRow>,
}

pub(super) fn query_waiting_links(
    conn: &Connection,
    now_iso: &str,
) -> Result<Vec<WaitingLinkRow>, AuthRecordsError> {
    if !table_exists(conn, "auth_pairing_links")? {
        return Ok(Vec::new());
    }
    let link_cols = table_columns(conn, "auth_pairing_links")?;
    if !link_cols.contains("requires_connect_approval") {
        return Ok(Vec::new());
    }
    let has_connect_table = table_exists(conn, "auth_connect_devices")?;
    let client_col = if link_cols.contains("requested_client_json") {
        "requested_client_json"
    } else {
        "NULL"
    };
    let not_trusted_clause = if has_connect_table {
        "AND proof_key_thumbprint NOT IN (SELECT proof_key_thumbprint FROM auth_connect_devices)"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id, proof_key_thumbprint, {client_col}
         FROM auth_pairing_links
         WHERE revoked_at IS NULL
           AND consumed_at IS NULL
           AND expires_at > ?1
           AND requires_connect_approval = 1
           AND proof_key_thumbprint IS NOT NULL
           {not_trusted_clause}
         ORDER BY created_at DESC, id DESC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|source| sqlite_err("query_waiting_links:prepare", source))?;
    let rows = stmt
        .query_map([now_iso], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|source| sqlite_err("query_waiting_links:query", source))?;

    let mut out = Vec::new();
    for item in rows {
        let (pairing_link_id, proof_key_thumbprint, client_raw) =
            item.map_err(|source| sqlite_err("query_waiting_links:row", source))?;
        let client = parse_optional_client(client_raw.as_deref())?;
        out.push(WaitingLinkRow {
            pairing_link_id,
            proof_key_thumbprint,
            client,
        });
    }
    Ok(out)
}

pub(super) fn query_trusted_devices(
    conn: &Connection,
) -> Result<Vec<TrustedConnectDevice>, AuthRecordsError> {
    if !table_exists(conn, "auth_connect_devices")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT proof_key_thumbprint, client_json, approved_at
             FROM auth_connect_devices
             ORDER BY approved_at DESC, proof_key_thumbprint",
        )
        .map_err(|source| sqlite_err("query_trusted_devices:prepare", source))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|source| sqlite_err("query_trusted_devices:query", source))?;

    let mut out = Vec::new();
    for item in rows {
        let (proof_key_thumbprint, client_raw, approved_at) =
            item.map_err(|source| sqlite_err("query_trusted_devices:row", source))?;
        let verification_code = connect_device_verification_code(&proof_key_thumbprint);
        let client = parse_optional_client(client_raw.as_deref())?;
        out.push(TrustedConnectDevice {
            proof_key_thumbprint,
            verification_code,
            client,
            approved_at,
        });
    }
    Ok(out)
}

fn parse_optional_client(raw: Option<&str>) -> Result<Option<ClientMetadataRow>, AuthRecordsError> {
    match raw {
        Some(json) if !json.trim().is_empty() => {
            serde_json::from_str(json)
                .map(Some)
                .map_err(|source| AuthRecordsError::Json {
                    operation: "parse_connect_client_metadata",
                    source,
                })
        }
        _ => Ok(None),
    }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, AuthRecordsError> {
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

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>, AuthRecordsError> {
    let query = format!("PRAGMA table_info({table})");
    let mut stmt = conn
        .prepare(&query)
        .map_err(|source| sqlite_err("table_columns:prepare", source))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|source| sqlite_err("table_columns:query", source))?;
    let mut cols = HashSet::new();
    for row in rows {
        cols.insert(row.map_err(|source| sqlite_err("table_columns:row", source))?);
    }
    Ok(cols)
}

pub(super) fn sqlite_err(operation: &'static str, source: rusqlite::Error) -> AuthRecordsError {
    AuthRecordsError::Sqlite { operation, source }
}
