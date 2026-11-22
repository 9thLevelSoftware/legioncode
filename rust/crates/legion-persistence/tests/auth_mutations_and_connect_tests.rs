//! Integration tests for `legion_persistence::auth_mutations` and `auth_connect_devices`.

use std::error::Error;

use legion_persistence::auth_connect_devices::{
    ConnectApproveOutcome, ConnectRevokeOutcome, approve_connect_device_by_code_at,
    connect_device_verification_code, format_connect_device_list, list_connect_devices_at,
    normalize_verification_code, revoke_connect_device_by_code_at,
};
use legion_persistence::auth_mutations::{
    DEFAULT_CLI_SESSION_SUBJECT, IssueSessionInput, format_issued_pairing_credential,
    format_issued_session, issue_scoped_session_at, parse_cli_duration_ms, revoke_pairing_link_at,
    revoke_session_at,
};
use legion_persistence::auth_records::{PairingLinkRow, list_active_sessions};
use legion_persistence::ownership::format_epoch_millis_utc;
use legion_persistence::pairing_tokens::{
    IssuePairingTokenInput, issue_pairing_token_with_conn, verify_bearer_session_with_conn,
};
use legion_persistence::sqlite::SqliteStore;
use rusqlite::params;

#[test]
fn parses_cli_durations_and_rejects_invalid_inputs() -> Result<(), Box<dyn Error>> {
    assert_eq!(parse_cli_duration_ms("500ms")?, 500);
    assert_eq!(parse_cli_duration_ms("10s")?, 10_000);
    assert_eq!(parse_cli_duration_ms("5m")?, 300_000);
    assert_eq!(parse_cli_duration_ms("1h")?, 3_600_000);
    assert_eq!(parse_cli_duration_ms("30d")?, 2_592_000_000);
    assert_eq!(parse_cli_duration_ms("2w")?, 1_209_600_000);
    assert_eq!(parse_cli_duration_ms(" 15 minutes ")?, 900_000);
    assert_eq!(parse_cli_duration_ms("1.5 hours")?, 5_400_000);

    for bad in ["", "   ", "-5m", "1.5h", "10", "5y", "abc"] {
        assert!(
            parse_cli_duration_ms(bad).is_err(),
            "expected err for {bad}"
        );
    }
    Ok(())
}

#[test]
fn pairing_revocation_and_scoped_session_lifecycle() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let now_ms = 1_700_000_000_000_i64;

    let link = issue_pairing_token_with_conn(
        conn,
        &IssuePairingTokenInput::default().with_label("Laptop"),
        now_ms,
    )?;
    assert!(revoke_pairing_link_at(conn, &link.id, now_ms + 100)?);
    assert!(!revoke_pairing_link_at(conn, &link.id, now_ms + 200)?);
    assert!(!revoke_pairing_link_at(conn, "nonexistent", now_ms + 200)?);

    let session = issue_scoped_session_at(
        conn,
        IssueSessionInput::default()
            .with_scopes(["orchestration:read", "terminal:read"])
            .with_ttl_ms(60_000)
            .with_label("Runner Bot"),
        now_ms,
    )?;
    assert_eq!(session.subject, DEFAULT_CLI_SESSION_SUBJECT);
    assert_eq!(session.client.device_type, "bot");
    assert_eq!(session.client.label.as_deref(), Some("Runner Bot"));

    let verified = verify_bearer_session_with_conn(conn, &session.token, now_ms + 1_000)?;
    assert_eq!(verified.session_id, session.session_id);
    assert_eq!(verified.scopes, vec!["orchestration:read", "terminal:read"]);

    let active = list_active_sessions(conn, &format_epoch_millis_utc(now_ms + 1_000))?;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].session_id, session.session_id);

    assert!(revoke_session_at(
        conn,
        &session.session_id,
        now_ms + 2_000
    )?);
    assert!(!revoke_session_at(
        conn,
        &session.session_id,
        now_ms + 3_000
    )?);
    assert!(verify_bearer_session_with_conn(conn, &session.token, now_ms + 2_500).is_err());
    Ok(())
}

