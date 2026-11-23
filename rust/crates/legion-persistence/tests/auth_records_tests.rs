//! Integration tests for auth/peer DDL upgrades, queries, and CLI list formatters.

use legion_persistence::auth_records::{
    AuthRecordsError, AuthSessionRow, ClientMetadataRow, PeerLinkRecord,
    format_pairing_credential_list, format_session_list, insert_peer_link,
    list_active_pairing_links, list_active_sessions, list_peer_links, unlink_peer_link,
};
use legion_persistence::sqlite::SqliteStore;
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn auth_and_peer_ddl_creates_tables_and_upgrades_legacy_columns() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("legacy_auth.sqlite");

    // Seed pre-021 / pre-032 / pre-041 / pre-071 tables missing newer columns.
    let legacy = Connection::open(&db_path)?;
    legacy.execute_batch(
        "CREATE TABLE auth_pairing_links (
           id TEXT PRIMARY KEY, credential TEXT NOT NULL UNIQUE, method TEXT NOT NULL,
           scopes TEXT NOT NULL, subject TEXT NOT NULL, created_at TEXT NOT NULL,
           expires_at TEXT NOT NULL, consumed_at TEXT, revoked_at TEXT
         );
         INSERT INTO auth_pairing_links (id, credential, method, scopes, subject, created_at, expires_at)
         VALUES ('leg-1', 'c-leg', 'one-time-token', '[\"owner\"]', 'u1', '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z');
         CREATE TABLE auth_sessions (
           session_id TEXT PRIMARY KEY, subject TEXT NOT NULL, scopes TEXT NOT NULL,
           method TEXT NOT NULL, issued_at TEXT NOT NULL, expires_at TEXT NOT NULL,
           last_connected_at TEXT, revoked_at TEXT
         );
         CREATE TABLE peer_environment_links (
           environment_id TEXT PRIMARY KEY, secret_name TEXT NOT NULL, label TEXT NOT NULL,
           urls_json TEXT NOT NULL, access TEXT NOT NULL, linked_at TEXT NOT NULL,
           expires_at TEXT NOT NULL, last_reached_at TEXT, last_error TEXT
         );
         INSERT INTO peer_environment_links VALUES (
           'env-old', 'sec-old', 'Old Box', '[\"http://127.0.0.1:3773\"]', 'full',
           '2026-11-20T00:00:00Z', '2026-12-20T00:00:00Z', NULL, NULL
         );",
    )?;
    legacy.close().map_err(|(_, err)| err)?;

    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();
    for name in [
        "checkpoint_diff_blobs",
        "idx_checkpoint_diff_blobs_thread_to_turn",
        "auth_pairing_links",
        "idx_auth_pairing_links_active",
        "auth_sessions",
        "idx_auth_sessions_active",
        "peer_environment_links",
    ] {
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;
        assert_eq!(found, 1, "missing object {name}");
    }

    let links = list_active_pairing_links(conn, "2026-11-20T12:00:00Z")?;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].label, None);
    assert_eq!(links[0].proof_key_thumbprint, None);

    let peers = list_peer_links(conn)?;
    assert_eq!(peers.len(), 1);
    assert!(!peers[0].fenced);

    // Reopening remains idempotent and preserves checkpoint_diff_blobs uniqueness.
    conn.execute(
        "INSERT INTO checkpoint_diff_blobs VALUES ('t1', 0, 1, 'diff-a', '2026-11-20T00:00:00Z')",
        [],
    )?;
    store.close()?;
    let reopened = SqliteStore::open(&db_path)?;
    let count: i64 = reopened.connection().query_row(
        "SELECT COUNT(*) FROM checkpoint_diff_blobs WHERE thread_id = 't1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(count, 1);
    reopened.close()?;
    Ok(())
}

