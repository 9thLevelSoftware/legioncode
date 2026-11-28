//! Migration 113 (`WorkflowBoardExecuteShip`) DDL, v112->v113 table rebuild, FK integrity,
//! `'shipped'` status, and cascade tests.

use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore, read_recorded_migrations};
use legion_persistence::workflow_board_repo::ensure_workflow_board_schema;
use rusqlite::{Connection, params};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EXPECTED_M3_TABLES: &[&str] = &[
    "workflow_board_contract_violations",
    "workflow_board_check_receipts",
    "workflow_board_review_runs",
    "workflow_board_review_findings",
    "workflow_board_amendments",
    "workflow_board_external_actions",
    "workflow_board_delivery_receipts",
];

const EXPECTED_M3_INDEXES: &[&str] = &[
    "idx_workflow_board_contract_violations_card",
    "idx_workflow_board_check_receipts_card_check",
    "idx_workflow_board_check_receipts_card_status",
    "idx_workflow_board_review_runs_card",
    "idx_workflow_board_review_findings_card",
    "idx_workflow_board_amendments_card",
    "idx_workflow_board_external_actions_card",
    "idx_workflow_board_delivery_receipts_card",
];

const EXPECTED_M3_CARD_COLUMNS: &[&str] = &[
    "product_snapshot_digest",
    "paused_from_status",
    "pause_reason",
    "paused_at",
    "external_edit_state",
    "external_edit_detail_json",
];

#[test]
fn fresh_bootstrap_creates_v113_tables_indexes_columns_and_migration_record() -> TestResult {
    assert_eq!(BUILD_MIGRATION_HEAD, 114);
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("fresh_v113.sqlite");
    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(114));

    let conn = store.connection();
    for col in EXPECTED_M3_CARD_COLUMNS {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('workflow_board_cards') WHERE name = ?1",
            params![col],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected M3 column {col}");
    }
    for table in EXPECTED_M3_TABLES {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected M3 table {table}");
    }
    for index in EXPECTED_M3_INDEXES {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
            params![index],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected M3 index {index}");
    }
    store.close()?;

    let migrations = read_recorded_migrations(&db_path).ok_or("missing migrations")?;
    assert!(
        migrations
            .iter()
            .any(|m| m.migration_id == 113 && m.name == "WorkflowBoardExecuteShip")
    );
    Ok(())
}

fn seed_pre_v113_schema(conn: &Connection) -> TestResult {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE effect_sql_migrations (
           migration_id INTEGER PRIMARY KEY NOT NULL,
           created_at DATETIME NOT NULL DEFAULT current_timestamp,
           name VARCHAR(255) NOT NULL
         );
         INSERT INTO effect_sql_migrations (migration_id, name) VALUES (112, 'WorkflowBoardPlanning');
         CREATE TABLE workflow_board_cards (
           card_id TEXT PRIMARY KEY,
           project_id TEXT NOT NULL,
           title TEXT NOT NULL,
           stage TEXT NOT NULL CHECK (stage IN ('spec', 'plan', 'execute', 'ship')),
           status TEXT NOT NULL CHECK (status IN ('active', 'blocked', 'paused', 'completed')),
           revision INTEGER NOT NULL DEFAULT 1,
           brief TEXT NOT NULL DEFAULT '',
           source_type TEXT NOT NULL DEFAULT 'idea',
           assistance_mode TEXT NOT NULL DEFAULT 'balanced',
           workspace_strategy_json TEXT NOT NULL DEFAULT '{\"type\":\"root\"}',
           worktree_path TEXT,
           branch TEXT,
           model_selection_json TEXT,
           runtime_mode TEXT,
           interaction_mode TEXT,
           spec_digest TEXT,
           plan_digest TEXT,
           current_task_id TEXT,
           blocker_reason TEXT,
           next_action TEXT,
           spec_json TEXT NOT NULL DEFAULT '{}',
           plan_json TEXT,
           import_provenance_json TEXT,
           metadata_json TEXT NOT NULL DEFAULT '{}',
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           completed_at TEXT,
           archived_at TEXT,
           intake_goal TEXT,
           brief_path TEXT,
           brief_digest TEXT,
           input_paths_json TEXT NOT NULL DEFAULT '[]',
           wireframes_digest TEXT,
           design_digest TEXT,
           tasks_digest TEXT
         );
         CREATE TABLE workflow_board_revisions (
           id TEXT PRIMARY KEY,
           card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
           revision INTEGER NOT NULL,
           stage TEXT NOT NULL,
           kind TEXT NOT NULL,
           actor_kind TEXT NOT NULL DEFAULT 'client',
           summary TEXT NOT NULL DEFAULT '',
           spec_digest TEXT,
           plan_digest TEXT,
           payload_json TEXT NOT NULL DEFAULT '{}',
           created_at TEXT NOT NULL,
           UNIQUE (card_id, revision)
         );
         INSERT INTO workflow_board_cards (card_id, project_id, title, stage, status, created_at, updated_at)
         VALUES ('card-v112', 'proj-1', 'Pre-113 Card', 'execute', 'active', '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z');
         INSERT INTO workflow_board_revisions (id, card_id, revision, stage, kind, created_at)
         VALUES ('rev-v112-1', 'card-v112', 1, 'spec', 'card_created', '2026-11-24T00:00:00Z');",
    )?;
    Ok(())
}

