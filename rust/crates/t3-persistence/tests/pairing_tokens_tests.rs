//! Integration tests for `t3_persistence::pairing_tokens`.

use std::error::Error;
use std::thread;

use rusqlite::params;
use t3_persistence::auth_records::{list_active_pairing_links, list_active_sessions};
use t3_persistence::ownership::format_epoch_millis_utc;
use t3_persistence::pairing_tokens::{
    ADMINISTRATIVE_CLIENT_SCOPES, DEFAULT_PAIRING_TOKEN_TTL_MS, DEFAULT_SESSION_TTL_MS,
    IssuePairingTokenInput, PAIRING_CODE_ALPHABET, PAIRING_CODE_LENGTH, PairingTokenError,
    exchange_pairing_token, issue_pairing_token, role_for_scopes, verify_bearer_session,
};
use t3_persistence::sqlite::SqliteStore;

#[test]
fn issue_and_exchange_pairing_token_roundtrip_and_sync_with_auth_records()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");
    let now_ms = 1_700_000_000_000_i64;

    assert!(matches!(
        issue_pairing_token(
            &db_path,
            &IssuePairingTokenInput::default().with_ttl_ms(0),
            now_ms,
        ),
        Err(PairingTokenError::InvalidTtl { ttl_ms: 0 })
    ));

    let issued = issue_pairing_token(
        &db_path,
        &IssuePairingTokenInput::default().with_label("Field Laptop"),
        now_ms,
    )?;
    assert_eq!(issued.credential.len(), PAIRING_CODE_LENGTH);
    assert!(
        issued
            .credential
            .bytes()
            .all(|byte| PAIRING_CODE_ALPHABET.as_bytes().contains(&byte))
    );
    assert_eq!(issued.expires_at_ms, now_ms + DEFAULT_PAIRING_TOKEN_TTL_MS,);

    // Visible in auth_records::list_active_pairing_links before consumption.
    let store = SqliteStore::open(&db_path)?;
    let before_links =
        list_active_pairing_links(store.connection(), &format_epoch_millis_utc(now_ms + 1_000))?;
    assert_eq!(before_links.len(), 1);
    assert_eq!(before_links[0].id, issued.id);
    store.close()?;

    // Grouped lowercase input (xxxx-xxxx-xxxx) matches canonical code.
    let grouped = format!(
        "{}-{}-{}",
        issued.credential[0..4].to_ascii_lowercase(),
        issued.credential[4..8].to_ascii_lowercase(),
        issued.credential[8..12].to_ascii_lowercase(),
    );
    let session = exchange_pairing_token(&db_path, &grouped, Some("My Phone"), now_ms + 2_000)?;
    assert_eq!(session.credential, issued.credential);
    assert_ne!(session.token, session.session_id);
    assert_eq!(session.role, "client");
    assert_eq!(
        session.expires_at_iso,
        format_epoch_millis_utc(now_ms + 2_000 + DEFAULT_SESSION_TTL_MS),
    );

    // Second use is atomically rejected as already consumed.
    assert!(matches!(
        exchange_pairing_token(&db_path, &issued.credential, None, now_ms + 3_000),
        Err(PairingTokenError::AlreadyConsumed)
    ));

    // Active session appears in auth_records::list_active_sessions and verifies cleanly.
    let store = SqliteStore::open(&db_path)?;
    let active =
        list_active_sessions(store.connection(), &format_epoch_millis_utc(now_ms + 3_000))?;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].session_id, session.session_id);
    assert_eq!(active[0].client.label.as_deref(), Some("My Phone"));
    store.close()?;

    let verified = verify_bearer_session(&db_path, &session.token, now_ms + 5_000)?;
    assert_eq!(verified.session_id, session.session_id);
    assert_eq!(verified.role, "client");
    assert_eq!(verified.client_label.as_deref(), Some("My Phone"));

    // Public session_id must NOT be accepted as a secret bearer token.
    assert!(matches!(
        verify_bearer_session(&db_path, &session.session_id, now_ms + 5_000),
        Err(PairingTokenError::UnknownToken)
    ));
    Ok(())
}

