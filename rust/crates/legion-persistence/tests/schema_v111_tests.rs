//! Migration 111 (`WorkflowBoard`) DDL, upgrade, constraint, and compatibility tests.

use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore, read_recorded_migrations};
use legion_persistence::workflow_board_repo::{
    CreateCardInput, StageWriteInput, UpsertStageRunInput, create_card, get_card_detail,
    link_stage_run_thread, list_incomplete_journal_entries, stage_write,
    upsert_or_get_stage_run_by_launch_key,
};
use legion_persistence::workflow_board_repo::{LinkStageRunThreadInput, UpsertStageRunOutcome};
use rusqlite::{Connection, params};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EXPECTED_TABLES: &[&str] = &[
    "workflow_board_cards",
    "workflow_board_revisions",
    "workflow_board_approvals",
    "workflow_board_stage_runs",
    "workflow_board_write_journal",
];

const EXPECTED_INDEXES: &[&str] = &[
    "idx_workflow_board_cards_project_updated",
    "idx_workflow_board_cards_project_stage",
    "idx_workflow_board_revisions_card_rev",
    "idx_workflow_board_approvals_active_gate",
    "idx_workflow_board_approvals_card_gate",
    "idx_workflow_board_stage_runs_card_stage",
    "idx_workflow_board_stage_runs_thread",
    "idx_workflow_board_stage_runs_workflow_run",
    "idx_workflow_board_write_journal_incomplete",
];

#[test]
fn fresh_bootstrap_creates_v111_tables_indexes_and_migration_record() -> TestResult {
    assert_eq!(BUILD_MIGRATION_HEAD, 114);
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("fresh.sqlite");
    let store = SqliteStore::open(&db_path)?;

    assert_eq!(store.recorded_migration_head()?, Some(114));
    let conn = store.connection();

    for table in EXPECTED_TABLES {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected table {table}");
    }
    for index in EXPECTED_INDEXES {
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
            .any(|m| m.migration_id == 111 && m.name == "WorkflowBoard")
    );
    Ok(())
}

#[test]
fn v110_to_v111_upgrade_preserves_existing_data_and_is_idempotent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("pre111.sqlite");

    // Seed a v110-shaped DB with pre-existing project & workflow_runs rows.
    let pre = Connection::open(&db_path)?;
    pre.execute_batch(
        "CREATE TABLE effect_sql_migrations (
           migration_id INTEGER PRIMARY KEY NOT NULL,
           created_at DATETIME NOT NULL DEFAULT current_timestamp,
           name VARCHAR(255) NOT NULL
         );
         INSERT INTO effect_sql_migrations (migration_id, name) VALUES (110, 'Workflows');
         CREATE TABLE projection_projects (
           project_id TEXT PRIMARY KEY,
           title TEXT NOT NULL,
           workspace_root TEXT NOT NULL,
           scripts_json TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           deleted_at TEXT
         );
         INSERT INTO projection_projects (project_id, title, workspace_root, scripts_json, created_at, updated_at)
         VALUES ('proj-legacy', 'Legacy', '/repo', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
         CREATE TABLE workflow_runs (
           run_id TEXT PRIMARY KEY,
           workflow TEXT NOT NULL,
           version INTEGER NOT NULL,
           engine_tag TEXT NOT NULL,
           idempotency_key TEXT NOT NULL,
           title TEXT NOT NULL,
           status TEXT NOT NULL,
           detail TEXT,
           result_json TEXT,
           pauses INTEGER NOT NULL DEFAULT 0,
           started_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           ended_at TEXT,
           input_json TEXT,
           parent_run_id TEXT,
           parent_wait TEXT,
           held INTEGER NOT NULL DEFAULT 0,
           engine_pruned_at TEXT
         );
         INSERT INTO workflow_runs (run_id, workflow, version, engine_tag, idempotency_key, title, status, started_at, updated_at)
         VALUES ('run-legacy', 'plan', 1, 'v1', 'idem-legacy', 'Legacy Run', 'completed', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
    )?;
    pre.close().map_err(|(_, e)| e)?;

    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(114));
    let title: String = store.connection().query_row(
        "SELECT title FROM projection_projects WHERE project_id = 'proj-legacy'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(title, "Legacy");
    store.close()?;

    // Second open must stay idempotent.
    let reopened = SqliteStore::open(&db_path)?;
    assert_eq!(reopened.recorded_migration_head()?, Some(114));
    for table in EXPECTED_TABLES {
        let count: i64 = reopened.connection().query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "expected table {table} after reopen");
    }
    Ok(())
}