#[test]
fn pairing_links_filter_order_scope_parsing_and_formatting() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    conn.execute_batch(
        "INSERT INTO auth_pairing_links (id, credential, method, scopes, subject, label, created_at, expires_at, consumed_at, revoked_at) VALUES
           ('link-b', 'tok-b', 'one-time-token', '[\"owner\",\"read\"]', 'user-b', 'Second', '2026-11-20T02:00:00Z', '2026-11-21T00:00:00Z', NULL, NULL),
           ('link-a', 'tok-a', 'one-time-token', 'read   write', 'user-a', NULL, '2026-11-20T01:00:00Z', '2026-11-21T00:00:00Z', NULL, NULL),
           ('link-a2', 'tok-a2', 'one-time-token', '[\"read\"]', 'user-a2', '', '2026-11-20T01:00:00Z', '2026-11-21T00:00:00Z', NULL, NULL),
           ('boot', 'tok-boot', 'desktop-bootstrap', '[\"owner\"]', 'internal-administrative-bootstrap', 'Hidden', '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z', NULL, NULL),
           ('revoked', 'tok-rev', 'one-time-token', '[\"read\"]', 'u', NULL, '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z', NULL, '2026-11-20T05:00:00Z'),
           ('consumed', 'tok-con', 'one-time-token', '[\"read\"]', 'u', NULL, '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z', '2026-11-20T05:00:00Z', NULL),
           ('expired', 'tok-exp', 'one-time-token', '[\"read\"]', 'u', NULL, '2026-11-20T00:00:00Z', '2026-11-20T11:00:00Z', NULL, NULL);",
    )?;

    let active = list_active_pairing_links(conn, "2026-11-20T12:00:00Z")?;
    let ids: Vec<&str> = active.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["link-a", "link-a2", "link-b"]);
    assert_eq!(active[0].scopes, vec!["read", "write"]);
    assert_eq!(active[2].scopes, vec!["owner", "read"]);

    let text = format_pairing_credential_list(&active[..2], false)?;
    assert_eq!(
        text,
        "link-a\n  scopes: read write\n  created: 2026-11-20T01:00:00Z\n  expires: 2026-11-21T00:00:00Z\n\n\
         link-a2\n  scopes: read\n  created: 2026-11-20T01:00:00Z\n  expires: 2026-11-21T00:00:00Z\n"
    );
    let labeled = format_pairing_credential_list(&active[2..], false)?;
    assert_eq!(
        labeled,
        "link-b (Second)\n  scopes: owner read\n  created: 2026-11-20T02:00:00Z\n  expires: 2026-11-21T00:00:00Z\n"
    );

    let json_text = format_pairing_credential_list(&active[2..], true)?;
    let expected_json = "[\n  {\n    \"id\": \"link-b\",\n    \"label\": \"Second\",\n    \"scopes\": [\n      \"owner\",\n      \"read\"\n    ],\n    \"createdAt\": \"2026-11-20T02:00:00Z\",\n    \"expiresAt\": \"2026-11-21T00:00:00Z\"\n  }\n]\n";
    assert_eq!(json_text, expected_json);
    assert_eq!(
        format_pairing_credential_list(&[], false)?,
        "No active pairing credentials.\n"
    );
    assert_eq!(format_pairing_credential_list(&[], true)?, "[]\n");

    conn.execute(
        "INSERT INTO auth_pairing_links (id, credential, method, scopes, subject, created_at, expires_at)
         VALUES ('bad', 'tok-bad', 'one-time-token', '[\"unclosed', 'u', '2026-11-20T03:00:00Z', '2026-11-21T00:00:00Z')",
        [],
    )?;
    let bad_err = list_active_pairing_links(conn, "2026-11-20T12:00:00Z")
        .err()
        .ok_or("expected malformed JSON error")?;
    assert!(matches!(bad_err, AuthRecordsError::Json { .. }));
    Ok(())
}

