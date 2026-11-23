//! Integration tests for `legion_persistence::service_launcher`.

use std::cmp::Ordering;
use std::error::Error;
use std::fs;
use std::time::SystemTime;

use legion_persistence::ownership::{
    PersistedServerRuntimeState, ServerOwnerLock, format_iso8601_utc, persist_server_runtime_state,
};
use legion_persistence::paths::ServerDerivedPaths;
use legion_persistence::service_launcher::{
    DESKTOP_UPDATE_IN_PROGRESS_TTL_MS, LauncherChildRole, LauncherOptions, LauncherStartupDecision,
    PINNED_RUNTIME_PRUNE_GRACE_MS, ServiceLockGuard, ServiceState, ServiceUpdateRecord,
    compare_exact_service_versions, decode_service_launcher_context, is_exact_service_version,
    parse_legacy_service_state, parse_service_launcher_args, parse_service_state,
    plan_launcher_startup, prune_pinned_runtimes_at, publish_update_marker, read_service_state,
    remaining_update_wait_ms, runtime_entry_paths, service_launcher_is_running,
    service_launcher_is_serving, service_launcher_lock_path, service_serving_lock_path,
    service_state_path, state_dir_owned, write_service_state,
};
use legion_persistence::sqlite::SqliteStore;

#[test]
fn parses_launcher_args_matching_cli_contract() {
    let empty: Vec<String> = Vec::new();
    assert_eq!(
        parse_service_launcher_args(&empty),
        Some(LauncherOptions::default()),
    );

    let full = vec![
        "--base-dir".to_owned(),
        "/var/legioncode".to_owned(),
        "--log-file".to_owned(),
        "/var/legioncode/service.log".to_owned(),
        "--service-unit".to_owned(),
        "legioncode.service".to_owned(),
    ];
    assert_eq!(
        parse_service_launcher_args(&full),
        Some(LauncherOptions {
            base_dir: Some("/var/legioncode".to_owned()),
            log_file: Some("/var/legioncode/service.log".to_owned()),
            service_unit: Some("legioncode.service".to_owned()),
        }),
    );

    assert_eq!(
        parse_service_launcher_args(&["--base-dir".to_owned()]),
        None,
    );
    assert_eq!(
        parse_service_launcher_args(&["--unknown".to_owned(), "x".to_owned()]),
        None,
    );
}

#[test]
fn launcher_locks_and_state_dir_ownership_probes_detect_active_holders()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let base_dir = dir.path();

    assert!(!service_launcher_is_running(&service_launcher_lock_path(
        base_dir
    )));
    assert!(!service_launcher_is_serving(base_dir));
    assert!(!state_dir_owned(base_dir));

    let launcher_guard = ServiceLockGuard::try_acquire(&service_launcher_lock_path(base_dir), 0)?
        .ok_or("expected free launcher lock")?;
    assert!(service_launcher_is_running(launcher_guard.lock_path()));
    drop(launcher_guard);
    assert!(!service_launcher_is_running(&service_launcher_lock_path(
        base_dir
    )));

    let serving_guard = ServiceLockGuard::try_acquire(&service_serving_lock_path(base_dir), 0)?
        .ok_or("expected free serving lock")?;
    assert!(service_launcher_is_serving(base_dir));
    drop(serving_guard);
    assert!(!service_launcher_is_serving(base_dir));

    let derived = ServerDerivedPaths::derive(base_dir, false, true);
    let owner_lock = ServerOwnerLock::acquire(&derived.state_dir)?;
    assert!(state_dir_owned(base_dir));
    owner_lock.release()?;
    assert!(!state_dir_owned(base_dir));

    // Pre-lock legacy runtime JSON with current live PID owns the dir; ownerId without lock does not.
    let runtime_state_path = derived.state_dir.join("server-runtime.json");
    let mut legacy = PersistedServerRuntimeState::for_current_process(3773, Some("127.0.0.1"));
    legacy.started_at = format_iso8601_utc(SystemTime::now());
    persist_server_runtime_state(&runtime_state_path, &legacy)?;
    assert!(state_dir_owned(base_dir));

    legacy.owner_id = Some("stale-owner".to_owned());
    persist_server_runtime_state(&runtime_state_path, &legacy)?;
    assert!(!state_dir_owned(base_dir));
    Ok(())
}

