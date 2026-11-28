//! Incremental schema migration `112` for Milestone 2 Workflow Board Planning state.

use rusqlite::{Connection, params};

use super::schema_workflow_board_m2;

/// Applies migration `112` (`WorkflowBoardPlanning`) idempotently and records it in `effect_sql_migrations`.
pub(crate) fn apply(conn: &Connection) -> rusqlite::Result<()> {
    schema_workflow_board_m2::apply_m2_schema(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO effect_sql_migrations (migration_id, name) VALUES (?1, ?2)",
        params![112_i64, "WorkflowBoardPlanning"],
    )?;
    Ok(())
}
