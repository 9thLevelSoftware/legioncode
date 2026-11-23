//! Schema migrations `075` guard (`#30783`) and `106..=110` tests (`#36802`, `#30261`, `#40316`,
//! `#35871`, `#39179`, `#40587`, `#38882`).

use legion_persistence::issue_tracker_store::{
    IssueTrackerBindingEntry, add_issue_tracker_account, get_issue_tracker_binding,
    list_issue_tracker_accounts, list_issue_tracker_bindings, remove_issue_tracker_account,
    set_issue_tracker_binding,
};
use legion_persistence::sqlite::{
    BUILD_MIGRATION_HEAD, SqliteStore, deferred_command_message_condition, read_recorded_migrations,
};
use legion_persistence::worktree_lookup::{
    list_thread_ids_by_worktree_path, list_threads_by_worktree_path,
    normalize_project_path_for_comparison,
};
use rusqlite::{Connection, params};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn guarded_migration_075_survives_corrupt_subagent_json_and_backfills_valid_rows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("pre075.sqlite");

    // Pre-create a partial schema up to migration 74 with unreadable/corrupt subagent payloads.
    let pre = Connection::open(&db_path)?;
    pre.execute_batch(
        "CREATE TABLE effect_sql_migrations (
           migration_id INTEGER PRIMARY KEY NOT NULL,
           created_at DATETIME NOT NULL DEFAULT current_timestamp,
           name TEXT NOT NULL
         );
         INSERT INTO effect_sql_migrations (migration_id, name) VALUES (74, 'Previous');
         CREATE TABLE orchestration_v2_projection_subagents (
           subagent_id TEXT PRIMARY KEY,
           thread_id TEXT NOT NULL,
           run_id TEXT,
           parent_node_id TEXT NOT NULL,
           provider TEXT NOT NULL,
           provider_thread_id TEXT,
           child_thread_id TEXT,
           origin TEXT NOT NULL,
           status TEXT NOT NULL,
           started_at TEXT,
           completed_at TEXT,
           updated_at TEXT NOT NULL,
           payload_json TEXT NOT NULL
         );
         INSERT INTO orchestration_v2_projection_subagents
           (subagent_id, thread_id, parent_node_id, provider, origin, status, updated_at, payload_json)
         VALUES
           ('sa-valid', 't-1', 'n-1', 'codex', 'spawn', 'running', '2026-01-01T00:00:00Z', '{\"completionDelivery\":{\"state\":\"pending\"}}'),
           ('sa-corrupt', 't-1', 'n-1', 'codex', 'spawn', 'running', '2026-01-01T00:00:00Z', '{bad json'),
           ('sa-empty', 't-1', 'n-1', 'codex', 'spawn', 'running', '2026-01-01T00:00:00Z', ''),
           ('sa-plain', 't-1', 'n-1', 'codex', 'spawn', 'running', '2026-01-01T00:00:00Z', '{\"status\":\"done\"}');",
    )?;
    pre.close().map_err(|(_, e)| e)?;

    let store = SqliteStore::open(&db_path)?;
    assert_eq!(store.recorded_migration_head()?, Some(BUILD_MIGRATION_HEAD));
    let conn = store.connection();

    let state_valid: Option<String> = conn.query_row(
        "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id = 'sa-valid'",
        [],
        |r| r.get(0),
    )?;
    let state_corrupt: Option<String> = conn.query_row(
        "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id = 'sa-corrupt'",
        [],
        |r| r.get(0),
    )?;
    let state_empty: Option<String> = conn.query_row(
        "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id = 'sa-empty'",
        [],
        |r| r.get(0),
    )?;
    let state_plain: Option<String> = conn.query_row(
        "SELECT completion_delivery_state FROM orchestration_v2_projection_subagents WHERE subagent_id = 'sa-plain'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(state_valid.as_deref(), Some("pending"));
    assert_eq!(state_corrupt, None);
    assert_eq!(state_empty, None);
    assert_eq!(state_plain, None);
    Ok(())
}

