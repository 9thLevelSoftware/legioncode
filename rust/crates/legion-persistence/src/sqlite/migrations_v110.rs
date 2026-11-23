//! Incremental core schema migrations `106..=110` applied during core DDL bootstrap.
//!
//! Mirrors `apps/server/src/persistence/Migrations/106_WorktreePathKeyIndex.ts` through
//! `110_Workflows.ts`, plus `workflow_engine_journal` for Rust replay step journaling.

use rusqlite::{Connection, params};

use super::{schema_v2_indexes, schema_v2_triggers, schema_workflow_tables};

fn table_has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migration_recorded(conn: &Connection, migration_id: i64) -> rusqlite::Result<bool> {
    match conn.query_row(
        "SELECT 1 FROM effect_sql_migrations WHERE migration_id = ?1 LIMIT 1",
        [migration_id],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(err) => Err(err),
    }
}

fn record_migration(conn: &Connection, migration_id: i64, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO effect_sql_migrations (migration_id, name) VALUES (?1, ?2)",
        params![migration_id, name],
    )?;
    Ok(())
}

/// Applies idempotent migrations `106..=110` and records them in `effect_sql_migrations`.
pub(crate) fn apply(conn: &Connection) -> rusqlite::Result<()> {
    // Migration 106: normalized worktreePath lookup index.
    conn.execute_batch(schema_v2_indexes::WORKTREE_PATH_KEY_INDEX_SQL)?;
    record_migration(conn, 106, "WorktreePathKeyIndex")?;

    // Migration 107: drop and rebuild partial deferred slash command index with `GLOB '/*'`.
    if !migration_recorded(conn, 107)? {
        conn.execute_batch(&format!(
            "DROP INDEX IF EXISTS orchestration_v2_projection_messages_deferred_command_idx;\n{}",
            schema_v2_triggers::deferred_command_index_sql(false),
        ))?;
        record_migration(conn, 107, "DeferredSlashCommandIndex")?;
    }

    // Migration 108: agent_feedback + agent_feedback_run_idx.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_feedback (
           id TEXT PRIMARY KEY,
           thread_id TEXT,
           run_id TEXT,
           created_at TEXT NOT NULL,
           kind TEXT NOT NULL,
           subject TEXT NOT NULL,
           expected TEXT NOT NULL,
           actual TEXT NOT NULL,
           workaround TEXT NOT NULL,
           user_requested INTEGER NOT NULL DEFAULT 0,
           trace TEXT,
           source TEXT NOT NULL,
           sent INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS agent_feedback_run_idx
           ON agent_feedback(run_id, user_requested);",
    )?;
    record_migration(conn, 108, "AgentFeedback")?;

    // Migration 109: issue_tracker_accounts + issue_tracker_bindings.account_id.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS issue_tracker_accounts (
           tracker TEXT NOT NULL,
           account_id TEXT NOT NULL,
           created_at TEXT NOT NULL,
           PRIMARY KEY (tracker, account_id)
         ) WITHOUT ROWID;",
    )?;
    if !table_has_column(conn, "issue_tracker_bindings", "account_id")? {
        conn.execute_batch("ALTER TABLE issue_tracker_bindings ADD COLUMN account_id TEXT;")?;
    }
    record_migration(conn, 109, "IssueTrackerAccounts")?;

    // Migration 110: durable workflows + Rust replay step journal.
    schema_workflow_tables::create_workflow_tables(conn)?;
    schema_workflow_tables::create_workflow_indexes(conn)?;
    record_migration(conn, 110, "Workflows")?;

    Ok(())
}
