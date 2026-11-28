//! Migration 113 (`WorkflowBoardExecuteShip`): M3 Execute, Review, Recovery, and Ship DDL.

use rusqlite::Connection;

use super::schema_workflow_board_m3::apply_m3_schema;

/// Applies Migration 113 idempotently and records it in `effect_sql_migrations`.
pub(super) fn apply(conn: &Connection) -> rusqlite::Result<()> {
    apply_m3_schema(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO effect_sql_migrations (migration_id, name) VALUES (113, 'WorkflowBoardExecuteShip')",
        [],
    )?;
    Ok(())
}
