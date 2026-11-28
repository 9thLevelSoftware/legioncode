//! SQLite DDL for Milestone 3 Workflow Board Execute & Ship state (`BUILD_MIGRATION_HEAD = 113`).

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension as _};

const CARD_M3_COLUMNS: &[(&str, &str)] = &[
    ("product_snapshot_digest", "TEXT"),
    (
        "paused_from_status",
        "TEXT CHECK (paused_from_status IS NULL OR paused_from_status IN ('active', 'blocked'))",
    ),
    ("pause_reason", "TEXT"),
    ("paused_at", "TEXT"),
    (
        "external_edit_state",
        "TEXT CHECK (external_edit_state IS NULL OR external_edit_state IN ('clean', 'deliverable_stale', 'worktree_dirty', 'reconciliation_required'))",
    ),
    ("external_edit_detail_json", "TEXT"),
];

const TASK_M3_COLUMNS: &[(&str, &str)] = &[
    ("active_stage_run_id", "TEXT"),
    ("started_at", "TEXT"),
    ("completed_at", "TEXT"),
    ("attempt_count", "INTEGER NOT NULL DEFAULT 0"),
    ("last_error", "TEXT"),
    ("last_diff_digest", "TEXT"),
];

const PLANNED_CHECK_M3_COLUMNS: &[(&str, &str)] = &[
    ("stale_reason", "TEXT"),
    ("last_receipt_id", "TEXT"),
    ("last_run_at", "TEXT"),
    ("last_duration_ms", "INTEGER"),
    ("last_output_excerpt", "TEXT"),
];

const AC_EVIDENCE_M3_COLUMNS: &[(&str, &str)] =
    &[("stale_reason", "TEXT"), ("external_source", "TEXT")];

