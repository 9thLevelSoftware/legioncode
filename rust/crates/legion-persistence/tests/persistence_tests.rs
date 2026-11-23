//! Integration tests for `legion-persistence` on isolated temporary directories.

use std::fs;
use std::sync::{Arc, Barrier};

use legion_persistence::ownership::{
    OwnershipError, PersistedServerRuntimeState, SERVER_EXIT_CODE_STATE_DIR_OWNED, ServerOwnerLock,
    is_process_alive, persist_server_runtime_state, read_live_server_runtime_state,
    read_persisted_server_runtime_state,
};
use legion_persistence::paths::{
    ServerDerivedPaths, ensure_environment_id, ensure_server_directories,
};
use legion_persistence::sqlite::{
    BUILD_MIGRATION_HEAD, SqliteStore, SqliteStoreError, WAL_SIZE_LIMIT_BYTES,
    is_damaged_sqlite_code, read_migration_head,
};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn derives_paths_and_initializes_environment_id_atomically() -> TestResult {
    let temp = tempfile::tempdir()?;
    let prod = ServerDerivedPaths::derive(temp.path(), false, false);
    assert_eq!(prod.state_dir, temp.path().join("userdata"));
    assert_eq!(prod.db_path, prod.state_dir.join("statev2.sqlite"));
    assert_eq!(
        prod.worktree_checkout_cache_dir,
        temp.path().join("caches").join("worktree-checkouts")
    );

    let dev = ServerDerivedPaths::derive(temp.path(), true, false);
    assert_eq!(dev.state_dir, temp.path().join("dev"));
    let explicit_dev = ServerDerivedPaths::derive(temp.path(), true, true);
    assert_eq!(explicit_dev.state_dir, temp.path().join("userdata"));

    ensure_server_directories(&prod)?;
    assert!(prod.state_dir.is_dir());
    assert!(prod.provider_logs_dir.is_dir());
    assert!(prod.terminal_logs_dir.is_dir());
    assert!(prod.attachments_dir.is_dir());
    assert!(prod.worktrees_dir.is_dir());

    // Racing initializers must converge on the exact same environment ID.
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let b = Arc::clone(&barrier);
        let id_path = prod.environment_id_path.clone();
        handles.push(std::thread::spawn(move || {
            b.wait();
            ensure_environment_id(&id_path)
        }));
    }
    let mut ids = Vec::new();
    for handle in handles {
        let id = handle.join().map_err(|_| "thread panicked")??;
        ids.push(id);
    }
    assert!(ids.windows(2).all(|pair| pair[0] == pair[1]));

    // Empty pre-existing environment-id must recover via `.recovery`.
    let recovery_dir = temp.path().join("recover");
    fs::create_dir_all(&recovery_dir)?;
    let empty_id_path = recovery_dir.join("environment-id");
    fs::write(&empty_id_path, "   \n")?;
    let recovered = ensure_environment_id(&empty_id_path)?;
    assert_ne!(recovered, "");
    assert_eq!(ensure_environment_id(&empty_id_path)?, recovered);
    assert!(recovery_dir.join("environment-id.recovery").exists());
    Ok(())
}

#[test]
fn sqlite_store_applies_wal_pragmas_ddl_and_fts5() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("userdata").join("statev2.sqlite");

    assert_eq!(read_migration_head(&db_path), Some(BUILD_MIGRATION_HEAD));
    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();

    assert_eq!(
        conn.query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))?,
        5000
    );
    assert_eq!(
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))?,
        1
    );
    assert_eq!(
        conn.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0))?,
        2
    );
    assert_eq!(
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))?
            .to_ascii_lowercase(),
        "wal"
    );
    assert_eq!(
        conn.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))?,
        1
    );
    assert_eq!(
        conn.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))?,
        2
    );
    assert_eq!(
        conn.query_row("PRAGMA journal_size_limit", [], |r| r.get::<_, i64>(0))?,
        WAL_SIZE_LIMIT_BYTES
    );

    for table in [
        "effect_sql_migrations",
        "projection_projects",
        "projection_threads",
        "orchestration_events",
        "orchestration_command_receipts",
        "orchestration_v2_projection_threads",
        "orchestration_v2_projection_runs",
        "orchestration_v2_effect_outbox",
        "orchestration_v2_message_search",
    ] {
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
            [table],
            |r| r.get(0),
        )?;
        assert_eq!(found, 1, "missing core table {table}");
    }

    // Trigger populates FTS5 trigram index and normalizes curly apostrophes.
    conn.execute(
        "INSERT INTO orchestration_v2_projection_messages (
           message_id, thread_id, run_id, node_id, role, streaming, created_at, updated_at, payload_json
         ) VALUES ('m1', 't1', NULL, NULL, 'assistant', 0, '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z', '{\"text\":\"it’s searchable\"}')",
        [],
    )?;
    let hit: String = conn.query_row(
        "SELECT m.message_id FROM orchestration_v2_message_search s
         JOIN orchestration_v2_projection_messages m ON m.rowid = s.rowid
         WHERE orchestration_v2_message_search MATCH '\"it''s\"'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(hit, "m1");

    conn.execute(
        "INSERT INTO effect_sql_migrations (migration_id, name) VALUES (140, 'Future')",
        [],
    )?;
    store.close()?;
    assert_eq!(read_migration_head(&db_path), Some(140));
    Ok(())
}

