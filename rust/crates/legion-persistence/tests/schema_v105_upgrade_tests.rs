//! Legacy Migration-102 and partial DB upgrade integration tests.

use std::collections::HashSet;

use legion_persistence::sqlite::{BUILD_MIGRATION_HEAD, SqliteStore};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn columns_of(conn: &Connection, table: &str) -> Result<HashSet<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut cols = HashSet::new();
    for col in rows {
        cols.insert(col?);
    }
    Ok(cols)
}

const PRE_102_FIXTURE_SQL: &str = "CREATE TABLE effect_sql_migrations (
           migration_id integer PRIMARY KEY NOT NULL,
           created_at datetime NOT NULL DEFAULT current_timestamp,
           name VARCHAR(255) NOT NULL
         );
         INSERT INTO effect_sql_migrations (migration_id, name) VALUES (102, 'ThreadItemCounts');
         CREATE TABLE projection_projects (
           project_id TEXT PRIMARY KEY,
           title TEXT NOT NULL,
           workspace_root TEXT NOT NULL,
           scripts_json TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           deleted_at TEXT
         );
         INSERT INTO projection_projects VALUES ('p1', 'Proj', '/tmp/p1', '[]', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z', NULL);
         CREATE TABLE orchestration_events (
           sequence INTEGER PRIMARY KEY AUTOINCREMENT,
           event_id TEXT NOT NULL UNIQUE,
           aggregate_kind TEXT NOT NULL,
           stream_id TEXT NOT NULL,
           stream_version INTEGER NOT NULL,
           event_type TEXT NOT NULL,
           occurred_at TEXT NOT NULL,
           command_id TEXT,
           causation_event_id TEXT,
           correlation_id TEXT,
           actor_kind TEXT NOT NULL,
           payload_json TEXT NOT NULL,
           metadata_json TEXT NOT NULL
         );
         CREATE TABLE orchestration_v2_events (
           sequence INTEGER PRIMARY KEY AUTOINCREMENT,
           event_id TEXT NOT NULL UNIQUE,
           command_id TEXT,
           thread_id TEXT NOT NULL,
           run_id TEXT,
           node_id TEXT,
           provider TEXT,
           raw_event_id TEXT,
           event_type TEXT NOT NULL,
           occurred_at TEXT NOT NULL,
           payload_json TEXT NOT NULL
         );
         CREATE TABLE orchestration_v2_projection_runs (
           run_id TEXT PRIMARY KEY,
           thread_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL,
           provider TEXT NOT NULL,
           provider_thread_id TEXT,
           status TEXT NOT NULL,
           requested_at TEXT NOT NULL,
           completed_at TEXT,
           payload_json TEXT NOT NULL
         );
         INSERT INTO orchestration_v2_projection_runs VALUES ('r1', 't1', 1, 'codex', NULL, 'completed', '2026-10-01T00:00:00Z', NULL, '{}');
         CREATE TABLE orchestration_v2_projection_turn_items (
           turn_item_id TEXT PRIMARY KEY,
           thread_id TEXT NOT NULL,
           run_id TEXT,
           node_id TEXT,
           provider_thread_id TEXT,
           provider_turn_id TEXT,
           parent_item_id TEXT,
           ordinal INTEGER NOT NULL,
           type TEXT NOT NULL,
           status TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           payload_json TEXT NOT NULL
         );
         INSERT INTO orchestration_v2_projection_turn_items VALUES ('i1', 't1', 'r1', NULL, NULL, NULL, NULL, 1, 'user_message', 'completed', '2026-10-01T00:00:00Z', '{}');
         CREATE TABLE orchestration_v2_projection_messages (
           message_id TEXT PRIMARY KEY,
           thread_id TEXT NOT NULL,
           run_id TEXT,
           node_id TEXT,
           role TEXT NOT NULL,
           streaming INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           payload_json TEXT NOT NULL
         );
         CREATE TABLE scheduled_tasks (
           task_id TEXT PRIMARY KEY,
           title TEXT NOT NULL,
           prompt TEXT NOT NULL,
           enabled INTEGER NOT NULL,
           schedule_json TEXT NOT NULL,
           project_id TEXT NOT NULL,
           thread_id TEXT,
           workspace_strategy_json TEXT NOT NULL,
           model_selection_json TEXT NOT NULL,
           runtime_mode TEXT NOT NULL,
           interaction_mode TEXT NOT NULL,
           created_by TEXT NOT NULL,
           creation_source TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           next_run_at TEXT,
           last_run_at TEXT,
           last_run_status TEXT NOT NULL,
           last_run_error TEXT,
           run_count INTEGER NOT NULL
         );
         CREATE TABLE issue_tracker_bindings (
           project_id TEXT PRIMARY KEY,
           tracker TEXT NOT NULL,
           scope TEXT NOT NULL,
           updated_at TEXT NOT NULL
         ) WITHOUT ROWID;";

#[test]
fn opens_node_migration_102_and_partial_legacy_dbs_with_column_upgrades() -> TestResult {
    let temp = tempfile::tempdir()?;
    let db_path = temp.path().join("node_m102.sqlite");
    let raw = Connection::open(&db_path)?;
    raw.execute_batch(PRE_102_FIXTURE_SQL)?;
    raw.close().map_err(|(_, e)| e)?;

    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();
    assert_eq!(store.recorded_migration_head()?, Some(BUILD_MIGRATION_HEAD));
    assert!(columns_of(conn, "orchestration_events")?.contains("application_event_version"));
    assert!(columns_of(conn, "orchestration_v2_projection_messages")?.contains("search_text"));
    assert!(
        columns_of(conn, "orchestration_v2_projection_turn_items")?.contains("inline_work_output")
    );
    assert!(columns_of(conn, "scheduled_tasks")?.contains("thread_mode"));
    assert!(columns_of(conn, "issue_tracker_bindings")?.contains("additional_scopes"));
    assert!(columns_of(conn, "projection_projects")?.contains("tracked_folders_json"));

    let backfilled_instance: Option<String> = conn.query_row(
        "SELECT provider_instance_id FROM orchestration_v2_projection_runs WHERE run_id = 'r1'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(backfilled_instance.as_deref(), Some("codex"));

    let backfilled_count: i64 = conn.query_row(
        "SELECT item_count FROM orchestration_v2_projection_thread_item_counts WHERE thread_id = 't1'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(backfilled_count, 1);

    let title: String = conn.query_row(
        "SELECT title FROM projection_projects WHERE project_id = 'p1'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(title, "Proj");
    Ok(())
}