#[test]
fn connect_devices_handle_waiting_approve_revoke_and_duplicate_code_ambiguity()
-> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    legion_persistence::pairing_tokens::ensure_pairing_schema(conn)?;
    let now_ms = 1_700_000_000_000_i64;
    let created_iso = format_epoch_millis_utc(now_ms);
    let expires_iso = format_epoch_millis_utc(now_ms + 600_000);

    // Verification code uses only first 8 base64url chars -> matching prefixes collide cleanly.
    let key_a = "ABCDEFGH-device-one";
    let key_b = "ABCDEFGH-device-two";
    let code = connect_device_verification_code(key_a);
    assert_eq!(code, connect_device_verification_code(key_b));
    assert_eq!(
        normalize_verification_code(&code.to_ascii_lowercase().replace('-', " ")),
        code.replace('-', ""),
    );

    for (id, cred, key, meta) in [
        (
            "req-1",
            "C1",
            key_a,
            r#"{"label":"Phone A","deviceType":"mobile"}"#,
        ),
        (
            "req-2",
            "C2",
            key_b,
            r#"{"label":"Phone B","deviceType":"mobile"}"#,
        ),
    ] {
        conn.execute(
            "INSERT INTO auth_pairing_links (
               id, credential, method, scopes, subject, created_at, expires_at,
               proof_key_thumbprint, requires_connect_approval, requested_client_json
             ) VALUES (?1, ?2, 'one-time-token', '[\"orchestration:read\"]', 'cloud-connect', ?3, ?4, ?5, 1, ?6)",
            params![id, cred, created_iso, expires_iso, key, meta],
        )?;
    }

    let (waiting, trusted) = list_connect_devices_at(conn, now_ms)?;
    assert_eq!(waiting.len(), 2);
    assert_eq!(trusted, []);

    // Ambiguous waiting code MUST NOT approve either request.
    assert_eq!(
        approve_connect_device_by_code_at(conn, &code, now_ms + 100)?,
        ConnectApproveOutcome::Ambiguous { match_count: 2 },
    );
    assert_eq!(list_connect_devices_at(conn, now_ms + 100)?.1, []);

    // Revoking req-2 marks connect_denied and resolves ambiguity so req-1 approves cleanly.
    assert!(revoke_pairing_link_at(conn, "req-2", now_ms + 150)?);
    let denied: i64 = conn.query_row(
        "SELECT connect_denied FROM auth_pairing_links WHERE id = 'req-2'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(denied, 1);

    let approved =
        approve_connect_device_by_code_at(conn, &code.to_ascii_lowercase(), now_ms + 200)?;
    assert_eq!(
        approved,
        ConnectApproveOutcome::Approved {
            pairing_link_id: "req-1".to_owned(),
            proof_key_thumbprint: key_a.to_owned(),
            verification_code: code.clone(),
        },
    );
    // Once key_a is trusted, its unconsumed approval pairing link must not be revocable.
    assert!(!revoke_pairing_link_at(conn, "req-1", now_ms + 250)?);

    // Issuing & revoking a session bound to key_a cascades to auth_connect_devices and sibling links.
    conn.execute(
        "INSERT INTO auth_sessions (
           session_id, subject, scopes, method, client_device_type,
           issued_at, expires_at, proof_key_thumbprint
         ) VALUES ('sess-connect', 'cloud-connect', '[\"orchestration:read\"]', 'dpop-access-token', 'mobile', ?1, ?2, ?3)",
        params![created_iso, expires_iso, key_a],
    )?;
    assert!(revoke_session_at(conn, "sess-connect", now_ms + 300)?);
    assert_eq!(list_connect_devices_at(conn, now_ms + 300)?.1, []);

    // Ambiguous trusted devices also refuse single-code revocation; unique trusted device revokes cleanly.
    for key in [key_a, key_b] {
        conn.execute(
            "INSERT INTO auth_connect_devices (proof_key_thumbprint, approved_at) VALUES (?1, ?2)",
            params![key, created_iso],
        )?;
    }
    assert_eq!(
        revoke_connect_device_by_code_at(conn, &code, now_ms + 400)?,
        ConnectRevokeOutcome::Ambiguous { match_count: 2 },
    );

    let key_unique = "zzzzzzzz-unique";
    let code_unique = connect_device_verification_code(key_unique);
    conn.execute(
        "INSERT INTO auth_connect_devices (proof_key_thumbprint, approved_at) VALUES (?1, ?2)",
        params![key_unique, created_iso],
    )?;
    assert_eq!(
        revoke_connect_device_by_code_at(conn, &code_unique, now_ms + 500)?,
        ConnectRevokeOutcome::Revoked {
            proof_key_thumbprint: key_unique.to_owned(),
        },
    );
    Ok(())
}

#[test]
fn cli_auth_formatters_match_ts_output_byte_for_byte() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open_in_memory()?;
    let issued = issue_scoped_session_at(
        store.connection(),
        IssueSessionInput::default()
            .with_scopes(["orchestration:read"])
            .with_ttl_ms(60_000)
            .with_label("Build"),
        1_700_000_000_000,
    )?;

    assert_eq!(
        format_issued_session(&issued, false, true)?,
        format!("{}\n", issued.token),
    );
    let session_text = format_issued_session(&issued, false, false)?;
    assert_eq!(
        session_text,
        format!(
            "Issued bearer access token {}.\nScopes: orchestration:read\nToken: {}\nSubject: cli-issued-session\nClient: Build | bot\nExpires at: {}\n",
            issued.session_id, issued.token, issued.expires_at
        ),
    );
    let session_json: serde_json::Value =
        serde_json::from_str(&format_issued_session(&issued, true, false)?)?;
    assert_eq!(session_json["sessionId"], issued.session_id);
    assert_eq!(session_json["token"], issued.token);

    let link = PairingLinkRow {
        id: "link-1".to_owned(),
        credential: "ABCD2345EFGH".to_owned(),
        method: "one-time-token".to_owned(),
        scopes: vec!["orchestration:read".to_owned()],
        subject: "one-time-token".to_owned(),
        label: Some("Desk".to_owned()),
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
        expires_at: "2026-01-01T00:05:00.000Z".to_owned(),
        consumed_at: None,
        revoked_at: None,
        proof_key_thumbprint: None,
    };
    let text = format_issued_pairing_credential(&link, false, Some("https://host.example/foo"))?;
    assert_eq!(
        text,
        "Issued client pairing token link-1.\nToken: ABCD2345EFGH\nPair URL: https://host.example/pair#token=ABCD2345EFGH\nExpires at: 2026-01-01T00:05:00.000Z\n",
    );
    let empty_connect = format_connect_device_list(&[], &[], false)?;
    assert_eq!(
        empty_connect,
        "No devices are waiting for approval.\n\nNo trusted Legion Connect devices.\n",
    );
    let json_connect: serde_json::Value =
        serde_json::from_str(&format_connect_device_list(&[], &[], true)?)?;
    assert_eq!(
        json_connect,
        serde_json::json!({ "waiting": [], "trusted": [] })
    );
    Ok(())
}
