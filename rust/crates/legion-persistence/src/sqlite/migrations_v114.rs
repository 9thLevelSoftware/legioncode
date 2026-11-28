//! Migration 114 (`WorkflowBoardCliImport`): M4 `.legion-cli` import metadata and idempotency DDL.

use rusqlite::Connection;

use super::schema_workflow_board_m4::apply_m4_schema;

/// Applies Migration 114 idempotently and records it in `effect_sql_migrations`.
pub(super) fn apply(conn: &Connection) -> rusqlite::Result<()> {
    apply_m4_schema(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO effect_sql_migrations (migration_id, name) VALUES (114, 'WorkflowBoardCliImport')",
        [],
    )?;
    Ok(())
}
