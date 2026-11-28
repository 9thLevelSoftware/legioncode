//! SQLite DDL for Milestone 2 Workflow Board planning state (`BUILD_MIGRATION_HEAD = 112`).

use std::collections::HashSet;

use rusqlite::Connection;

const CARD_M2_COLUMNS: &[(&str, &str)] = &[
    ("intake_goal", "TEXT"),
    ("brief_path", "TEXT"),
    ("brief_digest", "TEXT"),
    ("input_paths_json", "TEXT NOT NULL DEFAULT '[]'"),
    ("wireframes_digest", "TEXT"),
    ("design_digest", "TEXT"),
    ("tasks_digest", "TEXT"),
];

const WORKFLOW_BOARD_M2_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS workflow_board_exploration_rounds (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  round_index INTEGER NOT NULL CHECK (round_index >= 1),
  status TEXT NOT NULL CHECK (status IN ('in_progress', 'awaiting_continue', 'paused', 'completed')),
  decision_ids_json TEXT NOT NULL DEFAULT '[]',
  pending_continue_prompt INTEGER NOT NULL DEFAULT 0 CHECK (pending_continue_prompt IN (0, 1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (card_id, round_index)
);

CREATE TABLE IF NOT EXISTS workflow_board_decisions (
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  decision_id TEXT NOT NULL,
  round_index INTEGER,
  prompt TEXT NOT NULL DEFAULT '',
  recommendation TEXT,
  depends_on_json TEXT NOT NULL DEFAULT '[]',
  last_action TEXT CHECK (last_action IS NULL OR last_action IN ('explain', 'recommend', 'edit', 'unsure', 'pause', 'resume')),
  status TEXT NOT NULL DEFAULT 'unresolved' CHECK (status IN ('unresolved', 'resolved', 'accepted', 'rejected', 'superseded')),
  answer TEXT,
  rationale TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (card_id, decision_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS workflow_board_challenges (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  round INTEGER NOT NULL DEFAULT 1 CHECK (round >= 1),
  status TEXT NOT NULL CHECK (status IN ('pending', 'in_progress', 'resolved', 'synthesized', 'manual_fallback', 'complete', 'stale')),
  spec_digest TEXT NOT NULL,
  interview_digest TEXT NOT NULL DEFAULT '',
  source_refs_digest TEXT NOT NULL DEFAULT '',
  final_spec_digest TEXT,
  concerns_json TEXT NOT NULL DEFAULT '[]',
  synthesis_count INTEGER NOT NULL DEFAULT 0 CHECK (synthesis_count BETWEEN 0 AND 1),
  synthesis_diff_json TEXT,
  manual_fallback_json TEXT,
  generation_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE (card_id, round)
);

CREATE TABLE IF NOT EXISTS workflow_board_plan_designs (
  card_id TEXT PRIMARY KEY REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  lenses_json TEXT NOT NULL DEFAULT '[]',
  alternatives_json TEXT NOT NULL DEFAULT '[]',
  selected_alternative_id TEXT,
  selection_rationale TEXT,
  comparison_revision_count INTEGER NOT NULL DEFAULT 0 CHECK (comparison_revision_count BETWEEN 0 AND 1),
  strategy TEXT NOT NULL DEFAULT 'outcomes' CHECK (strategy IN ('outcomes', 'risk-first', 'expand-contract', 'custom')),
  strategy_rationale TEXT,
  strategy_metadata_json TEXT NOT NULL DEFAULT '{}',
  granularity TEXT NOT NULL DEFAULT 'balanced' CHECK (granularity IN ('coarse', 'balanced', 'fine')),
  design_digest TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_board_tasks (
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  task_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'todo' CHECK (status IN ('pending', 'todo', 'in_progress', 'blocked', 'completed', 'failed', 'skipped')),
  priority TEXT NOT NULL DEFAULT 'P1',
  parent_id TEXT,
  blocked_by_json TEXT NOT NULL DEFAULT '[]',
  blocks_json TEXT NOT NULL DEFAULT '[]',
  files_allowed_json TEXT NOT NULL DEFAULT '[]',
  files_forbidden_json TEXT NOT NULL DEFAULT '[]',
  expected_artifacts_json TEXT NOT NULL DEFAULT '[]',
  verification_commands_json TEXT NOT NULL DEFAULT '[]',
  testing_method TEXT NOT NULL DEFAULT 'test-first' CHECK (testing_method IN ('test-first', 'regression-first', 'existing-checks')),
  testing_boundary TEXT,
  acceptance_ids_json TEXT NOT NULL DEFAULT '[]',
  blocker_reason TEXT,
  notes TEXT,
  max_files_touched INTEGER,
  contract_digest TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (card_id, task_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS workflow_board_planned_checks (
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  check_id TEXT NOT NULL,
  owner_task_id TEXT,
  argv_json TEXT NOT NULL,
  label TEXT,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'passed', 'failed', 'stale', 'historical')),
  last_exit_code INTEGER,
  last_log_ref TEXT,
  product_snapshot_digest TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (card_id, check_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS workflow_board_ac_evidence (
  card_id TEXT NOT NULL REFERENCES workflow_board_cards(card_id) ON DELETE CASCADE,
  criterion_id TEXT NOT NULL,
  method TEXT NOT NULL CHECK (method IN ('automated', 'manual', 'external')),
  check_ids_json TEXT NOT NULL DEFAULT '[]',
  command_argv_json TEXT,
  procedure TEXT,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'pass', 'fail', 'not_applicable', 'stale', 'historical')),
  observation_note TEXT,
  evidence_ref TEXT,
  evidence_digest TEXT,
  product_snapshot_digest TEXT,
  observed_by TEXT,
  observed_at TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (card_id, criterion_id)
) WITHOUT ROWID;
";

const WORKFLOW_BOARD_M2_INDEXES_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_workflow_board_exploration_rounds_card
  ON workflow_board_exploration_rounds (card_id, round_index DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_decisions_card_status
  ON workflow_board_decisions (card_id, status, decision_id);

CREATE INDEX IF NOT EXISTS idx_workflow_board_challenges_card_round
  ON workflow_board_challenges (card_id, round DESC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_tasks_card_ordinal
  ON workflow_board_tasks (card_id, ordinal ASC, task_id ASC);

CREATE INDEX IF NOT EXISTS idx_workflow_board_tasks_card_status
  ON workflow_board_tasks (card_id, status);

CREATE INDEX IF NOT EXISTS idx_workflow_board_planned_checks_card_status
  ON workflow_board_planned_checks (card_id, status);

CREATE INDEX IF NOT EXISTS idx_workflow_board_ac_evidence_card_status
  ON workflow_board_ac_evidence (card_id, status);
";

fn ensure_card_m2_columns(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info('workflow_board_cards')")?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    if existing.is_empty() {
        return Ok(());
    }
    for (col_name, col_def) in CARD_M2_COLUMNS {
        if !existing.contains(*col_name) {
            conn.execute_batch(&format!(
                "ALTER TABLE workflow_board_cards ADD COLUMN {col_name} {col_def};"
            ))?;
        }
    }
    Ok(())
}

/// Applies M2 workflow board columns, 7 child planning tables, and 7 indexes idempotently.
pub(crate) fn apply_m2_schema(conn: &Connection) -> rusqlite::Result<()> {
    ensure_card_m2_columns(conn)?;
    conn.execute_batch(WORKFLOW_BOARD_M2_TABLES_SQL)?;
    conn.execute_batch(WORKFLOW_BOARD_M2_INDEXES_SQL)?;
    Ok(())
}
