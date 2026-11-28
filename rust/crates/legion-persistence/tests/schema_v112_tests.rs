//! Migration 112 (`WorkflowBoardPlanning`) DDL, v111->v112 upgrade, cascade, and constraint tests.

use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore, read_recorded_migrations};
use legion_persistence::workflow_board_repo::{
    CreateCardInput, create_card, ensure_workflow_board_schema,
};
use rusqlite::{Connection, params};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EXPECTED_M2_TABLES: &[&str] = &[
    "workflow_board_exploration_rounds",
    "workflow_board_decisions",
    "workflow_board_challenges",
    "workflow_board_plan_designs",
    "workflow_board_tasks",
    "workflow_board_planned_checks",
    "workflow_board_ac_evidence",
];

const EXPECTED_M2_INDEXES: &[&str] = &[
    "idx_workflow_board_exploration_rounds_card",
    "idx_workflow_board_decisions_card_status",
    "idx_workflow_board_challenges_card_round",
    "idx_workflow_board_tasks_card_ordinal",
    "idx_workflow_board_tasks_card_status",
    "idx_workflow_board_planned_checks_card_status",
    "idx_workflow_board_ac_evidence_card_status",
];

const EXPECTED_M2_CARD_COLUMNS: &[&str] = &[
    "intake_goal",
    "brief_path",
    "brief_digest",
    "input_paths_json",
    "wireframes_digest",
    "design_digest",
    "tasks_digest",
];

#[test]
fn fresh_bootstrap_creates_v112_columns_tables_indexes_and_migration_record() -> TestResult {
    assert_eq!(BUILD_MIGRATION_HEAD, 114);
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("fresh_v112.sqlite");
    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(114));

    let conn = store.connection();
    for col in EXPECTED_M2_CARD_COLUMNS {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('workflow_board_cards') WHERE name = ?1",
            params![col],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected column {col} on workflow_board_cards");
    }
    for table in EXPECTED_M2_TABLES {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected table {table}");
    }
    for index in EXPECTED_M2_INDEXES {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
            params![index],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected index {index}");
    }

    store.close()?;
    let migrations = read_recorded_migrations(&db_path).ok_or("missing migrations")?;
    assert!(
        migrations
            .iter()
            .any(|m| m.migration_id == 112 && m.name == "WorkflowBoardPlanning")
    );
    Ok(())
}

fn seed_m2_rows_for_card(conn: &Connection, card_id: &str) -> TestResult {
    conn.execute(
        "INSERT INTO workflow_board_decisions (card_id, decision_id, created_at, updated_at)
         VALUES (?1, 'D-001', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        params![card_id],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_tasks (card_id, task_id, title, created_at, updated_at)
         VALUES (?1, 'TSK-0001', 'Shared task id', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        params![card_id],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_exploration_rounds (id, card_id, round_index, status, created_at, updated_at)
         VALUES (?1, ?2, 1, 'paused', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        params![format!("er-{card_id}"), card_id],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_challenges (id, card_id, status, spec_digest, created_at, updated_at)
         VALUES (?1, ?2, 'synthesized', 'sha256:s1', '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
        params![format!("ch-{card_id}"), card_id],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_plan_designs (card_id, updated_at)
         VALUES (?1, '2026-11-24T00:01:00Z')",
        params![card_id],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_planned_checks (card_id, check_id, argv_json, updated_at)
         VALUES (?1, 'chk-1', '[\"cargo\",\"test\"]', '2026-11-24T00:01:00Z')",
        params![card_id],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_ac_evidence (card_id, criterion_id, method, procedure, updated_at)
         VALUES (?1, 'AC-01', 'manual', 'Inspect screen', '2026-11-24T00:01:00Z')",
        params![card_id],
    )?;
    Ok(())
}

#[test]
fn v111_to_v112_upgrade_is_idempotent_and_cascades_across_all_m2_tables() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("upgrade_112.sqlite");

    // Seed a v111 database with a pre-existing workflow_board_cards row.
    let pre = Connection::open(&db_path)?;
    pre.execute_batch(
        "CREATE TABLE effect_sql_migrations (
           migration_id INTEGER PRIMARY KEY NOT NULL,
           created_at DATETIME NOT NULL DEFAULT current_timestamp,
           name VARCHAR(255) NOT NULL
         );
         INSERT INTO effect_sql_migrations (migration_id, name) VALUES (111, 'WorkflowBoard');",
    )?;
    ensure_workflow_board_schema(&pre)?;
    pre.execute(
        "INSERT INTO workflow_board_cards (card_id, project_id, title, stage, status, created_at, updated_at)
         VALUES ('card-pre-112', 'proj-1', 'Pre-112 Card', 'spec', 'active', '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z')",
        [],
    )?;
    pre.close().map_err(|(_, e)| e)?;

    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(114));
    store.close()?;

    let reopened = SqliteStore::open(&db_path)?;
    let conn = reopened.connection();
    assert_eq!(reopened.recorded_migration_head()?, Some(114));

    // Composite PKs allow TSK-0001 and D-001 on two distinct cards without colliding.
    create_card(
        conn,
        &CreateCardInput {
            card_id: "card-second",
            revision_id: "rev-second-1",
            project_id: "proj-1",
            title: "Second Card",
            brief: "Second",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "balanced",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{}",
            spec_digest: Some("sha256:second"),
            wireframes_digest: None,
        },
        "2026-11-24T00:01:00Z",
    )?;
    seed_m2_rows_for_card(conn, "card-pre-112")?;
    seed_m2_rows_for_card(conn, "card-second")?;

    conn.execute(
        "DELETE FROM workflow_board_cards WHERE card_id = 'card-pre-112'",
        [],
    )?;
    for table in EXPECTED_M2_TABLES {
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE card_id = 'card-pre-112'"),
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 0, "expected cascade delete in {table}");
    }

    // Check constraints reject invalid synthesis_count > 1 and invalid strategy.
    assert!(
        conn.execute(
            "INSERT INTO workflow_board_challenges (id, card_id, status, spec_digest, synthesis_count, created_at, updated_at)
             VALUES ('ch-bad', 'card-second', 'synthesized', 'sha256:s2', 2, '2026-11-24T00:01:00Z', '2026-11-24T00:01:00Z')",
            [],
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "INSERT INTO workflow_board_plan_designs (card_id, strategy, updated_at)
             VALUES ('card-second', 'bogus', '2026-11-24T00:01:00Z')",
            [],
        )
        .is_err()
    );
    Ok(())
}