#[test]
fn desktop_update_marker_publishes_atomically_and_bounds_remaining_wait()
-> Result<(), Box<dyn Error>> {
    assert_eq!(DESKTOP_UPDATE_IN_PROGRESS_TTL_MS, 60_000);
    let dir = tempfile::tempdir()?;
    let marker_file = dir.path().join("runtime").join("update-in-progress.json");
    assert_eq!(remaining_update_wait_ms(&marker_file, 10_000), None);

    let marker = publish_update_marker(&marker_file, 1_000_000)?;
    assert_eq!(marker.started_at, 1_000_000);
    assert_eq!(marker.expires_at, 1_060_000);

    assert_eq!(remaining_update_wait_ms(&marker_file, 999_999), None);
    assert_eq!(
        remaining_update_wait_ms(&marker_file, 1_000_000),
        Some(60_000)
    );
    assert_eq!(
        remaining_update_wait_ms(&marker_file, 1_025_000),
        Some(35_000)
    );
    assert_eq!(remaining_update_wait_ms(&marker_file, 1_060_000), None);

    // Oversized window is capped at DESKTOP_UPDATE_IN_PROGRESS_TTL_MS.
    fs::write(&marker_file, r#"{"startedAt": 1000, "expiresAt": 500000}"#)?;
    assert_eq!(remaining_update_wait_ms(&marker_file, 2_000), Some(60_000));

    // Inverted or malformed marker returns None.
    fs::write(&marker_file, r#"{"startedAt": 5000, "expiresAt": 4000}"#)?;
    assert_eq!(remaining_update_wait_ms(&marker_file, 4_500), None);
    fs::write(&marker_file, "not-json")?;
    assert_eq!(remaining_update_wait_ms(&marker_file, 4_500), None);
    Ok(())
}

#[test]
fn service_state_codec_validates_semver_protocols_and_updates() -> Result<(), Box<dyn Error>> {
    assert!(is_exact_service_version("1.2.3-alpha.1+build.9"));
    assert!(!is_exact_service_version("01.2.3"));
    assert!(!is_exact_service_version("v1.2.3"));

    assert_eq!(
        compare_exact_service_versions("1.0.0-alpha.1", "1.0.0-1"),
        Ordering::Greater,
    );
    assert_eq!(
        compare_exact_service_versions("1.0.0-beta", "1.0.0"),
        Ordering::Less,
    );
    assert_eq!(
        compare_exact_service_versions("1.2.3+a", "1.2.3+b"),
        Ordering::Equal,
    );

    let legacy = parse_legacy_service_state(r#"{"protocol":2,"activeVersion":"0.9.0"}"#)
        .ok_or("expected legacy service state")?;
    assert_eq!(legacy.protocol, 4);
    assert_eq!(legacy.active_version, "0.9.0");
    assert_eq!(
        parse_legacy_service_state(r#"{"protocol":2,"activeVersion":"0.9.0","extra":1}"#),
        None,
    );

    let proto3_json = r#"{
      "protocol": 3,
      "activeVersion": "1.0.0",
      "update": {
        "id": "upd-1",
        "fromVersion": "1.0.0",
        "targetVersion": "1.1.0",
        "dbPath": "/tmp/statev2.sqlite",
        "status": "pending"
      }
    }"#;
    let parsed = parse_service_state(proto3_json).ok_or("expected protocol 3 adoption")?;
    assert_eq!(parsed.protocol, 4);

    let dir = tempfile::tempdir()?;
    let state_file = service_state_path(dir.path());
    write_service_state(&state_file, &parsed)?;
    assert_eq!(read_service_state(&state_file)?, parsed);

    // Regression: targetVersion <= fromVersion must be rejected.
    let bad = r#"{"protocol":4,"activeVersion":"1.1.0","update":{"id":"u","fromVersion":"1.1.0","targetVersion":"1.0.0","dbPath":"/tmp/d","status":"pending"}}"#;
    assert_eq!(parse_service_state(bad), None);
    Ok(())
}

#[test]
fn launcher_startup_plan_covers_active_trial_rollback_and_preflight_block()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let base_dir = dir.path();

    for version in ["1.0.0", "1.1.0"] {
        let (bin, sentinel) = runtime_entry_paths(base_dir, version);
        if let Some(parent) = bin.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&bin, b"binary")?;
        fs::write(&sentinel, format!("{version}\n"))?;
    }

    let active_state = ServiceState {
        protocol: 4,
        active_version: "1.0.0".to_owned(),
        update: None,
    };
    let decision = plan_launcher_startup(base_dir, &active_state)?;
    let LauncherStartupDecision::Launch(plan) = decision else {
        return Err("expected Launch for active runtime".into());
    };
    assert_eq!(plan.role, LauncherChildRole::Active);
    assert_eq!(plan.version, "1.0.0");
    assert!(decode_service_launcher_context(&plan.context_json).is_some());

    let trial_db = base_dir.join("userdata").join("statev2.sqlite");
    let pending_state = ServiceState {
        protocol: 4,
        active_version: "1.0.0".to_owned(),
        update: Some(ServiceUpdateRecord::Pending {
            id: "upd-42".to_owned(),
            from_version: "1.0.0".to_owned(),
            target_version: "1.1.0".to_owned(),
            db_path: trial_db.to_string_lossy().into_owned(),
        }),
    };
    let LauncherStartupDecision::Launch(trial_plan) =
        plan_launcher_startup(base_dir, &pending_state)?
    else {
        return Err("expected Launch for trial update".into());
    };
    assert_eq!(trial_plan.role, LauncherChildRole::Trial);
    assert_eq!(trial_plan.version, "1.1.0");
    assert!(decode_service_launcher_context(&trial_plan.context_json).is_some());

    // Simulate future migration (> BUILD_MIGRATION_HEAD) blocking launch via service_preflight.
    let store = SqliteStore::open(&trial_db)?;
    store.connection().execute(
        "INSERT INTO effect_sql_migrations (migration_id, name) VALUES (?1, 'Future')",
        [legion_persistence::sqlite::BUILD_MIGRATION_HEAD + 1],
    )?;
    store.close()?;
    assert!(matches!(
        plan_launcher_startup(base_dir, &pending_state)?,
        LauncherStartupDecision::Blocked { version, .. } if version == "1.1.0"
    ));

    // Simulate interrupted database restore forcing rollback.
    let restore_marker = base_dir
        .join("runtime")
        .join("db-backup")
        .join("upd-42")
        .join(".restore-pending");
    if let Some(parent) = restore_marker.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&restore_marker, b"")?;
    assert!(matches!(
        plan_launcher_startup(base_dir, &pending_state)?,
        LauncherStartupDecision::RollbackRequired {
            reason: "rollback-interrupted",
            ..
        }
    ));
    Ok(())
}