fn seed_card(conn: &Connection, card_id: &str) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            revision_id: Box::leak(format!("rev-{card_id}").into_boxed_str()),
            project_id: "proj-1",
            title: "Cascade & Lifetime Card",
            brief: "Check foreign key boundaries",
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
            spec_digest: Some("sha256:init"),
            wireframes_digest: None,
        },
        "2026-11-24T00:00:00Z",
    )?;
    Ok(())
}

#[test]
fn distinct_run_lifetimes_and_unique_active_gate() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-v111")?;

    conn.execute(
        "INSERT INTO workflow_runs (run_id, workflow, version, engine_tag, idempotency_key, title, status, started_at, updated_at)
         VALUES ('run-prune-1', 'board.plan', 1, 'v1', 'idem-prune-1', 'Plan', 'completed', '2026-11-24T00:01:00Z', '2026-11-24T00:02:00Z')",
        [],
    )?;
    let UpsertStageRunOutcome::Created(sr) = upsert_or_get_stage_run_by_launch_key(
        conn,
        &UpsertStageRunInput {
            id: "sr-1",
            card_id: "card-v111",
            stage: "plan",
            task_id: None,
            attempt: 1,
            stable_launch_key: "card-v111:plan:1",
            card_revision: 1,
            provider_instance_id: "devReplay",
            model: "dev-replay",
            model_options_json: None,
            runtime_mode: "auto-accept-edits",
            interaction_mode: "plan",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            brief_digest: None,
        },
        "2026-11-24T00:01:00Z",
    )?
    else {
        return Err("expected Created stage run".into());
    };
    link_stage_run_thread(
        conn,
        &LinkStageRunThreadInput {
            id: &sr.id,
            status: "completed",
            thread_id: Some("thr-1"),
            workflow_run_id: Some("run-prune-1"),
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: Some("2026-11-24T00:02:00Z"),
        },
        "2026-11-24T00:02:00Z",
    )?;
    conn.execute("DELETE FROM workflow_runs WHERE run_id = 'run-prune-1'", [])?;
    let detail_after_prune = get_card_detail(conn, "card-v111")?;
    assert_eq!(detail_after_prune.stage_runs.len(), 1);
    assert_eq!(
        detail_after_prune.stage_runs[0].workflow_run_id.as_deref(),
        Some("run-prune-1")
    );

    conn.execute(
        "INSERT INTO workflow_board_approvals (id, card_id, gate, status, revision, artifact_digest, approved_at)
         VALUES ('apr-1', 'card-v111', 'spec', 'valid', 1, 'sha256:a', '2026-11-24T00:03:00Z')",
        [],
    )?;
    let dup = conn.execute(
        "INSERT INTO workflow_board_approvals (id, card_id, gate, status, revision, artifact_digest, approved_at)
         VALUES ('apr-2', 'card-v111', 'spec', 'valid', 2, 'sha256:b', '2026-11-24T00:04:00Z')",
        [],
    );
    assert!(
        dup.is_err(),
        "unique partial index must reject two valid spec approvals"
    );
    Ok(())
}

#[test]
fn cascade_deletes_and_raw_journal_insert_compat() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    seed_card(conn, "card-cas-del")?;

    conn.execute(
        "INSERT INTO workflow_board_approvals (id, card_id, gate, status, revision, artifact_digest, approved_at)
         VALUES ('apr-del', 'card-cas-del', 'spec', 'valid', 1, 'sha256:a', '2026-11-24T00:03:00Z')",
        [],
    )?;
    // Raw E2E write-journal insert compatibility (f20_sqlite_write_journal.rs:136).
    conn.execute(
        "INSERT OR IGNORE INTO workflow_board_write_journal (id, card_id, target_path, status, created_at)
         VALUES ('j-crash-1', ?1, 'spec.md', 'pending', '2026-01-01T00:00:00Z')",
        params!["card-cas-del"],
    )?;
    let _staged = stage_write(
        conn,
        &StageWriteInput {
            id: "j-staged-2",
            card_id: "card-cas-del",
            target_path: "card.json",
            staging_path: Some("card.json.tmp"),
            operation: "write",
            expected_digest: Some("sha256:card"),
            previous_digest: None,
            revision: Some(1),
        },
        "2026-11-24T00:05:00Z",
    )?;
    let incomplete = list_incomplete_journal_entries(conn, Some("card-cas-del"))?;
    assert_eq!(incomplete.len(), 2);
    assert_eq!(incomplete[0].id, "j-crash-1");
    assert_eq!(incomplete[0].status, "pending");

    conn.execute(
        "DELETE FROM workflow_board_cards WHERE card_id = 'card-cas-del'",
        [],
    )?;
    for table in [
        "workflow_board_revisions",
        "workflow_board_approvals",
        "workflow_board_stage_runs",
        "workflow_board_write_journal",
    ] {
        let remaining: i64 =
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
        assert_eq!(remaining, 0, "expected cascade delete in {table}");
    }
    Ok(())
}