#[test]
fn migration_106_worktree_path_key_index_accelerates_windows_and_posix_lookups() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open(dir.path().join("statev2.sqlite"))?;
    let conn = store.connection();

    let insert_thread = |id: &str,
                         worktree: Option<&str>,
                         deleted_at: Option<&str>|
     -> rusqlite::Result<()> {
        let payload = worktree.map_or_else(
            || "{}".to_owned(),
            |wt| serde_json::json!({ "worktreePath": wt }).to_string(),
        );
        conn.execute(
            "INSERT INTO orchestration_v2_projection_threads (
               thread_id, project_id, title, default_provider, runtime_mode, interaction_mode,
               created_at, updated_at, deleted_at, payload_json
             ) VALUES (?1, 'proj-1', 'T', 'codex', 'full-access', 'default', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?3, ?2)",
            params![id, payload, deleted_at],
        )?;
        Ok(())
    };
    insert_thread("t-win", Some(r"C:\Users\Alice\Repo\"), None)?;
    insert_thread(
        "t-win-del",
        Some(r"C:\Users\Alice\Repo\"),
        Some("2026-01-02T00:00:00Z"),
    )?;
    insert_thread("t-posix-upper", Some("/Users/Alice/Repo/"), None)?;
    insert_thread("t-posix-lower", Some("/users/alice/repo/"), None)?;
    insert_thread("t-none", None, None)?;

    assert_eq!(
        normalize_project_path_for_comparison("c:/users/alice/repo/"),
        normalize_project_path_for_comparison(r"C:\Users\Alice\Repo\")
    );
    assert_eq!(
        list_thread_ids_by_worktree_path(conn, "c:/users/alice/repo", false)?,
        vec!["t-win".to_owned()]
    );
    assert_eq!(
        list_thread_ids_by_worktree_path(conn, "c:/users/alice/repo", true)?,
        vec!["t-win".to_owned(), "t-win-del".to_owned()]
    );
    assert_eq!(
        list_thread_ids_by_worktree_path(conn, "/Users/Alice/Repo", false)?,
        vec!["t-posix-upper".to_owned()]
    );
    assert_eq!(
        list_threads_by_worktree_path(conn, "/users/alice/repo", false)?.len(),
        1
    );

    let plan_sql = "EXPLAIN QUERY PLAN
         SELECT thread_id FROM orchestration_v2_projection_threads
         WHERE deleted_at IS NULL
           AND CASE WHEN json_valid(payload_json)
             THEN lower(rtrim(replace(trim(json_extract(payload_json, '$.worktreePath')), '\\', '/'), '/'))
           END = lower(rtrim(replace(trim(?1), '\\', '/'), '/'))";
    let details: Vec<String> = conn
        .prepare(plan_sql)?
        .query_map(["/Users/Alice/Repo"], |row| row.get::<_, String>(3))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        details
            .iter()
            .any(|d| d.contains("orchestration_v2_projection_threads_worktree_path_key_idx")),
        "expected query plan to use worktree_path_key_idx, got: {details:?}"
    );
    Ok(())
}

#[test]
fn migration_107_rebuilds_deferred_slash_command_index_for_all_slash_commands() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("upgrade107.sqlite");

    // Simulate a pre-107 DB stopped at 106 with the old narrow /compact|/goal index.
    let store = SqliteStore::open(&db_path)?;
    {
        let conn = store.connection();
        conn.execute_batch(
            "DROP INDEX orchestration_v2_projection_messages_deferred_command_idx;
             CREATE INDEX orchestration_v2_projection_messages_deferred_command_idx
               ON orchestration_v2_projection_messages (thread_id, created_at, message_id)
               WHERE role = 'user' AND lower(ltrim(json_extract(payload_json, '$.text'))) LIKE '/compact%';
             DELETE FROM effect_sql_migrations WHERE migration_id >= 107;",
        )?;
    }
    store.close()?;

    let upgraded = SqliteStore::open(&db_path)?;
    assert_eq!(
        upgraded.recorded_migration_head()?,
        Some(BUILD_MIGRATION_HEAD)
    );
    let conn = upgraded.connection();

    let insert_msg = |id: &str, role: &str, text: &str| -> rusqlite::Result<()> {
        let payload = serde_json::json!({ "text": text }).to_string();
        conn.execute(
            "INSERT INTO orchestration_v2_projection_messages
               (message_id, thread_id, run_id, role, streaming, created_at, updated_at, payload_json)
             VALUES (?1, 't-1', 'run-1', ?2, 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:01Z', ?3)",
            params![id, role, payload],
        )?;
        Ok(())
    };
    insert_msg("m-goal", "user", "/goal fix tests")?;
    insert_msg("m-plan", "user", " \n/plan refactor")?;
    insert_msg("m-review", "user", "/review")?;
    insert_msg("m-text", "user", "normal text")?;
    insert_msg("m-assistant", "assistant", "/goal ignored")?;

    let predicate = deferred_command_message_condition();
    let query = format!(
        "SELECT message_id FROM orchestration_v2_projection_messages WHERE thread_id = 't-1' AND {predicate} ORDER BY created_at, message_id"
    );
    let matched: Vec<String> = conn
        .prepare(&query)?
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(matched, vec!["m-goal", "m-plan", "m-review"]);

    let explain_sql = format!("EXPLAIN QUERY PLAN {query}");
    let details: Vec<String> = conn
        .prepare(&explain_sql)?
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        details
            .iter()
            .any(|d| d.contains("orchestration_v2_projection_messages_deferred_command_idx")),
        "expected deferred command partial index hit, got: {details:?}"
    );
    Ok(())
}

#[test]
fn migrations_108_109_110_create_feedback_accounts_and_workflow_tables_and_helpers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("statev2.sqlite");
    let store = SqliteStore::open(&db_path)?;
    let conn = store.connection();

    let migrations = read_recorded_migrations(&db_path).ok_or("missing recorded migrations")?;
    for (id, name) in [
        (106, "WorktreePathKeyIndex"),
        (107, "DeferredSlashCommandIndex"),
        (108, "AgentFeedback"),
        (109, "IssueTrackerAccounts"),
        (110, "Workflows"),
    ] {
        assert!(
            migrations
                .iter()
                .any(|m| m.migration_id == id && m.name == name),
            "missing recorded migration {id}_{name}"
        );
    }

    // 108: agent_feedback + agent_feedback_run_idx
    conn.execute(
        "INSERT INTO agent_feedback (id, created_at, run_id, kind, subject, expected, actual, workaround, source)
         VALUES ('fb-1', '2026-01-01T00:00:00Z', 'run-1', 'bug', 'ui', 'fast', 'slow', 'none', 'agent')",
        [],
    )?;
    let (sent, run_id): (i64, String) = conn.query_row(
        "SELECT sent, run_id FROM agent_feedback WHERE id = 'fb-1'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(sent, 0);
    assert_eq!(run_id, "run-1");

    // 109: issue_tracker_accounts & selected-project binding query tolerating corrupt sibling rows
    add_issue_tracker_account(conn, "linear", "acct-1", "2026-01-01T00:00:00Z")?;
    add_issue_tracker_account(conn, "linear", "acct-2", "2026-01-01T00:01:00Z")?;
    assert_eq!(list_issue_tracker_accounts(conn)?.len(), 2);

    set_issue_tracker_binding(
        conn,
        "proj-ok",
        Some(&IssueTrackerBindingEntry {
            project_id: "proj-ok".to_owned(),
            tracker: "linear".to_owned(),
            account_id: Some("acct-1".to_owned()),
            scope: "ENG".to_owned(),
            additional_scopes: vec!["OPS".to_owned()],
        }),
        "2026-01-01T00:02:00Z",
    )?;
    conn.execute(
        "INSERT INTO issue_tracker_bindings (project_id, tracker, scope, updated_at, additional_scopes, account_id)
         VALUES ('proj-bad', 'linear', 'BAD', '2026-01-01T00:02:00Z', '{corrupt', 'acct-2')",
        [],
    )?;
    let selected = get_issue_tracker_binding(conn, "proj-ok")?;
    assert_eq!(
        selected.as_ref().and_then(|b| b.account_id.as_deref()),
        Some("acct-1")
    );
    assert_eq!(list_issue_tracker_bindings(conn, Some("proj-ok"))?.len(), 1);
    assert!(list_issue_tracker_bindings(conn, None).is_err());

    remove_issue_tracker_account(conn, "linear", Some("acct-1"))?;
    assert_eq!(get_issue_tracker_binding(conn, "proj-ok")?, None);
    assert_eq!(list_issue_tracker_accounts(conn)?.len(), 1);

    // 110: all 10 workflow tables + workflow_engine_journal + 7 indexes
    for table in [
        "workflow_runs",
        "workflow_steps",
        "workflow_waits",
        "workflow_mail",
        "workflow_marks",
        "workflow_signals",
        "workflow_triggers",
        "workflow_hooks",
        "workflow_store",
        "workflow_run_references",
        "workflow_engine_journal",
    ] {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1, "missing workflow table {table}");
    }
    conn.execute(
        "INSERT INTO workflow_runs (run_id, workflow, version, engine_tag, idempotency_key, title, status, started_at, updated_at)
         VALUES ('wfr-1', 'wf', 1, 'v1', 'idem-1', 'Run 1', 'running', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO workflow_engine_journal (run_id, step_name, action_key, outcome, payload_json, created_at, updated_at)
         VALUES ('wfr-1', 'step-a', 'act-1', 'completed', '{\"ok\":true}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )?;
    Ok(())
}
