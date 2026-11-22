//! Incremental core schema migrations `103..=105` applied during core DDL bootstrap.
//!
//! Mirrors `apps/server/src/persistence/Migrations/103_IssueTrackerAdditionalScopes.ts`,
//! `104_ProjectionProjectsTrackedFolders.ts`, and `105_TaskFolders.ts`.

use rusqlite::{Connection, params};

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

fn record_migration(conn: &Connection, migration_id: i64, name: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO effect_sql_migrations (migration_id, name) VALUES (?1, ?2)",
        params![migration_id, name],
    )?;
    Ok(())
}

/// Applies idempotent migrations `103..=105` and records them in `effect_sql_migrations`.
pub(crate) fn apply(conn: &Connection) -> rusqlite::Result<()> {
    // Migration 103: issue_tracker_bindings.additional_scopes
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS issue_tracker_bindings (
           project_id TEXT PRIMARY KEY,
           tracker TEXT NOT NULL,
           scope TEXT NOT NULL,
           updated_at TEXT NOT NULL
         ) WITHOUT ROWID;",
    )?;
    if !table_has_column(conn, "issue_tracker_bindings", "additional_scopes")? {
        conn.execute_batch(
            "ALTER TABLE issue_tracker_bindings
             ADD COLUMN additional_scopes TEXT NOT NULL DEFAULT '[]';",
        )?;
    }
    record_migration(conn, 103, "IssueTrackerAdditionalScopes")?;

    // Migration 104: projection_projects.tracked_folders_json (PRAGMA guarded)
    if !table_has_column(conn, "projection_projects", "tracked_folders_json")? {
        conn.execute_batch(
            "ALTER TABLE projection_projects
             ADD COLUMN tracked_folders_json TEXT NOT NULL DEFAULT '[]';",
        )?;
    }
    record_migration(conn, 104, "ProjectionProjectsTrackedFolders")?;

    // Migration 105: task_folders + idx_task_folders_real_root
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_folders (
           root TEXT PRIMARY KEY,
           real_root TEXT NOT NULL,
           project_id TEXT NOT NULL,
           project_root TEXT NOT NULL,
           anchor TEXT NOT NULL,
           project_path TEXT NOT NULL,
           project_is_worktree INTEGER NOT NULL,
           repositories_json TEXT NOT NULL DEFAULT '[]',
           created_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_task_folders_real_root
           ON task_folders (real_root);",
    )?;
    record_migration(conn, 105, "TaskFolders")?;

    Ok(())
}
