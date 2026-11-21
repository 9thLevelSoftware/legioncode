//! Integration tests for `t3_persistence::service_preflight`.

use std::error::Error;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, params};
use t3_persistence::service_preflight::{
    BUILD_MIGRATION_HEAD, SERVICE_LAUNCHER_PROTOCOL, ServicePreflightResult, V2_MIGRATION_MANIFEST,
    is_supported_launcher_protocol, reconciled_v2_preview_history, recorded_migrations,
    run_service_preflight, skipped_v2_migrations,
};
use t3_persistence::sqlite::RecordedMigration;

fn create_migrations_db(db_path: &Path, rows: &[(i64, &str)]) -> Result<(), Box<dyn Error>> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "CREATE TABLE effect_sql_migrations (
           migration_id integer PRIMARY KEY NOT NULL,
           created_at datetime NOT NULL DEFAULT current_timestamp,
           name VARCHAR(255) NOT NULL
         );",
    )?;
    for &(id, name) in rows {
        conn.execute(
            "INSERT INTO effect_sql_migrations (migration_id, name) VALUES (?1, ?2)",
            params![id, name],
        )?;
    }
    conn.close().map_err(|(_, err)| err)?;
    Ok(())
}

#[test]
fn missing_db_and_supported_protocols_are_ready_with_exact_wire_json() -> Result<(), Box<dyn Error>>
{
    assert!(is_supported_launcher_protocol(3));
    assert!(is_supported_launcher_protocol(4));
    assert!(!is_supported_launcher_protocol(2));
    assert!(!is_supported_launcher_protocol(5));
    assert_eq!(V2_MIGRATION_MANIFEST.len(), 48);

    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("missing.sqlite");
    assert_eq!(recorded_migrations(&db_path), Some(Vec::new()));

    let ready_v4 = run_service_preflight(&db_path, SERVICE_LAUNCHER_PROTOCOL, "0.2.9");
    assert!(ready_v4.is_ready());
    assert_eq!(ready_v4.version(), "0.2.9");
    assert_eq!(ready_v4.reason(), None);
    assert_eq!(
        serde_json::to_string(&ready_v4)?,
        r#"{"status":"ready","version":"0.2.9","launcherProtocol":4}"#,
    );

    let ready_v3 = run_service_preflight(&db_path, 3, "0.2.9");
    assert_eq!(
        serde_json::to_string(&ready_v3)?,
        r#"{"status":"ready","version":"0.2.9","launcherProtocol":3}"#,
    );
    Ok(())
}

#[test]
fn unsupported_protocol_emits_exact_blocked_reason_and_json() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");

    let blocked = run_service_preflight(&db_path, 2, "0.4.1");
    let expected_reason = "The installed service launcher speaks protocol 2, but t3@0.4.1 requires protocol 4. On the server machine, install this exact CLI: `curl -fsSL https://t3.codes/install.sh | T3CODE_VERSION=0.4.1 sh`. Check that `t3 --version` prints 0.4.1 and that `which -a t3` lists no older t3 (such as an npm or nvm install) first, then run `~/.local/bin/t3 service install` (the installer's default path). Updating the desktop app does not update this CLI, and `t3 service update` only repairs the service with whichever CLI runs it.";
    assert_eq!(
        blocked,
        ServicePreflightResult::Blocked {
            version: "0.4.1".to_owned(),
            reason: expected_reason.to_owned(),
        },
    );
    let expected_json = format!(
        r#"{{"status":"blocked","version":"0.4.1","reason":{}}}"#,
        serde_json::to_string(expected_reason)?,
    );
    assert_eq!(serde_json::to_string(&blocked)?, expected_json);
    Ok(())
}

#[test]
fn empty_sqlite_without_migrations_table_is_ready_while_corrupt_file_blocks()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;

    let empty_sqlite = dir.path().join("no-migrations.sqlite");
    let conn = Connection::open(&empty_sqlite)?;
    conn.execute_batch("CREATE TABLE scratch (id INTEGER PRIMARY KEY);")?;
    conn.close().map_err(|(_, err)| err)?;
    assert_eq!(recorded_migrations(&empty_sqlite), Some(Vec::new()));
    assert!(run_service_preflight(&empty_sqlite, 4, "1.0.0").is_ready());

    let corrupt = dir.path().join("corrupt.sqlite");
    fs::write(&corrupt, b"not a sqlite file at all")?;
    assert_eq!(recorded_migrations(&corrupt), None);
    let blocked = run_service_preflight(&corrupt, 4, "1.0.0");
    assert_eq!(
        blocked.reason(),
        Some("Could not read this database to confirm t3@1.0.0 supports it."),
    );
    Ok(())
}

