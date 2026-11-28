//! SQLite DDL for the Project Workflow Board (`BUILD_MIGRATION_HEAD = 111`).

use rusqlite::Connection;

const WORKFLOW_BOARD_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS workflow_board_cards (
  card_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  stage TEXT NOT NULL CHECK (stage IN ('spec', 'plan', 'execute', 'ship')),
  status TEXT NOT NULL CHECK (status IN ('active', 'blocked', 'paused', 'completed')),
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
  archived_at TEXT
);

CREATE TABLE IF NOT EXISTS workflow_board_revisions (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  revision INTEGER NOT NULL,
  stage TEXT NOT NULL,
  kind TEXT NOT NULL,
  actor_kind TEXT NOT NULL DEFAULT 'client',
  summary TEXT NOT NULL DEFAULT '',
  spec_digest TEXT,
  plan_digest TEXT,
  payload_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  UNIQUE (card_id, revision)
);

CREATE TABLE IF NOT EXISTS workflow_board_approvals (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  gate TEXT NOT NULL CHECK (gate IN ('spec', 'plan', 'delivery')),
  status TEXT NOT NULL CHECK (status IN ('valid', 'stale', 'superseded', 'historical')),
  revision INTEGER NOT NULL,
  artifact_digest TEXT NOT NULL,
  spec_digest TEXT,
  plan_digest TEXT,
  tasks_digest TEXT,
  wireframes_digest TEXT,
  product_snapshot_digest TEXT,
  approved_by TEXT NOT NULL DEFAULT 'client',
  approved_at TEXT NOT NULL,
  invalidated_at TEXT,
  invalidation_reason TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS workflow_board_stage_runs (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  stage TEXT NOT NULL CHECK (stage IN ('spec', 'plan', 'execute', 'review', 'ship')),
  task_id TEXT,
  attempt INTEGER NOT NULL DEFAULT 1,
  stable_launch_key TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL CHECK (
    status IN ('pending', 'launching', 'running', 'paused', 'completed', 'failed', 'cancelled')
  ),
  card_revision INTEGER NOT NULL,
  provider_instance_id TEXT NOT NULL,
  model TEXT NOT NULL,
  model_options_json TEXT,
  runtime_mode TEXT NOT NULL,
  interaction_mode TEXT NOT NULL,
  workspace_strategy_json TEXT NOT NULL DEFAULT '{\"type\":\"root\"}',
  worktree_path TEXT,
  branch TEXT,
  thread_id TEXT,
  workflow_run_id TEXT,
  brief_digest TEXT,
  result_json TEXT,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  ended_at TEXT
);

CREATE TABLE IF NOT EXISTS workflow_board_write_journal (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  target_path TEXT NOT NULL,
  staging_path TEXT,
  operation TEXT NOT NULL DEFAULT 'write' CHECK (operation IN ('write', 'delete')),
  status TEXT NOT NULL DEFAULT 'pending' CHECK (
    status IN ('pending', 'staged', 'file_written', 'committed', 'rolled_back', 'failed')
  ),
  expected_digest TEXT,
  previous_digest TEXT,
  revision INTEGER,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT '',
  completed_at TEXT
);
";

const WORKFLOW_BOARD_INDEXES_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_workflow_board_cards_project_updated
  ON workflow_board_cards (project_id, updated_at DESC, card_id);

CREATE INDEX IF NOT EXISTS idx_workflow_board_cards_project_stage
  ON workflow_board_cards (project_id, stage, status);

CREATE INDEX IF NOT EXISTS idx_workflow_board_revisions_card_rev
  ON workflow_board_revisions (card_id, revision DESC);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_board_approvals_active_gate
  ON workflow_board_approvals (card_id, gate)
  WHERE status = 'valid';

CREATE INDEX IF NOT EXISTS idx_workflow_board_approvals_card_gate
  ON workflow_board_approvals (card_id, gate, approved_at DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_stage_runs_card_stage
  ON workflow_board_stage_runs (card_id, stage, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_stage_runs_thread
  ON workflow_board_stage_runs (thread_id)
  WHERE thread_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_workflow_board_stage_runs_workflow_run
  ON workflow_board_stage_runs (workflow_run_id)
  WHERE workflow_run_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_workflow_board_write_journal_incomplete
  ON workflow_board_write_journal (card_id, created_at)
  WHERE status NOT IN ('committed', 'rolled_back');
";

/// Creates the 5 Project Workflow Board tables idempotently.
pub(crate) fn create_workflow_board_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(WORKFLOW_BOARD_TABLES_SQL)
}

/// Creates the 9 Project Workflow Board indexes idempotently.
pub(crate) fn create_workflow_board_indexes(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(WORKFLOW_BOARD_INDEXES_SQL)
}
