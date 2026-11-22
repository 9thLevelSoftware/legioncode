use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use legion_contracts::permissions::EnvironmentScope;
use legion_protocol::auth::{
    ConnectionInvalidationReason, ConnectionState, CredentialPrecedence, CredentialSelectionError,
    CredentialSource, DpopBindingError, NewSessionLease, RenewDpopSessionInput, RequestCredentials,
    SessionLeaseRegistry, SessionRenewalError, SessionVerificationError, TtlSecretCache,
    extract_cookie_value, verify_dpop_credential_binding,
};
use legion_shared::dpop::{DpopKeyPair, SignDpopProofInput, sign_dpop_proof};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn auth_header_precedence_blocks_cookie_fallback_bug_03() -> TestResult {
    let bearer_wins = CredentialPrecedence::select(&RequestCredentials {
        authorization: Some("Bearer header-secret"),
        primary_cookie: Some("cookie-primary"),
        legacy_cookie: Some("cookie-legacy"),
        dev_cookie: Some("cookie-dev"),
    })?;
    assert_eq!(bearer_wins.source, CredentialSource::AuthorizationBearer);
    assert_eq!(bearer_wins.token, "header-secret");

    let dpop_wins = CredentialPrecedence::select(&RequestCredentials {
        authorization: Some("dpop   dpop-secret "),
        primary_cookie: Some("cookie-primary"),
        ..Default::default()
    })?;
    assert_eq!(dpop_wins.source, CredentialSource::AuthorizationDpop);
    assert_eq!(dpop_wins.token, "dpop-secret");

    // Any present invalid Authorization header must reject instead of falling back to cookies.
    for bad_auth in [
        "",
        "   ",
        "Bearer   ",
        "DPoP\t",
        "Basic dXNlcjpwYXNz",
        "Bearer a b",
    ] {
        let err = CredentialPrecedence::select(&RequestCredentials {
            authorization: Some(bad_auth),
            primary_cookie: Some("valid-cookie"),
            legacy_cookie: Some("valid-legacy"),
            dev_cookie: Some("valid-dev"),
        });
        assert!(
            err.is_err(),
            "expected failure for auth header {bad_auth:?}"
        );
    }

    // Absent Authorization header falls through cookies in priority order.
    let from_legacy = CredentialPrecedence::select(&RequestCredentials {
        authorization: None,
        primary_cookie: Some("  "),
        legacy_cookie: Some("legacy-tok"),
        dev_cookie: Some("dev-tok"),
    })?;
    assert_eq!(from_legacy.source, CredentialSource::LegacyCookie);
    assert_eq!(from_legacy.token, "legacy-tok");

    assert_eq!(
        extract_cookie_value(
            "foo=bar; legioncode_session=abc123; empty=  ",
            "legioncode_session"
        )
        .as_deref(),
        Some("abc123")
    );
    assert_eq!(
        CredentialPrecedence::select(&RequestCredentials::default()),
        Err(CredentialSelectionError::MissingCredential)
    );

    // Verify DPoP binding policy enforces scheme parity.
    let kp = DpopKeyPair::generate();
    let proof = sign_dpop_proof(
        &kp,
        &SignDpopProofInput {
            method: "GET",
            url: "https://legion.local/ws?q=1",
            iat_epoch_seconds: 1_700_000_000,
            jti: Some("jti-1"),
            access_token: Some("dpop-secret"),
        },
    )?;
    let claims = verify_dpop_credential_binding(
        &dpop_wins,
        Some(&kp.thumbprint()),
        Some(&proof),
        "GET",
        "https://legion.local/ws",
        1_700_000_005,
        None,
    )?;
    assert_eq!(claims.map(|c| c.thumbprint), Some(kp.thumbprint()));

    assert!(matches!(
        verify_dpop_credential_binding(
            &bearer_wins,
            Some(&kp.thumbprint()),
            Some(&proof),
            "GET",
            "https://legion.local/ws",
            1_700_000_005,
            None,
        ),
        Err(DpopBindingError::DpopSchemeRequired)
    ));
    assert!(matches!(
        verify_dpop_credential_binding(
            &dpop_wins,
            None,
            Some(&proof),
            "GET",
            "https://legion.local/ws",
            1_700_000_005,
            None,
        ),
        Err(DpopBindingError::UnexpectedDpopForUnboundSession)
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn session_lease_expiry_and_successor_lineage_bug_04() -> TestResult {
    let registry = SessionLeaseRegistry::new();
    let kp = DpopKeyPair::generate();
    let thumbprint = kp.thumbprint();

    registry.register(NewSessionLease {
        session_id: "s1".to_owned(),
        subject: "alice".to_owned(),
        scopes: vec![
            EnvironmentScope::OrchestrationRead,
            EnvironmentScope::OrchestrationOperate,
        ],
        proof_key_thumbprint: Some(thumbprint.clone()),
        renewable: true,
        issued_at_ms: 1_000,
        db_expires_at_ms: 10_000,
        label: Some("desktop".to_owned()),
    })?;

    // BUG-04: expired DB lease rejects even when token `exp` is further in the future.
    assert_eq!(
        registry.verify_session("s1", 20_000, 10_000),
        Err(SessionVerificationError::DatabaseLeaseExpired {
            expired_at_ms: 10_000
        })
    );
    assert!(registry.verify_session("s1", 20_000, 9_999).is_ok());

    let migrated_conn = registry.connect("s1", 5_000)?;
    let stray_old_conn = registry.connect("s1", 5_000)?;

    // Wrong thumbprint cannot renew.
    assert!(matches!(
        registry.renew_dpop_session(&RenewDpopSessionInput {
            session_id: "s1".to_owned(),
            presented_thumbprint: "wrong-thumb".to_owned(),
            new_session_id: Some("s2".to_owned()),
            new_expires_at_ms: 30_000,
            now_ms: 6_000,
        }),
        Err(SessionRenewalError::ThumbprintMismatch { .. })
    ));

    // Rotate s1 -> s2; migrated connections follow s2.
    let s2 = registry.renew_dpop_session(&RenewDpopSessionInput {
        session_id: "s1".to_owned(),
        presented_thumbprint: thumbprint.clone(),
        new_session_id: Some("s2".to_owned()),
        new_expires_at_ms: 30_000,
        now_ms: 6_000,
    })?;
    assert_eq!(s2.predecessor_id.as_deref(), Some("s1"));
    assert_eq!(
        registry.check_connection(migrated_conn, 15_000),
        ConnectionState::Active {
            session_id: "s2".to_owned(),
            expires_at_ms: 30_000
        }
    );

    // Disconnecting a separate socket does not drop migrated_conn, and s1 shows disconnected.
    assert!(registry.disconnect(stray_old_conn));
    let active = registry.list_active_sessions(12_000);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].lease.session_id, "s2");
    assert!(active[0].connected);

    // Rotate s2 -> s3 then revoke ancestor s1: invalidates socket on s3 immediately.
    registry.renew_dpop_session(&RenewDpopSessionInput {
        session_id: "s2".to_owned(),
        presented_thumbprint: thumbprint,
        new_session_id: Some("s3".to_owned()),
        new_expires_at_ms: 50_000,
        now_ms: 15_000,
    })?;

    let reg_clone = registry.clone();
    let waiter =
        tokio::spawn(async move { reg_clone.await_invalidation(migrated_conn, || 16_000).await });
    tokio::task::yield_now().await;
    assert_eq!(registry.revoke_session("s1", 16_000)?, 1);
    let reason = waiter.await?;
    assert!(matches!(
        reason,
        ConnectionInvalidationReason::Revoked { .. }
    ));
    assert!(matches!(
        registry.verify_session("s3", 60_000, 16_001),
        Err(SessionVerificationError::Revoked { .. })
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn await_invalidation_wakes_on_db_lease_expiry_bug_04() -> TestResult {
    let registry = SessionLeaseRegistry::new();
    registry.register(NewSessionLease {
        session_id: "short".to_owned(),
        subject: "bob".to_owned(),
        scopes: vec![EnvironmentScope::OrchestrationRead],
        proof_key_thumbprint: None,
        renewable: false,
        issued_at_ms: 0,
        db_expires_at_ms: 500,
        label: None,
    })?;
    let start = tokio::time::Instant::now();
    let timed_conn = registry.connect("short", 0)?;
    let reg_timed = registry.clone();
    let expired_wait = tokio::spawn(async move {
        reg_timed
            .await_invalidation(timed_conn, move || {
                i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX)
            })
            .await
    });
    tokio::time::advance(std::time::Duration::from_millis(600)).await;
    let exp_reason = expired_wait.await?;
    assert_eq!(
        exp_reason,
        ConnectionInvalidationReason::Expired {
            session_id: "short".to_owned(),
            expired_at_ms: 500
        }
    );
    Ok(())
}

#[tokio::test]
async fn ttl_secret_cache_never_memoizes_errors_or_empty_bug_15() -> TestResult {
    let cache = TtlSecretCache::new(10_000);
    let calls = Arc::new(AtomicUsize::new(0));

    // Err is never cached.
    let err_res: Result<Option<String>, &str> = cache
        .resolve_with("1p", "ref/a", "cfg-1", 1_000, false, || {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err("locked")
            }
        })
        .await;
    assert_eq!(err_res, Err("locked"));

    // Ok(None) and blank values are never cached.
    let blank_res: Result<Option<String>, &str> = cache
        .resolve_with("1p", "ref/a", "cfg-1", 1_100, false, || {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Some("   \t ".to_owned()))
            }
        })
        .await;
    assert_eq!(blank_res, Ok(None));
    assert!(cache.is_empty());

    // Valid secret is cached and served without calling fetcher until TTL expires or config changes.
    let ok_res: Result<Option<String>, &str> = cache
        .resolve_with("1p", "ref/a", "cfg-1", 2_000, false, || {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Some("sk-live-1".to_owned()))
            }
        })
        .await;
    assert_eq!(ok_res?.as_deref(), Some("sk-live-1"));
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let cached_hit: Result<Option<String>, &str> = cache
        .resolve_with("1p", "ref/a", "cfg-1", 11_999, false, || async {
            Err("must not run on hit")
        })
        .await;
    assert_eq!(cached_hit?.as_deref(), Some("sk-live-1"));

    // `fresh = true` failure preserves prior valid value while surfacing the failure.
    let fresh_fail: Result<Option<String>, &str> = cache
        .resolve_with("1p", "ref/a", "cfg-1", 5_000, true, || async {
            Err("transient")
        })
        .await;
    assert_eq!(fresh_fail, Err("transient"));
    assert_eq!(
        cache.get_if_valid("1p", "ref/a", "cfg-1", 5_500).as_deref(),
        Some("sk-live-1")
    );

    // Config change and TTL expiration both miss cache.
    assert!(cache.get_if_valid("1p", "ref/a", "cfg-2", 5_500).is_none());
    assert!(cache.get_if_valid("1p", "ref/a", "cfg-1", 12_000).is_none());
    Ok(())
}