#[test]
fn sqlite_store_preserves_existing_auto_vacuum_drops_stats_and_detects_damage() -> TestResult {
    let temp = tempfile::tempdir()?;
    let legacy_db = temp.path().join("existing.sqlite");
    let raw = Connection::open(&legacy_db)?;
    raw.execute_batch(
        "PRAGMA auto_vacuum = NONE;
         CREATE TABLE probe (id TEXT PRIMARY KEY);
         INSERT INTO probe VALUES ('a');
         ANALYZE probe;",
    )?;
    raw.close().map_err(|(_, e)| e)?;

    let reopened = SqliteStore::open(&legacy_db)?;
    assert_eq!(
        reopened
            .connection()
            .query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0))?,
        0
    );
    let stat_count: i64 =
        reopened
            .connection()
            .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |r| r.get(0))?;
    assert_eq!(stat_count, 0);
    reopened.close()?;

    assert!(is_damaged_sqlite_code(11));
    assert!(is_damaged_sqlite_code(26));
    assert!(is_damaged_sqlite_code(0x000b | (1 << 8)));
    assert!(!is_damaged_sqlite_code(5));

    let bad_path = temp.path().join("corrupt.sqlite");
    fs::write(&bad_path, b"not a database\n")?;
    assert_eq!(read_migration_head(&bad_path), None);
    let err = SqliteStore::open(&bad_path)
        .err()
        .ok_or("expected damaged error")?;
    assert!(matches!(err, SqliteStoreError::DamagedDatabase { .. }));
    assert!(err.to_string().contains(&bad_path.display().to_string()));
    Ok(())
}

#[tokio::test]
async fn ownership_lock_contention_runtime_state_and_fencing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let runtime_path = temp.path().join("userdata").join("server-runtime.json");

    let owner = ServerOwnerLock::acquire(&runtime_path)?;
    assert!(is_process_alive(i64::from(std::process::id())));
    assert!(!is_process_alive(0));

    let published = owner.publish(PersistedServerRuntimeState::for_current_process(3773, None))?;
    assert_eq!(published.owner_id.as_deref(), owner.owner_id());
    assert!(read_live_server_runtime_state(&runtime_path).is_some());

    let before = fs::read_to_string(&runtime_path)?;
    let contended = ServerOwnerLock::acquire(&runtime_path)
        .err()
        .ok_or("expected contention")?;
    assert_eq!(
        contended.exit_code(),
        Some(SERVER_EXIT_CODE_STATE_DIR_OWNED)
    );
    assert_eq!(contended.exit_code(), Some(78));
    assert!(matches!(contended, OwnershipError::StateDirOwned { .. }));
    assert_eq!(fs::read_to_string(&runtime_path)?, before);

    owner.release()?;
    assert!(!runtime_path.exists());

    // Drop also clears matching runtime state automatically.
    {
        let scoped = ServerOwnerLock::acquire(&runtime_path)?;
        scoped.publish(PersistedServerRuntimeState::for_current_process(3773, None))?;
        assert!(runtime_path.exists());
    }
    assert!(!runtime_path.exists());

    // Never removes a runtime record stamped with a different ownerId, and hold_lock_only never removes.
    let holder = ServerOwnerLock::acquire(&runtime_path)?;
    let state = PersistedServerRuntimeState::for_current_process(3774, None);
    holder.publish(state.clone())?;
    let mut other = state;
    other.owner_id = Some("other-owner".to_owned());
    persist_server_runtime_state(&runtime_path, &other)?;
    holder.release()?;
    assert_eq!(
        read_persisted_server_runtime_state(&runtime_path)?.and_then(|s| s.owner_id),
        Some("other-owner".to_owned())
    );

    let lock_only = ServerOwnerLock::hold_lock_only(&runtime_path)?;
    let blocked = ServerOwnerLock::acquire(&runtime_path)
        .err()
        .ok_or("expected contention")?;
    assert!(matches!(blocked, OwnershipError::StateDirOwned { .. }));
    lock_only.release()?;
    assert!(runtime_path.exists());

    // Pre-lock legacy record blocks live PID and allows dead PID.
    let legacy_dir = temp.path().join("legacy-owner");
    let legacy_runtime = legacy_dir.join("server-runtime.json");
    let mut legacy = PersistedServerRuntimeState::for_current_process(3775, None);
    legacy.owner_id = None;
    legacy.started_at = "2100-01-01T00:00:00.000Z".to_owned();
    persist_server_runtime_state(&legacy_runtime, &legacy)?;
    let legacy_err = ServerOwnerLock::acquire(&legacy_runtime)
        .err()
        .ok_or("expected legacy block")?;
    assert_eq!(legacy_err.exit_code(), Some(78));
    legacy.pid = 0;
    persist_server_runtime_state(&legacy_runtime, &legacy)?;
    ServerOwnerLock::acquire(&legacy_runtime)?.release()?;

    // CancellationToken fence wakes waiters immediately on cancel and (on Unix) on lock unlink.
    let fenced = ServerOwnerLock::acquire_with_poll_interval(
        temp.path().join("fence"),
        std::time::Duration::from_millis(20),
    )?;
    let token = fenced.fence_token();
    assert!(!fenced.is_fenced());
    #[cfg(unix)]
    {
        fs::remove_file(fenced.lock_path())?;
        tokio::time::timeout(std::time::Duration::from_secs(2), token.cancelled()).await?;
        assert!(fenced.is_fenced());
        assert!(matches!(
            fenced.verify_ownership(),
            Err(OwnershipError::LockLost { .. })
        ));
    }
    #[cfg(not(unix))]
    {
        token.cancel();
        token.cancelled().await;
        assert!(fenced.is_fenced());
    }
    fenced.release()?;
    Ok(())
}
