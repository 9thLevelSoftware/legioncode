//! Idempotent DDL for checkpoint diff blobs, auth pairing/sessions, and peer environment links.
//!
//! Mirrors legacy migrations `003`, `020`, `021`, `031`, `032`, `041`, `070`, and `071`.

use std::collections::HashSet;

use rusqlite::Connection;

const CHECKPOINT_DIFF_BLOBS_SQL: &str = "
CREATE TABLE IF NOT EXISTS checkpoint_diff_blobs (
  thread_id TEXT NOT NULL,
  from_turn_count INTEGER NOT NULL,
  to_turn_count INTEGER NOT NULL,
  diff TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (thread_id, from_turn_count, to_turn_count)
);
CREATE INDEX IF NOT EXISTS idx_checkpoint_diff_blobs_thread_to_turn
  ON checkpoint_diff_blobs(thread_id, to_turn_count);
";

const AUTH_PAIRING_LINKS_SQL: &str = "
CREATE TABLE IF NOT EXISTS auth_pairing_links (
  id TEXT PRIMARY KEY,
  credential TEXT NOT NULL UNIQUE,
  method TEXT NOT NULL,
  scopes TEXT NOT NULL,
  subject TEXT NOT NULL,
  label TEXT,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT,
  revoked_at TEXT,
  proof_key_thumbprint TEXT
);
CREATE INDEX IF NOT EXISTS idx_auth_pairing_links_active
  ON auth_pairing_links(revoked_at, consumed_at, expires_at);
";

const AUTH_SESSIONS_SQL: &str = "
CREATE TABLE IF NOT EXISTS auth_sessions (
  session_id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  scopes TEXT NOT NULL,
  method TEXT NOT NULL,
  client_label TEXT,
  client_ip_address TEXT,
  client_user_agent TEXT,
  client_device_type TEXT NOT NULL DEFAULT 'unknown',
  client_os TEXT,
  client_browser TEXT,
  client_surface TEXT,
  client_app_version TEXT,
  issued_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  last_connected_at TEXT,
  revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_active
  ON auth_sessions(revoked_at, expires_at, issued_at);
";

const PEER_ENVIRONMENT_LINKS_SQL: &str = "
CREATE TABLE IF NOT EXISTS peer_environment_links (
  environment_id TEXT PRIMARY KEY,
  secret_name TEXT NOT NULL,
  label TEXT NOT NULL,
  urls_json TEXT NOT NULL,
  access TEXT NOT NULL,
  linked_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  last_reached_at TEXT,
  last_error TEXT,
  fenced INTEGER NOT NULL DEFAULT 0
);
";

const AUTH_CONNECT_DEVICES_SQL: &str = "
CREATE TABLE IF NOT EXISTS auth_connect_devices (
  proof_key_thumbprint TEXT PRIMARY KEY,
  client_json TEXT,
  approved_at TEXT NOT NULL
);
";

pub(crate) fn apply_auth_and_peer_ddl(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(CHECKPOINT_DIFF_BLOBS_SQL)?;
    conn.execute_batch(AUTH_PAIRING_LINKS_SQL)?;
    ensure_missing_columns(
        conn,
        "auth_pairing_links",
        &[
            ("scopes", "TEXT NOT NULL DEFAULT '[]'"),
            ("label", "TEXT"),
            ("proof_key_thumbprint", "TEXT"),
            ("requires_connect_approval", "INTEGER NOT NULL DEFAULT 0"),
            ("requested_client_json", "TEXT"),
            ("connect_denied", "INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;

    conn.execute_batch(AUTH_SESSIONS_SQL)?;
    ensure_missing_columns(
        conn,
        "auth_sessions",
        &[
            ("scopes", "TEXT NOT NULL DEFAULT '[]'"),
            ("client_label", "TEXT"),
            ("client_ip_address", "TEXT"),
            ("client_user_agent", "TEXT"),
            ("client_device_type", "TEXT NOT NULL DEFAULT 'unknown'"),
            ("client_os", "TEXT"),
            ("client_browser", "TEXT"),
            ("client_surface", "TEXT"),
            ("client_app_version", "TEXT"),
            ("last_connected_at", "TEXT"),
            ("proof_key_thumbprint", "TEXT"),
            ("bearer_token_hash", "TEXT"),
        ],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_auth_sessions_bearer_token_hash
           ON auth_sessions(bearer_token_hash);",
    )?;

    conn.execute_batch(AUTH_CONNECT_DEVICES_SQL)?;

    conn.execute_batch(PEER_ENVIRONMENT_LINKS_SQL)?;
    ensure_missing_columns(
        conn,
        "peer_environment_links",
        &[("fenced", "INTEGER NOT NULL DEFAULT 0")],
    )?;
    Ok(())
}

fn ensure_missing_columns(
    conn: &Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> rusqlite::Result<()> {
    let existing = table_columns(conn, table)?;
    for &(name, definition) in columns {
        if !existing.contains(name) {
            let alter = format!("ALTER TABLE {table} ADD COLUMN {name} {definition}");
            conn.execute_batch(&alter)?;
        }
    }
    Ok(())
}

fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let query = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = HashSet::new();
    for row in rows {
        columns.insert(row?);
    }
    Ok(columns)
}