#[test]
fn pairing_exchange_rejects_expired_revoked_and_unknown_and_sets_admin_role()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");
    let now_ms = 1_700_000_000_000_i64;

    assert!(matches!(
        exchange_pairing_token(&db_path, "ZZZZ-ZZZZ-ZZZZ", None, now_ms),
        Err(PairingTokenError::UnknownToken)
    ));

    let short = issue_pairing_token(
        &db_path,
        &IssuePairingTokenInput {
            scopes: vec!["orchestration:read".to_owned()],
            ttl_ms: 10_000,
            label: None,
        },
        now_ms,
    )?;
    assert!(matches!(
        exchange_pairing_token(&db_path, &short.credential, None, now_ms + 10_000),
        Err(PairingTokenError::TokenExpired { .. })
    ));

    let revoked = issue_pairing_token(&db_path, &IssuePairingTokenInput::default(), now_ms)?;
    let store = SqliteStore::open(&db_path)?;
    store.connection().execute(
        "UPDATE auth_pairing_links SET revoked_at = ?1 WHERE id = ?2",
        params![format_epoch_millis_utc(now_ms + 100), revoked.id],
    )?;
    store.close()?;
    assert!(matches!(
        exchange_pairing_token(&db_path, &revoked.credential, None, now_ms + 200),
        Err(PairingTokenError::TokenRevoked)
    ));

    let admin_scopes: Vec<String> = ADMINISTRATIVE_CLIENT_SCOPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(role_for_scopes(&admin_scopes), "admin");
    let admin_link = issue_pairing_token(
        &db_path,
        &IssuePairingTokenInput::default().with_scopes(admin_scopes),
        now_ms,
    )?;
    let admin_session = exchange_pairing_token(&db_path, &admin_link.credential, None, now_ms)?;
    assert_eq!(admin_session.role, "admin");
    Ok(())
}

#[test]
fn verify_bearer_session_enforces_bug04_revocation_and_expiration_boundaries()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let missing_db = dir.path().join("missing.sqlite");
    assert!(matches!(
        verify_bearer_session(&missing_db, "t3s_any", 1_700_000_000_000),
        Err(PairingTokenError::UnknownToken)
    ));
    assert!(!missing_db.exists());

    let db_path = dir.path().join("statev2.sqlite");
    let now_ms = 1_700_000_000_000_i64;
    let issued = issue_pairing_token(&db_path, &IssuePairingTokenInput::default(), now_ms)?;
    let session = exchange_pairing_token(&db_path, &issued.credential, None, now_ms)?;

    let verified = verify_bearer_session(&db_path, &session.token, now_ms)?;
    let exact_exp_ms = verified.expires_at_ms;
    assert_eq!(exact_exp_ms, now_ms + DEFAULT_SESSION_TTL_MS);

    // BUG-04 boundary: exact expiration timestamp and beyond must fail.
    assert!(matches!(
        verify_bearer_session(&db_path, &session.token, exact_exp_ms),
        Err(PairingTokenError::SessionExpired { .. })
    ));
    assert!(matches!(
        verify_bearer_session(&db_path, &session.token, exact_exp_ms + 1),
        Err(PairingTokenError::SessionExpired { .. })
    ));

    // BUG-04: DB revocation rejects even when token expires in the future.
    let store = SqliteStore::open(&db_path)?;
    store.connection().execute(
        "UPDATE auth_sessions SET revoked_at = ?1 WHERE session_id = ?2",
        params![format_epoch_millis_utc(now_ms + 500), session.session_id],
    )?;
    store.close()?;
    assert!(matches!(
        verify_bearer_session(&db_path, &session.token, now_ms + 1_000),
        Err(PairingTokenError::SessionRevoked { .. })
    ));
    Ok(())
}

#[test]
fn concurrent_exchange_allows_only_one_winner() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");
    let now_ms = 1_700_000_000_000_i64;
    let issued = issue_pairing_token(&db_path, &IssuePairingTokenInput::default(), now_ms)?;

    let outcomes = thread::scope(|scope| {
        let h1 = scope
            .spawn(|| exchange_pairing_token(&db_path, &issued.credential, Some("A"), now_ms + 10));
        let h2 = scope
            .spawn(|| exchange_pairing_token(&db_path, &issued.credential, Some("B"), now_ms + 10));
        (h1.join(), h2.join())
    });

    let r1 = outcomes.0.map_err(|_| "thread 1 panicked")?;
    let r2 = outcomes.1.map_err(|_| "thread 2 panicked")?;
    let successes = [r1.is_ok(), r2.is_ok()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1);
    let ((Err(failure), Ok(_)) | (Ok(_), Err(failure))) = (r1, r2) else {
        return Err("expected exactly one success and one failure".into());
    };
    assert!(matches!(failure, PairingTokenError::AlreadyConsumed));
    Ok(())
}
