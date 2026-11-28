//! SQLite DDL for Milestone 4 `.legion-cli` Project Import state (`BUILD_MIGRATION_HEAD = 114`).

use rusqlite::Connection;

const WORKFLOW_BOARD_M4_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS workflow_board_cli_imports (
  card_id TEXT PRIMARY KEY REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  project_id TEXT NOT NULL,
  source_cli_spec_id TEXT NOT NULL,
  source_cli_dir TEXT NOT NULL,
  source_digest TEXT NOT NULL,
  last_imported_revision INTEGER NOT NULL,
  ownership_owner TEXT NOT NULL DEFAULT 'legioncode' CHECK (ownership_owner = 'legioncode'),
  unsupported_policies_json TEXT NOT NULL DEFAULT '[]',
  acknowledged_policies_json TEXT NOT NULL DEFAULT '[]',
  policies_acknowledged_at TEXT,
  assurance_policy_json TEXT,
  import_provenance_json TEXT NOT NULL,
  imported_at TEXT NOT NULL,
  last_synced_at TEXT NOT NULL,
  UNIQUE (project_id, source_cli_spec_id)
);
";

const WORKFLOW_BOARD_M4_INDEXES_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_workflow_board_cli_imports_project
  ON workflow_board_cli_imports (project_id, source_cli_spec_id);
";

/// Applies M4 `.legion-cli` import tables and indexes idempotently.
pub(crate) fn apply_m4_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(WORKFLOW_BOARD_M4_TABLES_SQL)?;
    conn.execute_batch(WORKFLOW_BOARD_M4_INDEXES_SQL)?;
    Ok(())
}
