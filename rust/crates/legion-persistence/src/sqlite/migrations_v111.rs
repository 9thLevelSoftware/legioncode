//! Incremental schema migration `111` for Project Workflow Board persistence.

use rusqlite::{Connection, params};

use super::schema_workflow_board;

/// Applies migration `111` (`WorkflowBoard`) idempotently and records it in `effect_sql_migrations`.
pub(crate) fn apply(conn: &Connection) -> rusqlite::Result<()> {
    schema_workflow_board::create_workflow_board_tables(conn)?;
    schema_workflow_board::create_workflow_board_indexes(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO effect_sql_migrations (migration_id, name) VALUES (?1, ?2)",
        params![111_i64, "WorkflowBoard"],
    )?;
    Ok(())
}