#[test]
fn newer_migrations_block_with_exact_versioned_message() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("newer.sqlite");
    let mut rows = V2_MIGRATION_MANIFEST.to_vec();
    rows.push((BUILD_MIGRATION_HEAD + 1, "FutureOne"));
    rows.push((BUILD_MIGRATION_HEAD + 2, "FutureTwo"));
    create_migrations_db(&db_path, &rows)?;

    let blocked = run_service_preflight(&db_path, 4, "0.9.0");
    assert_eq!(
        blocked.reason(),
        Some(
            "This database has migrations that t3@0.9.0 does not know: 103_FutureOne, 104_FutureTwo. A newer build wrote them, and running t3@0.9.0 on this database is not safe. Stay on the newer release, or wait for a release that includes these migrations."
        ),
    );
    Ok(())
}

#[test]
fn v2_preview_history_reconciliation_handles_valid_and_forked_histories()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;

    let legacy_54 = dir.path().join("preview54.sqlite");
    create_migrations_db(
        &legacy_54,
        &[
            (52, "PriorMigration"),
            (53, "PullRequestFilesViewed"),
            (54, "OrchestrationV2"),
            (55, "RemoveRedundantProjectionIndexes"),
        ],
    )?;
    assert!(run_service_preflight(&legacy_54, 4, "0.9.0").is_ready());

    let legacy_53 = dir.path().join("preview53.sqlite");
    create_migrations_db(
        &legacy_53,
        &[(52, "PriorMigration"), (53, "OrchestrationV2")],
    )?;
    assert!(run_service_preflight(&legacy_53, 4, "0.9.0").is_ready());

    let forked = vec![
        RecordedMigration {
            migration_id: 54,
            name: "OrchestrationV2".to_owned(),
        },
        RecordedMigration {
            migration_id: 56,
            name: "ScheduledTaskWebhooks".to_owned(),
        },
    ];
    assert_eq!(reconciled_v2_preview_history(&forked), None);

    let forked_db = dir.path().join("forked.sqlite");
    create_migrations_db(
        &forked_db,
        &[(54, "OrchestrationV2"), (56, "ScheduledTaskWebhooks")],
    )?;
    let blocked = run_service_preflight(&forked_db, 4, "0.9.0");
    assert_eq!(
        blocked.reason(),
        Some(
            "This database has incompatible migration history: an unexpected V2 preview history. Start the build that wrote it, or report this migration history so an upgrade path can be added."
        ),
    );
    Ok(())
}

#[test]
fn full_v2_manifest_is_ready_while_skipped_or_renamed_v2_migrations_block()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;

    let full_db = dir.path().join("full.sqlite");
    create_migrations_db(&full_db, V2_MIGRATION_MANIFEST)?;
    assert!(run_service_preflight(&full_db, 4, "0.9.0").is_ready());

    let gapped_rows = vec![
        RecordedMigration {
            migration_id: 55,
            name: "OrchestrationV2".to_owned(),
        },
        RecordedMigration {
            migration_id: 56,
            name: "WrongName".to_owned(),
        },
        RecordedMigration {
            migration_id: 58,
            name: "WebhookRelayDeliveries".to_owned(),
        },
    ];
    assert_eq!(
        skipped_v2_migrations(&gapped_rows),
        vec![
            "56_RemoveRedundantProjectionIndexes (recorded: WrongName)".to_owned(),
            "57_ScheduledTaskWebhooks (recorded: nothing)".to_owned(),
        ],
    );

    let gapped_db = dir.path().join("gapped.sqlite");
    create_migrations_db(
        &gapped_db,
        &[
            (55, "OrchestrationV2"),
            (56, "WrongName"),
            (58, "WebhookRelayDeliveries"),
        ],
    )?;
    let blocked = run_service_preflight(&gapped_db, 4, "0.9.0");
    assert_eq!(
        blocked.reason(),
        Some(
            "This database has incompatible migration history: 56_RemoveRedundantProjectionIndexes (recorded: WrongName), 57_ScheduledTaskWebhooks (recorded: nothing). Start the build that wrote it, or report this migration history so an upgrade path can be added."
        ),
    );
    Ok(())
}