#[test]
fn prune_pinned_runtimes_removes_committed_from_version_and_abandoned_staging()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let base_dir = dir.path();
    let versions_dir = base_dir.join("runtime").join("versions");

    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        let (entry, sentinel) = runtime_entry_paths(base_dir, version);
        if let Some(parent) = entry.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&entry, b"bin")?;
        fs::write(&sentinel, version)?;
    }

    let staging_dir = versions_dir.join(".staging-abandoned");
    fs::create_dir_all(&staging_dir)?;

    let committed = ServiceState {
        protocol: 4,
        active_version: "1.2.0".to_owned(),
        update: Some(ServiceUpdateRecord::Committed {
            id: "upd-1".to_owned(),
            from_version: "1.1.0".to_owned(),
            target_version: "1.2.0".to_owned(),
            reason: None,
        }),
    };
    write_service_state(&service_state_path(base_dir), &committed)?;

    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_millis();

    // Within the 1-hour grace window, nothing completed or staged is pruned.
    let early = prune_pinned_runtimes_at(base_dir, &["1.0.0"], &[], now_ms, Some(""));
    assert_eq!(early, Vec::<String>::new());
    assert!(staging_dir.exists());
    assert!(versions_dir.join("1.1.0").exists());

    // Past the 1-hour grace window, committed `from_version` (1.1.0) and abandoned staging are removed.
    let past_grace_ms = now_ms + PINNED_RUNTIME_PRUNE_GRACE_MS + 60_000;
    let removed = prune_pinned_runtimes_at(base_dir, &["1.0.0"], &[], past_grace_ms, Some(""));
    assert_eq!(removed, vec!["1.1.0".to_owned()]);
    assert!(!versions_dir.join("1.1.0").exists());
    assert!(!staging_dir.exists());
    assert!(versions_dir.join("1.0.0").exists());
    assert!(versions_dir.join("1.2.0").exists());

    // A rolled-back update retains `from_version` so rollback fallback stays runnable.
    let rollback_staging = versions_dir.join(".staging-keep");
    fs::create_dir_all(&rollback_staging)?;
    let (v110_entry, v110_sentinel) = runtime_entry_paths(base_dir, "1.1.0");
    if let Some(parent) = v110_entry.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&v110_entry, b"bin")?;
    fs::write(&v110_sentinel, "1.1.0")?;
    let rolled_back = ServiceState {
        protocol: 4,
        active_version: "1.2.0".to_owned(),
        update: Some(ServiceUpdateRecord::RolledBack {
            id: "upd-2".to_owned(),
            from_version: "1.1.0".to_owned(),
            target_version: "1.3.0".to_owned(),
            reason: None,
        }),
    };
    write_service_state(&service_state_path(base_dir), &rolled_back)?;
    let kept =
        prune_pinned_runtimes_at(base_dir, &["1.0.0"], &[], past_grace_ms + 60_000, Some(""));
    assert_eq!(kept, Vec::<String>::new());
    assert!(versions_dir.join("1.1.0").exists());
    Ok(())
}