#[test]
fn sessions_filter_order_and_format_match_cli() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    conn.execute_batch(
        "INSERT INTO auth_sessions (
           session_id, subject, scopes, method, client_label, client_ip_address,
           client_user_agent, client_device_type, client_os, client_browser,
           client_surface, client_app_version, issued_at, expires_at, last_connected_at, revoked_at
         ) VALUES
           ('sess-2', 'bob', '[\"owner\"]', 'pairing', 'Work Mac', '10.0.0.2', 'ua', 'desktop', 'macOS', 'Chrome', 'web', '0.1.10', '2026-11-20T02:00:00Z', '2026-11-21T00:00:00Z', '2026-11-20T10:00:00Z', NULL),
           ('sess-1', 'alice', 'read', 'bearer-token', NULL, NULL, NULL, 'unknown', NULL, NULL, NULL, NULL, '2026-11-20T01:00:00Z', '2026-11-21T00:00:00Z', NULL, NULL),
           ('sess-rev', 'eve', '[\"read\"]', 'bearer-token', NULL, NULL, NULL, 'unknown', NULL, NULL, NULL, NULL, '2026-11-20T00:00:00Z', '2026-11-21T00:00:00Z', NULL, '2026-11-20T04:00:00Z'),
           ('sess-exp', 'dan', '[\"read\"]', 'bearer-token', NULL, NULL, NULL, 'unknown', NULL, NULL, NULL, NULL, '2026-11-20T00:00:00Z', '2026-11-20T11:00:00Z', NULL, NULL);",
    )?;

    let mut sessions = list_active_sessions(conn, "2026-11-20T12:00:00Z")?;
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec!["sess-1", "sess-2"]);
    assert_eq!(sessions[0].client, ClientMetadataRow::default());
    sessions[1].connected = true;

    let rendered = format_session_list(&sessions, false)?;
    let expected = concat!(
        "sess-1\n",
        "  scopes: read\n",
        "  method: bearer-token\n",
        "  subject: alice\n",
        "  client: unlabeled client\n",
        "  issued: 2026-11-20T01:00:00Z\n",
        "  last connected: never\n",
        "  expires: 2026-11-21T00:00:00Z\n\n",
        "sess-2 connected\n",
        "  scopes: owner\n",
        "  method: pairing\n",
        "  subject: bob\n",
        "  client: Work Mac | desktop | macOS | Chrome | 10.0.0.2\n",
        "  issued: 2026-11-20T02:00:00Z\n",
        "  last connected: 2026-11-20T10:00:00Z\n",
        "  expires: 2026-11-21T00:00:00Z\n"
    );
    assert_eq!(rendered, expected);

    let json_single = format_session_list(&sessions[..1], true)?;
    let parsed: Vec<AuthSessionRow> = serde_json::from_str(&json_single)?;
    assert_eq!(parsed.len(), 1);
    assert!(json_single.contains("\"lastConnectedAt\": null"));
    assert_eq!(format_session_list(&[], false)?, "No active sessions.\n");
    assert_eq!(format_session_list(&[], true)?, "[]\n");
    Ok(())
}

#[test]
fn peer_links_insert_upsert_list_and_unlink_roundtrip() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    assert_eq!(list_peer_links(conn)?, Vec::new());

    let peer_b = PeerLinkRecord {
        environment_id: "env-b".to_owned(),
        secret_name: "sec-b".to_owned(),
        label: "Beta".to_owned(),
        urls: vec!["https://beta.ts.net".to_owned()],
        access: "read-only".to_owned(),
        linked_at: "2026-11-20T01:00:00Z".to_owned(),
        expires_at: "2026-12-20T01:00:00Z".to_owned(),
        last_reached_at: None,
        last_error: None,
        fenced: false,
    };
    let mut peer_a = PeerLinkRecord {
        environment_id: "env-a".to_owned(),
        secret_name: "sec-a-1".to_owned(),
        label: "Alpha".to_owned(),
        urls: vec!["http://127.0.0.1:3773".to_owned()],
        access: "full".to_owned(),
        linked_at: "2026-11-20T00:00:00Z".to_owned(),
        expires_at: "2026-12-20T00:00:00Z".to_owned(),
        last_reached_at: None,
        last_error: Some("timeout".to_owned()),
        fenced: false,
    };
    insert_peer_link(conn, &peer_b)?;
    insert_peer_link(conn, &peer_a)?;

    // Upsert updates existing environment_id row in place.
    peer_a.secret_name = "sec-a-2".to_owned();
    peer_a.fenced = true;
    peer_a.last_error = None;
    peer_a.last_reached_at = Some("2026-11-20T05:00:00Z".to_owned());
    insert_peer_link(conn, &peer_a)?;

    let listed = list_peer_links(conn)?;
    assert_eq!(listed, vec![peer_a, peer_b]);

    assert_eq!(unlink_peer_link(conn, "env-a")?, Some("sec-a-2".to_owned()));
    assert_eq!(unlink_peer_link(conn, "env-a")?, None);
    assert_eq!(list_peer_links(conn)?.len(), 1);
    Ok(())
}