#[test]
fn v112_to_v113_upgrade_rebuilds_cards_safely_preserves_fks_and_cascades_m3_tables() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("upgrade_113.sqlite");
    let pre = Connection::open(&db_path)?;
    seed_pre_v113_schema(&pre)?;
    // Verify pre-v113 rejects 'shipped'.
    assert!(
        pre.execute(
            "UPDATE workflow_board_cards SET status = 'shipped' WHERE card_id = 'card-v112'",
            [],
        )
        .is_err()
    );
    pre.close().map_err(|(_, e)| e)?;

    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(114));
    store.close()?;

    // Reopening remains idempotent.
    let reopened = SqliteStore::open(&db_path)?;
    let conn = reopened.connection();
    assert_eq!(reopened.recorded_migration_head()?, Some(114));
    assert!(
        conn.query_row("PRAGMA foreign_key_check", [], |r| r.get::<_, String>(0))
            .is_err()
    );

    // Pre-existing row preserved and accepts 'shipped'.
    conn.execute(
        "UPDATE workflow_board_cards SET stage = 'ship', status = 'shipped' WHERE card_id = 'card-v112'",
        [],
    )?;
    let status: String = conn.query_row(
        "SELECT status FROM workflow_board_cards WHERE card_id = 'card-v112'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(status, "shipped");

    // Seed all 7 M3 child tables and confirm ON DELETE CASCADE.
    conn.execute(
        "INSERT INTO workflow_board_contract_violations (id, card_id, task_id, card_revision, created_at, updated_at)
         VALUES ('viol-1', 'card-v112', 'TSK-1', 1, '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_check_receipts (id, card_id, check_id, card_revision, argv_json, status, updated_at)
         VALUES ('chk-r1', 'card-v112', 'chk-1', 1, '[\"cargo\",\"test\"]', 'passed', '2026-11-24T00:01:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_review_runs (id, card_id, card_revision, created_at, updated_at)
         VALUES ('rr-1', 'card-v112', 1, '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_review_findings (id, card_id, review_run_id, severity, message, created_at, updated_at)
         VALUES ('rf-1', 'card-v112', 'rr-1', 'major', 'Fix edge case', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_amendments (id, card_id, card_revision, reason, created_at, updated_at)
         VALUES ('am-1', 'card-v112', 1, 'Expand scope', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_external_actions (id, card_id, action_kind, idempotency_key, status, created_at, updated_at)
         VALUES ('ea-1', 'card-v112', 'task_turn', 'k-1', 'interrupted', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_delivery_receipts (id, card_id, status, card_revision, product_snapshot_digest, approved_by, approved_at, created_at, updated_at)
         VALUES ('dr-1', 'card-v112', 'shipped', 1, 'sha256:snap-1', 'client', '2026-11-24T00:02:00Z', '2026-11-24T00:02:00Z', '2026-11-24T00:02:00Z')",
        [],
    )?;

    conn.execute(
        "DELETE FROM workflow_board_cards WHERE card_id = 'card-v112'",
        [],
    )?;
    for table in EXPECTED_M3_TABLES {
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE card_id = 'card-v112'"),
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 0, "expected cascade delete in {table}");
    }
    let rev_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_board_revisions WHERE card_id = 'card-v112'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(rev_count, 0);
    Ok(())
}

#[test]
fn pre_v113_schema_ensure_refuses_active_transaction_without_wiping_child_rows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("tx_guard_113.sqlite");
    let conn = Connection::open(&db_path)?;
    seed_pre_v113_schema(&conn)?;

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    assert!(ensure_workflow_board_schema(&conn).is_err());
    conn.execute_batch("ROLLBACK;")?;

    let revs_after_rollback: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_board_revisions WHERE card_id = 'card-v112'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(revs_after_rollback, 1);

    // Autocommit rebuild succeeds and preserves pre-existing child rows.
    ensure_workflow_board_schema(&conn)?;
    let revs_after_upgrade: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_board_revisions WHERE card_id = 'card-v112'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(revs_after_upgrade, 1);
    conn.execute(
        "UPDATE workflow_board_cards SET stage = 'ship', status = 'shipped' WHERE card_id = 'card-v112'",
        [],
    )?;
    Ok(())
}
