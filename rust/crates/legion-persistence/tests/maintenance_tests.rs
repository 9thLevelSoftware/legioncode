//! Doctor report formatting, legacy inspection, and offline database compaction tests.

use std::path::Path;

use legion_persistence::maintenance::{
    DATABASE_COMPACTION_RECOMMENDED_BYTES, DatabaseCompactStep, DatabaseInspection, DoctorOptions,
    LegacyOnlyRecords, MaintenanceError, compact_database_with_steps, format_bytes,
    inspect_database, remove_legacy_database, render_doctor_report,
};
use legion_persistence::sqlite::SqliteStore;
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn doctor_report_matches_cli_byte_for_byte() {
    assert_eq!(format_bytes(0), "0 KiB");
    assert_eq!(format_bytes(1), "1 KiB");
    assert_eq!(format_bytes(1024), "1 KiB");
    assert_eq!(format_bytes(1025), "2 KiB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    assert_eq!(format_bytes(12 * 1024 * 1024 / 10), "1.2 MiB");
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GiB");

    let clean = DatabaseInspection {
        database_bytes: 16 * 1024,
        reclaimable_bytes: 0,
        incremental_vacuum_enabled: true,
        legacy_database_bytes: None,
        legacy_only_records: None,
    };
    assert_eq!(
        render_doctor_report(&clean, DoctorOptions::default()),
        "Database: 16 KiB, 0 KiB reclaimable.\nNothing to repair."
    );

    let non_incremental = DatabaseInspection {
        database_bytes: 5 * 1024 * 1024,
        reclaimable_bytes: 2 * 1024 * 1024,
        incremental_vacuum_enabled: false,
        legacy_database_bytes: None,
        legacy_only_records: None,
    };
    let expected_dry = concat!(
        "Database: 5.0 MiB, 2.0 MiB reclaimable.\n",
        "\n",
        "Reclaim database space (recommended)\n",
        "  Returns unused space to disk and turns on automatic shrinking.\n",
        "  Rewrites the whole 5.0 MiB database, which can take more than an hour on a large database or slow disk.\n",
        "  Pauses the background service, if installed, while it runs and may need temporary disk space.\n",
        "\n",
        "Dry run: nothing changed."
    );
    assert_eq!(
        render_doctor_report(
            &non_incremental,
            DoctorOptions {
                fix: false,
                dry_run: true,
                interactive: false,
            }
        ),
        expected_dry
    );

    let with_legacy = DatabaseInspection {
        database_bytes: 2 * DATABASE_COMPACTION_RECOMMENDED_BYTES,
        reclaimable_bytes: DATABASE_COMPACTION_RECOMMENDED_BYTES,
        incremental_vacuum_enabled: true,
        legacy_database_bytes: Some(4 * 1024),
        legacy_only_records: Some(LegacyOnlyRecords {
            projects: 0,
            threads: 0,
        }),
    };
    let expected_legacy = concat!(
        "Database: 2.00 GiB, 1.00 GiB reclaimable.\n",
        "Database from before 0.1.0: 4 KiB, kept for rolling back.\n",
        "\n",
        "Reclaim database space (recommended)\n",
        "  Returns unused space to disk.\n",
        "  Rewrites the whole 2.00 GiB database, which can take more than an hour on a large database or slow disk.\n",
        "  Pauses the background service, if installed, while it runs and may need temporary disk space.\n",
        "\n",
        "Delete the database from before 0.1.0\n",
        "  Frees 4 KiB.\n",
        "  Warning: after this, a LegionCode version before 0.1.0 starts with no threads or settings, so you can no longer roll back.\n",
        "\n",
        "Run in a terminal to choose repairs, or pass --fix for the recommended ones."
    );
    assert_eq!(
        render_doctor_report(&with_legacy, DoctorOptions::default()),
        expected_legacy
    );
}

#[test]
fn compact_database_shrinks_free_pages_rebuilds_fts5_and_checks_contention() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("statev2.sqlite");
    let store = SqliteStore::open(&db_path)?;
    store.connection().execute_batch(
        "PRAGMA auto_vacuum = NONE;
         VACUUM;
         CREATE TABLE retained (v TEXT);
         INSERT INTO retained VALUES ('keep');
         CREATE TABLE discarded (b BLOB);
         INSERT INTO discarded VALUES (zeroblob(1048576));
         DROP TABLE discarded;
         INSERT INTO orchestration_v2_projection_messages (
           message_id, thread_id, run_id, node_id, role, streaming, created_at, updated_at, payload_json
         ) VALUES (
           'msg-1', 't-1', NULL, NULL, 'user', 0,
           '2026-09-01T00:00:00.000Z', '2026-09-01T00:00:00.000Z', '{\"text\":\"needle\"}'
         );
         INSERT INTO orchestration_v2_message_search (orchestration_v2_message_search)
         VALUES ('delete-all');
         INSERT INTO orchestration_v2_legacy_imports (
           thread_id, source_updated_at, shell_imported_at, transcript_imported_at, imported_message_count
         ) VALUES ('imported', '2026-10-17', '2026-10-17', '2026-10-17', 0);
         INSERT INTO orchestration_command_receipts (
           command_id, aggregate_kind, aggregate_id, accepted_at, result_sequence, status, command_type
         ) VALUES ('cmd-1', 'thread', 'imported', '2026-10-17', 0, 'accepted', 'legacy');
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    store.close()?;

    let before = inspect_database(&db_path)?;
    assert!(!before.incremental_vacuum_enabled);
    assert!(before.reclaimable_bytes >= 512 * 1024);

    // Must refuse offline compaction while another connection holds the WAL database open.
    let active_reader = Connection::open(&db_path)?;
    let _: String = active_reader.query_row("SELECT v FROM retained", [], |r| r.get(0))?;
    let in_use = compact_database_with_steps(&db_path, |_| {})
        .err()
        .ok_or("expected DatabaseInUse")?;
    assert!(matches!(in_use, MaintenanceError::DatabaseInUse { .. }));
    active_reader.close().map_err(|(_, e)| e)?;

    let mut steps = Vec::new();
    let result = compact_database_with_steps(&db_path, |step| steps.push(step))?;
    assert_eq!(
        steps,
        vec![
            DatabaseCompactStep::PruneEvents,
            DatabaseCompactStep::Vacuum,
            DatabaseCompactStep::RebuildSearch,
            DatabaseCompactStep::Checkpoint,
        ]
    );
    assert_eq!(result.deleted_receipt_count, 1);
    assert!(result.after_bytes < result.before_bytes);

    let after = inspect_database(&db_path)?;
    assert!(after.incremental_vacuum_enabled);
    assert_eq!(after.reclaimable_bytes, 0);
    assert!(after.database_bytes < before.database_bytes);

    let verify_conn = Connection::open(&db_path)?;
    let matched: String = verify_conn.query_row(
        "SELECT m.message_id FROM orchestration_v2_message_search s
         JOIN orchestration_v2_projection_messages m ON m.rowid = s.rowid
         WHERE orchestration_v2_message_search MATCH '\"needle\"'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(matched, "msg-1");
    verify_conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

#[test]
fn inspect_database_reports_legacy_records_and_renders_clean_vs_missing() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("statev2.sqlite");
    let legacy_path = temp.path().join("state.sqlite");

    let store = SqliteStore::open(&db_path)?;
    store.close()?;

    let legacy = Connection::open(&legacy_path)?;
    legacy.execute_batch(
        "CREATE TABLE projection_projects (project_id TEXT PRIMARY KEY, deleted_at TEXT);
         CREATE TABLE projection_threads (thread_id TEXT PRIMARY KEY, deleted_at TEXT);
         INSERT INTO projection_projects VALUES ('p1', NULL);
         INSERT INTO projection_threads VALUES ('t1', NULL), ('t2', '2026-10-01');",
    )?;
    legacy.close().map_err(|(_, e)| e)?;

    let inspected = inspect_database(&db_path)?;
    assert!(inspected.legacy_database_bytes.is_some());
    assert_eq!(
        inspected.legacy_only_records,
        Some(LegacyOnlyRecords {
            projects: 1,
            threads: 1,
        })
    );
    let report = render_doctor_report(&inspected, DoctorOptions::default());
    assert!(report.contains("Projects missing: 1. Threads missing: 1."));
    assert!(Path::new(&db_path).exists());
    Ok(())
}

#[test]
fn compact_database_prunes_stale_superseded_events() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("prune.sqlite");
    let store = SqliteStore::open(&db_path)?;
    store.connection().execute_batch(
        "INSERT INTO orchestration_v2_projection_metadata (projection_name, schema_version, last_sequence, updated_at)
         VALUES
           ('thread-projections', 2, 10, '2026-10-01T00:00:00.000Z'),
           ('event-log-prune-mark', 2, 10, '2020-01-01T00:00:00.000Z');
         INSERT INTO orchestration_events (
           event_id, aggregate_kind, stream_id, stream_version, event_type, occurred_at,
           command_id, causation_event_id, correlation_id, actor_kind, payload_json, metadata_json, application_event_version
         ) VALUES
           ('e1', 'thread', 't1', 0, 'thread.created', '2026-01-01T00:00:00Z', NULL, NULL, NULL, 'server', '{}', '{}', 2),
           ('e2', 'thread', 't1', 1, 'thread.updated', '2026-01-01T00:01:00Z', NULL, NULL, NULL, 'server', '{}', '{}', 2),
           ('e3', 'thread', 't1', 2, 'thread.updated', '2026-01-01T00:02:00Z', NULL, NULL, NULL, 'server', '{}', '{}', 2);",
    )?;
    store.close()?;

    let res = legion_persistence::compact_database(&db_path)?;
    assert_eq!(res.deleted_event_count, 1);
    let conn = Connection::open(&db_path)?;
    let remaining: i64 = conn.query_row("SELECT COUNT(*) FROM orchestration_events", [], |r| {
        r.get(0)
    })?;
    assert_eq!(remaining, 2);
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

#[test]
fn remove_legacy_database_checks_unmigrated_records_and_lock_contention() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("statev2.sqlite");
    let legacy_path = temp.path().join("state.sqlite");

    let store = SqliteStore::open(&db_path)?;
    store.close()?;
    assert!(!remove_legacy_database(&db_path)?);

    let legacy = Connection::open(&legacy_path)?;
    legacy.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE projection_projects (project_id TEXT PRIMARY KEY, deleted_at TEXT);
         CREATE TABLE projection_threads (thread_id TEXT PRIMARY KEY, deleted_at TEXT);
         INSERT INTO projection_projects VALUES ('p1', NULL);
         INSERT INTO projection_threads VALUES ('t1', NULL), ('t2', '2026-10-01');",
    )?;
    legacy.close().map_err(|(_, e)| e)?;

    let blocked = remove_legacy_database(&db_path);
    assert!(matches!(
        blocked,
        Err(MaintenanceError::LegacyDatabaseHasUnmigratedRecords {
            projects: 1,
            threads: 1
        })
    ));
    assert!(legacy_path.exists());

    // Copy missing rows into statev2.sqlite so migration check passes.
    let v2 = Connection::open(&db_path)?;
    v2.execute_batch(
        "INSERT INTO projection_projects (project_id, title, workspace_root, scripts_json, created_at, updated_at)
         VALUES ('p1', 'P1', '/work/p1', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
         INSERT INTO orchestration_v2_projection_threads (
           thread_id, project_id, title, default_provider, runtime_mode, interaction_mode, created_at, updated_at, payload_json
         ) VALUES ('t1', 'p1', 'T1', 'codex', 'full-access', 'default', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '{}');",
    )?;
    v2.close().map_err(|(_, e)| e)?;

    // Contended legacy lock rejects removal safely.
    let holder = Connection::open(&legacy_path)?;
    holder.execute_batch("BEGIN EXCLUSIVE;")?;
    assert!(matches!(
        remove_legacy_database(&db_path),
        Err(MaintenanceError::DatabaseInUse { .. })
    ));
    holder.execute_batch("ROLLBACK;")?;
    holder.close().map_err(|(_, e)| e)?;

    // Leave -wal / -shm sidecars and verify remove_legacy_database deletes all three.
    std::fs::write(temp.path().join("state.sqlite-wal"), b"wal")?;
    std::fs::write(temp.path().join("state.sqlite-shm"), b"shm")?;
    assert!(remove_legacy_database(&db_path)?);
    assert!(!legacy_path.exists());
    assert!(!temp.path().join("state.sqlite-wal").exists());
    assert!(!temp.path().join("state.sqlite-shm").exists());
    assert!(db_path.exists());
    Ok(())
}