const WORKFLOW_BOARD_M3_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS workflow_board_contract_violations (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  stage_run_id TEXT,
  thread_id TEXT,
  workflow_run_id TEXT,
  card_revision INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'violation_pending' CHECK (status IN ('clean', 'violation_pending', 'resolved', 'amended', 'stale')),
  changed_files_json TEXT NOT NULL DEFAULT '[]',
  allowed_files_json TEXT NOT NULL DEFAULT '[]',
  unexpected_files_json TEXT NOT NULL DEFAULT '[]',
  forbidden_files_json TEXT NOT NULL DEFAULT '[]',
  preserved_in_worktree INTEGER NOT NULL DEFAULT 1 CHECK (preserved_in_worktree IN (0, 1)),
  blocking_reason TEXT,
  diff_digest TEXT,
  resolution_disposition TEXT CHECK (resolution_disposition IS NULL OR resolution_disposition IN ('amend_plan', 'accept_scope', 'manual_reverted', 'dismiss')),
  resolution_note TEXT,
  resolved_by TEXT,
  resolved_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_board_check_receipts (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  check_id TEXT NOT NULL,
  criterion_id TEXT,
  task_id TEXT,
  card_revision INTEGER NOT NULL,
  argv_json TEXT NOT NULL,
  snapshot_digest TEXT,
  commit_sha TEXT,
  spec_digest TEXT,
  plan_digest TEXT,
  status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'passed', 'failed', 'stale', 'historical')),
  exit_code INTEGER,
  log_ref TEXT,
  stdout_excerpt TEXT,
  stderr_excerpt TEXT,
  duration_ms INTEGER,
  stale_reason TEXT,
  executed_at TEXT,
  invalidated_at TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_board_review_runs (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  stage_run_id TEXT REFERENCES workflow_board_stage_runs(id) ON DELETE SET NULL,
  thread_id TEXT,
  card_revision INTEGER NOT NULL,
  snapshot_digest TEXT,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'completed', 'failed', 'stale', 'historical')),
  verdict TEXT NOT NULL DEFAULT 'pending' CHECK (verdict IN ('pending', 'pass', 'findings', 'failed', 'stale')),
  summary TEXT,
  stale_reason TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_board_review_findings (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  review_run_id TEXT NOT NULL REFERENCES workflow_board_review_runs(id) ON DELETE CASCADE,
  severity TEXT NOT NULL CHECK (severity IN ('critical', 'major', 'minor', 'info')),
  criterion_id TEXT,
  file_path TEXT,
  line INTEGER,
  message TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'stale', 'historical')),
  disposition TEXT NOT NULL DEFAULT 'unresolved' CHECK (disposition IN ('unresolved', 'fix', 'accept_risk', 'dismiss', 'defer')),
  resolution_note TEXT,
  resolved_by TEXT,
  resolved_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_board_amendments (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  card_revision INTEGER NOT NULL,
  target_stage TEXT NOT NULL DEFAULT 'plan' CHECK (target_stage IN ('spec', 'plan')),
  status TEXT NOT NULL DEFAULT 'proposed' CHECK (status IN ('proposed', 'applied', 'superseded', 'rejected')),
  reason TEXT NOT NULL,
  summary TEXT,
  source_task_id TEXT,
  unexpected_files_json TEXT NOT NULL DEFAULT '[]',
  proposed_changes_json TEXT NOT NULL DEFAULT '{}',
  proposed_by TEXT NOT NULL DEFAULT 'client',
  resolved_by TEXT,
  resolved_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_board_external_actions (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  action_kind TEXT NOT NULL CHECK (action_kind IN ('task_turn', 'planned_check', 'review_turn', 'git_commit', 'git_push', 'git_create_pr', 'deliverable_sync', 'external_edit')),
  target_id TEXT,
  stage_run_id TEXT,
  idempotency_key TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('in_progress', 'interrupted', 'reconciled', 'completed', 'failed', 'cancelled')),
  expected_snapshot_digest TEXT,
  observed_snapshot_digest TEXT,
  details_json TEXT NOT NULL DEFAULT '{}',
  reconciliation_note TEXT,
  reconciled_by TEXT,
  reconciled_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (card_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS workflow_board_delivery_receipts (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  approval_id TEXT REFERENCES workflow_board_approvals(id) ON DELETE SET NULL,
  status TEXT NOT NULL CHECK (status IN ('shipped', 'valid', 'stale', 'superseded', 'historical')),
  card_revision INTEGER NOT NULL,
  spec_digest TEXT,
  plan_digest TEXT,
  tasks_digest TEXT,
  checks_digest TEXT,
  product_snapshot_digest TEXT NOT NULL,
  commit_sha TEXT,
  diff_digest TEXT,
  review_id TEXT,
  ac_verdicts_summary_json TEXT NOT NULL DEFAULT '{}',
  git_actions_json TEXT NOT NULL DEFAULT '{}',
  delivery_note TEXT,
  approved_by TEXT NOT NULL,
  approved_at TEXT NOT NULL,
  invalidated_at TEXT,
  invalidation_reason TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
";

const WORKFLOW_BOARD_M3_INDEXES_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_workflow_board_contract_violations_card
  ON workflow_board_contract_violations (card_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_check_receipts_card_check
  ON workflow_board_check_receipts (card_id, check_id, executed_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_check_receipts_card_status
  ON workflow_board_check_receipts (card_id, status);

CREATE INDEX IF NOT EXISTS idx_workflow_board_review_runs_card
  ON workflow_board_review_runs (card_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_review_findings_card
  ON workflow_board_review_findings (card_id, status, review_run_id);

CREATE INDEX IF NOT EXISTS idx_workflow_board_amendments_card
  ON workflow_board_amendments (card_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_external_actions_card
  ON workflow_board_external_actions (card_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_delivery_receipts_card
  ON workflow_board_delivery_receipts (card_id, approved_at DESC, id DESC);
";

fn ensure_columns(
    conn: &Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info('{table}')"))?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    if existing.is_empty() {
        return Ok(());
    }
    for (col_name, col_def) in columns {
        if !existing.contains(*col_name) {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {col_name} {col_def};"
            ))?;
        }
    }
    Ok(())
}

const CARDS_M3_REBUILD_SQL: &str = "DROP TABLE IF EXISTS workflow_board_cards_m3_new;
CREATE TABLE workflow_board_cards_m3_new (
  card_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  stage TEXT NOT NULL CHECK (stage IN ('spec', 'plan', 'execute', 'ship')),
  status TEXT NOT NULL CHECK (status IN ('active', 'blocked', 'paused', 'completed', 'shipped')),
  revision INTEGER NOT NULL DEFAULT 1,
  brief TEXT NOT NULL DEFAULT '',
  source_type TEXT NOT NULL DEFAULT 'idea',
  assistance_mode TEXT NOT NULL DEFAULT 'balanced',
  workspace_strategy_json TEXT NOT NULL DEFAULT '{\"type\":\"root\"}',
  worktree_path TEXT,
  branch TEXT,
  model_selection_json TEXT,
  runtime_mode TEXT,
  interaction_mode TEXT,
  spec_digest TEXT,
  plan_digest TEXT,
  current_task_id TEXT,
  blocker_reason TEXT,
  next_action TEXT,
  spec_json TEXT NOT NULL DEFAULT '{}',
  plan_json TEXT,
  import_provenance_json TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  archived_at TEXT,
  intake_goal TEXT,
  brief_path TEXT,
  brief_digest TEXT,
  input_paths_json TEXT NOT NULL DEFAULT '[]',
  wireframes_digest TEXT,
  design_digest TEXT,
  tasks_digest TEXT,
  product_snapshot_digest TEXT,
  paused_from_status TEXT CHECK (paused_from_status IS NULL OR paused_from_status IN ('active', 'blocked')),
  pause_reason TEXT,
  paused_at TEXT,
  external_edit_state TEXT CHECK (external_edit_state IS NULL OR external_edit_state IN ('clean', 'deliverable_stale', 'worktree_dirty', 'reconciliation_required')),
  external_edit_detail_json TEXT
);
INSERT INTO workflow_board_cards_m3_new (
  card_id, project_id, title, stage, status, revision, brief, source_type,
  assistance_mode, workspace_strategy_json, worktree_path, branch,
  model_selection_json, runtime_mode, interaction_mode, spec_digest, plan_digest,
  current_task_id, blocker_reason, next_action, spec_json, plan_json,
  import_provenance_json, metadata_json, created_at, updated_at, completed_at,
  archived_at, intake_goal, brief_path, brief_digest, input_paths_json,
  wireframes_digest, design_digest, tasks_digest
)
SELECT
  card_id, project_id, title, stage, status, revision, brief, source_type,
  assistance_mode, workspace_strategy_json, worktree_path, branch,
  model_selection_json, runtime_mode, interaction_mode, spec_digest, plan_digest,
  current_task_id, blocker_reason, next_action, spec_json, plan_json,
  import_provenance_json, metadata_json, created_at, updated_at, completed_at,
  archived_at, intake_goal, brief_path, brief_digest, input_paths_json,
  wireframes_digest, design_digest, tasks_digest
FROM workflow_board_cards;
DROP TABLE workflow_board_cards;
ALTER TABLE workflow_board_cards_m3_new RENAME TO workflow_board_cards;
CREATE INDEX IF NOT EXISTS idx_workflow_board_cards_project_updated
  ON workflow_board_cards (project_id, updated_at DESC, card_id);
CREATE INDEX IF NOT EXISTS idx_workflow_board_cards_project_stage
  ON workflow_board_cards (project_id, stage, status);";

fn rebuild_cards_table_for_shipped_status(conn: &Connection) -> rusqlite::Result<()> {
    let cards_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'workflow_board_cards'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(sql) = cards_sql else {
        return Ok(());
    };
    if sql.contains("'shipped'") {
        return ensure_columns(conn, "workflow_board_cards", CARD_M3_COLUMNS);
    }

    let fk_was_on: bool = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
        .unwrap_or(1)
        != 0;
    if fk_was_on && !conn.is_autocommit() {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
            Some(
                "workflow_board_cards v113 rebuild requires autocommit when foreign_keys=ON"
                    .to_owned(),
            ),
        ));
    }
    if fk_was_on {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }
    let started_tx = conn.is_autocommit();
    if started_tx && let Err(err) = conn.execute_batch("BEGIN IMMEDIATE;") {
        if fk_was_on && conn.is_autocommit() {
            let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
        }
        return Err(err);
    }

    let res = (|| -> rusqlite::Result<()> {
        conn.execute_batch(CARDS_M3_REBUILD_SQL)?;

        let fk_violation: Option<String> = conn
            .query_row("PRAGMA foreign_key_check", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        if let Some(table) = fk_violation {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some(format!(
                    "foreign_key_check failed on table `{table}` after workflow_board_cards rebuild"
                )),
            ));
        }
        Ok(())
    })();

    if started_tx {
        if res.is_ok() {
            conn.execute_batch("COMMIT;")?;
        } else {
            let _ = conn.execute_batch("ROLLBACK;");
        }
    }
    if fk_was_on && conn.is_autocommit() {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    }
    res
}

/// Applies M3 workflow board `cards` rebuild, child columns, 7 M3 tables, and 8 indexes idempotently.
pub(crate) fn apply_m3_schema(conn: &Connection) -> rusqlite::Result<()> {
    rebuild_cards_table_for_shipped_status(conn)?;
    ensure_columns(conn, "workflow_board_tasks", TASK_M3_COLUMNS)?;
    ensure_columns(
        conn,
        "workflow_board_planned_checks",
        PLANNED_CHECK_M3_COLUMNS,
    )?;
    ensure_columns(conn, "workflow_board_ac_evidence", AC_EVIDENCE_M3_COLUMNS)?;
    conn.execute_batch(WORKFLOW_BOARD_M3_TABLES_SQL)?;
    conn.execute_batch(WORKFLOW_BOARD_M3_INDEXES_SQL)?;
    Ok(())
}
