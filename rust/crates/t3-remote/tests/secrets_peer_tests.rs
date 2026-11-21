//! Integration tests for `FileSecretStore`, `SecretRef` TTL cache (`BUG-15`), `PeerLinksService`,
//! and `RemoteDelegationTracker` (`BUG-07` & `stop_count` fencing).

use std::cell::Cell;
use std::fs;

use t3_contracts::ids::{EnvironmentId, NodeId, RunId, ThreadId};
use t3_contracts::orchestration::McpClientAccess;
use t3_persistence::sqlite::SqliteStore;
use t3_remote::peer::delegation::{
    CallerCancelReason, FOLLOW_MAX_DEFECTS, FollowAction, FollowErrorKind, MAX_WAIT_MS,
    RemoteChildRef, RemoteDelegationTracker, RemoteStopCommand, RemoteTaskTerminalStatus,
    WAIT_SLICE_MS,
};
use t3_remote::peer::links::{
    InsertPeerLinkInput, PeerLinkErrorReason, PeerLinkStatus, PeerLinkSummary, PeerLinksService,
    format_peer_links, format_peer_links_json, format_peer_unlink_result, validate_peer_origin,
};
use t3_remote::secrets::{
    CachedSecretResolver, FileSecretStore, SecretRef, SecretRefError, SecretStoreError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn file_secret_store_atomic_create_and_zero_byte_repair() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = FileSecretStore::from_state_dir(dir.path())?;

    store.set_utf8("peer-1", "token-alpha")?;
    assert_eq!(store.get_utf8("peer-1")?.as_deref(), Some("token-alpha"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(store.directory())?.permissions().mode() & 0o777;
        let file_mode = fs::metadata(store.secret_path("peer-1")?)?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    assert!(matches!(
        store.create("peer-1", b"overwrite-attempt"),
        Err(SecretStoreError::AlreadyExists { .. })
    ));
    assert_eq!(store.get_utf8("peer-1")?.as_deref(), Some("token-alpha"));

    assert!(matches!(
        store.get("../escape"),
        Err(SecretStoreError::InvalidName { .. })
    ));

    // Leftover zero-byte secret files must be repaired cleanly by get_or_create_random.
    let zero_path = store.secret_path("session-key")?;
    fs::write(&zero_path, b"")?;
    let key = store.get_or_create_random("session-key", 32)?;
    assert_eq!(key.len(), 32);
    assert_eq!(store.get_or_create_random("session-key", 32)?, key);

    assert!(store.remove("peer-1")?);
    assert!(!store.remove("peer-1")?);
    assert!(!store.secret_path("peer-1")?.exists());
    Ok(())
}

#[test]
fn secret_ref_parsing_and_ttl_cache_never_caches_failures_bug15() -> TestResult {
    let parsed = SecretRef::parse("op://Private/T3 Token/credential?attribute=otp")?;
    assert_eq!(parsed.vault(), "Private");
    assert_eq!(parsed.item(), "T3 Token");
    assert_eq!(parsed.section(), None);
    assert_eq!(parsed.field(), "credential");

    let with_section = SecretRef::parse("op://Work/API/Prod/key")?;
    assert_eq!(with_section.section(), Some("Prod"));
    assert_eq!(with_section.field(), "key");

    assert!(matches!(
        SecretRef::parse("op://Private/Item/"),
        Err(SecretRefError::InvalidSyntax { .. })
    ));
    assert!(matches!(
        SecretRef::parse("https://example.com/secret"),
        Err(SecretRefError::InvalidSyntax { .. })
    ));

    // BUG-15: Err, Ok(None), and blank strings must never be memoized.
    let resolver = CachedSecretResolver::new(5_000);
    let calls = Cell::new(0usize);

    let err: Result<Option<String>, &str> =
        resolver.resolve("onepassword", &parsed, "cfg1", 1_000, false, |_| {
            calls.set(calls.get() + 1);
            Err("lock busy")
        });
    assert!(err.is_err());

    let none_res: Result<Option<String>, &str> =
        resolver.resolve("onepassword", &parsed, "cfg1", 1_100, false, |_| {
            calls.set(calls.get() + 1);
            Ok(None)
        });
    assert_eq!(none_res?, None);

    let blank_res: Result<Option<String>, &str> =
        resolver.resolve("onepassword", &parsed, "cfg1", 1_200, false, |_| {
            calls.set(calls.get() + 1);
            Ok(Some("  \n\t ".to_owned()))
        });
    assert_eq!(blank_res?, None);
    assert_eq!(calls.get(), 3);

    // Next real fetch populates the cache; subsequent call within TTL hits cache without calling fetcher.
    let hit1: Option<String> =
        resolver.resolve("onepassword", &parsed, "cfg1", 2_000, false, |_| {
            calls.set(calls.get() + 1);
            Ok::<_, &str>(Some("live-secret".to_owned()))
        })?;
    let hit2: Option<String> =
        resolver.resolve("onepassword", &parsed, "cfg1", 3_000, false, |_| {
            calls.set(calls.get() + 1);
            Ok::<_, &str>(Some("should-not-run".to_owned()))
        })?;
    assert_eq!(hit1.as_deref(), Some("live-secret"));
    assert_eq!(hit2.as_deref(), Some("live-secret"));
    assert_eq!(calls.get(), 4);

    // Config change and expiry both bypass stale cache.
    let _ = resolver.resolve("onepassword", &parsed, "cfg2", 3_500, false, |_| {
        calls.set(calls.get() + 1);
        Ok::<_, &str>(Some("reconfigured-secret".to_owned()))
    })?;
    assert_eq!(calls.get(), 5);
    Ok(())
}

#[test]
fn peer_links_url_guards_persistence_and_unlink_deletes_both() -> TestResult {
    assert_eq!(
        validate_peer_origin("http://127.0.0.1:3773/path?x=1")?,
        "http://127.0.0.1:3773"
    );
    assert_eq!(
        validate_peer_origin("http://box.tail.ts.net:3773")?,
        "http://box.tail.ts.net:3773"
    );
    assert_eq!(
        validate_peer_origin("http://[fd7a:115c:a1e0::1]:3773")?,
        "http://[fd7a:115c:a1e0::1]:3773"
    );
    for blocked in [
        "http://example.com:3773",
        "http://100.64.0.5:3773",
        "ws://localhost:3773",
    ] {
        assert_eq!(
            validate_peer_origin(blocked).map_err(|e| e.reason),
            Err(PeerLinkErrorReason::Unreachable)
        );
    }

    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let secrets = FileSecretStore::from_state_dir(dir.path())?;
    let service = PeerLinksService::new(secrets.clone());

    let inserted = service.insert(
        store.connection(),
        InsertPeerLinkInput {
            environment_id: EnvironmentId::new("env-work")?,
            label: "Workstation".to_owned(),
            urls: vec![
                "https://work.tail.ts.net/foo".to_owned(),
                "https://work.tail.ts.net/bar".to_owned(),
                "http://127.0.0.1:3773".to_owned(),
            ],
            access: McpClientAccess::AutoAcceptEdits,
            access_token: "bearer-123".to_owned(),
            linked_at: "2026-11-01T00:00:00.000Z".to_owned(),
            expires_at: "2026-12-01T00:00:00.000Z".to_owned(),
        },
    )?;
    assert_eq!(
        inserted.urls,
        vec!["https://work.tail.ts.net", "http://127.0.0.1:3773"]
    );

    let creds =
        service.resolve_credentials(store.connection(), "env-work", "2026-11-20T00:00:00.000Z")?;
    assert_eq!(creds.token, "bearer-123");

    let bin_files: Vec<_> = fs::read_dir(secrets.directory())?
        .filter_map(Result::ok)
        .collect();
    assert_eq!(bin_files.len(), 1);

    assert!(service.unlink(store.connection(), "env-work")?);
    assert_eq!(service.list(store.connection())?, []);
    assert_eq!(fs::read_dir(secrets.directory())?.count(), 0);
    Ok(())
}

#[test]
fn peer_links_cli_formatters_match_environment_ts() -> TestResult {
    assert_eq!(format_peer_links(&[]), "No linked environments.\n");
    assert_eq!(format_peer_links_json(&[])?, "[]\n");
    assert_eq!(
        format_peer_unlink_result("env-1", true),
        "Forgot env-1. Revoke it in that environment's Sharing & access to end its session there too.\n"
    );
    assert_eq!(
        format_peer_unlink_result("env-1", false),
        "No linked environment env-1.\n"
    );

    let summary = PeerLinkSummary {
        environment_id: EnvironmentId::new("env-1")?,
        label: "Box".to_owned(),
        urls: vec!["https://box.ts.net".to_owned()],
        access: McpClientAccess::Auto,
        linked_at: "2026-11-01T00:00:00.000Z".to_owned(),
        expires_at: "2026-12-01T00:00:00.000Z".to_owned(),
        last_reached_at: None,
        last_error: Some("timed out".to_owned()),
        status: PeerLinkStatus::Unreachable,
    };
    let text = format_peer_links(std::slice::from_ref(&summary));
    assert_eq!(
        text,
        "Box (env-1)\n  unreachable, access auto, expires 2026-12-01T00:00:00.000Z\n  https://box.ts.net\n  last error: timed out\n"
    );
    let json = format_peer_links_json(&[summary])?;
    assert!(json.ends_with('\n'));
    assert!(!json.trim_end().contains('\n'));
    assert!(json.contains("\"lastReachedAt\":null"));
    assert!(json.contains("\"lastError\":\"timed out\""));
    Ok(())
}

#[test]
fn remote_delegation_wait_slices_fencing_and_bug07() -> TestResult {
    assert_eq!(WAIT_SLICE_MS, 15_000);
    assert_eq!(MAX_WAIT_MS, 600_000);
    assert_eq!(FOLLOW_MAX_DEFECTS, 3);

    let child = RemoteChildRef {
        environment_id: EnvironmentId::new("env-box")?,
        thread_id: ThreadId::new("remote-th")?,
        label: "Box".to_owned(),
        title: None,
        run_id: Some(RunId::new("run-9")?),
        stopped_there: None,
        stop_count: None,
    };
    let mut tracker =
        RemoteDelegationTracker::new(NodeId::new("task-1")?, ThreadId::new("parent-1")?, child);

    let slice = tracker.record_wait_slice_timeout(590_000);
    assert_eq!(slice, FollowAction::WaitSlice { timeout_ms: 10_000 });

    // Record local result then cancel: increments stop_count and rejects stale acknowledgements.
    tracker.record_local_completion(RemoteTaskTerminalStatus::Completed, "ok".to_owned(), None);
    let stop1 = tracker.on_caller_cancel(CallerCancelReason::TaskCancelled {
        reason: Some("abort".to_owned()),
    });
    assert!(matches!(
        &stop1,
        RemoteStopCommand::StopThere {
            stop_count: Some(1),
            idempotency_key,
            ..
        } if idempotency_key == "stop-1:run-9:remote-th"
    ));
    assert!(!tracker.accept_stop_acknowledgement(Some(0)));
    assert!(tracker.accept_stop_acknowledgement(Some(1)));

    // BUG-07 fix: ParentDeleted + ThreadNotFound MUST still emit StopThere!
    let stop2 = tracker.on_caller_cancel(CallerCancelReason::ParentDeleted);
    assert!(matches!(
        stop2,
        RemoteStopCommand::StopThere {
            stop_count: Some(2),
            ..
        }
    ));
    let follow_err = tracker.on_follow_error(FollowErrorKind::ThreadNotFound, "thread not found");
    assert!(matches!(
        follow_err,
        FollowAction::SendStop(RemoteStopCommand::StopThere {
            stop_count: Some(2),
            ..
        })
    ));

    // Separate tracker checks defect threshold warning.
    let child2 = RemoteChildRef {
        environment_id: EnvironmentId::new("env-2")?,
        thread_id: ThreadId::new("r-2")?,
        label: "Lab".to_owned(),
        title: None,
        run_id: None,
        stopped_there: None,
        stop_count: None,
    };
    let mut defect_tracker =
        RemoteDelegationTracker::new(NodeId::new("t-2")?, ThreadId::new("p-2")?, child2);
    assert!(matches!(
        defect_tracker.on_follow_error(FollowErrorKind::Defect, "boom"),
        FollowAction::RetryWithBackoff { defect_count: 1 }
    ));
    assert!(matches!(
        defect_tracker.on_follow_error(FollowErrorKind::Defect, "boom"),
        FollowAction::RetryWithBackoff { defect_count: 2 }
    ));
    assert!(matches!(
        defect_tracker.on_follow_error(FollowErrorKind::Defect, "boom"),
        FollowAction::GiveUpWithWarning { .. }
    ));
    Ok(())
}
