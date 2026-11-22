//! Orchestration V2, scheduled task, webhook, and issue tracker table DDL.
//!
//! Covers every active table created in Node migrations `055..=102` and Rust `103..=105`.

use rusqlite::Connection;

const ORCHESTRATION_V2_CORE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS orchestration_v2_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  command_id TEXT,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  provider TEXT,
  raw_event_id TEXT,
  event_type TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  driver TEXT,
  provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_command_receipts (
  command_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  command_type TEXT NOT NULL,
  accepted_at TEXT NOT NULL,
  result_sequence INTEGER NOT NULL,
  status TEXT NOT NULL,
  error TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_threads (
  thread_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  default_provider TEXT NOT NULL,
  runtime_mode TEXT NOT NULL,
  interaction_mode TEXT NOT NULL,
  active_provider_thread_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT,
  payload_json TEXT NOT NULL,
  provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_runs (
  run_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  provider TEXT NOT NULL,
  provider_thread_id TEXT,
  status TEXT NOT NULL,
  requested_at TEXT NOT NULL,
  completed_at TEXT,
  payload_json TEXT NOT NULL,
  provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_run_attempts (
  attempt_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  attempt_ordinal INTEGER NOT NULL,
  root_node_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_thread_id TEXT NOT NULL,
  provider_turn_id TEXT,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_nodes (
  node_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  parent_node_id TEXT,
  root_node_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  provider_thread_id TEXT,
  provider_turn_id TEXT,
  runtime_request_id TEXT,
  checkpoint_scope_id TEXT,
  started_at TEXT,
  completed_at TEXT,
  payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_provider_sessions (
  provider_session_id TEXT PRIMARY KEY,
  thread_id TEXT,
  provider TEXT NOT NULL,
  status TEXT NOT NULL,
  model TEXT,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  driver TEXT,
  provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_provider_threads (
  provider_thread_id TEXT PRIMARY KEY,
  thread_id TEXT,
  owner_node_id TEXT,
  provider TEXT NOT NULL,
  provider_session_id TEXT,
  status TEXT NOT NULL,
  first_run_ordinal INTEGER,
  last_run_ordinal INTEGER,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  driver TEXT,
  provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_provider_turns (
  provider_turn_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  provider_thread_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  run_attempt_id TEXT,
  ordinal INTEGER NOT NULL,
  status TEXT NOT NULL,
  started_at TEXT,
  completed_at TEXT,
  payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_runtime_requests (
  runtime_request_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  provider_turn_id TEXT,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  resolved_at TEXT,
  payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_messages (
  message_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  role TEXT NOT NULL,
  streaming INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  search_text TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_plans (
  plan_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
";

const ORCHESTRATION_V2_EXTENDED_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS orchestration_v2_projection_turn_items (
  turn_item_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT,
  provider_thread_id TEXT,
  provider_turn_id TEXT,
  parent_item_id TEXT,
  ordinal INTEGER NOT NULL,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  inline_work_output INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_checkpoint_scopes (
  scope_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT NOT NULL,
  parent_scope_id TEXT,
  provider_thread_id TEXT,
  kind TEXT NOT NULL,
  ordinal_within_parent INTEGER NOT NULL,
  advances_app_run_count INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_checkpoints (
  checkpoint_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  run_id TEXT,
  node_id TEXT NOT NULL,
  parent_checkpoint_id TEXT,
  ordinal_within_scope INTEGER NOT NULL,
  app_run_ordinal INTEGER,
  status TEXT NOT NULL,
  captured_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_context_handoffs (
  context_handoff_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  target_run_id TEXT NOT NULL,
  to_provider_thread_id TEXT NOT NULL,
  strategy TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_context_transfers (
  context_transfer_id TEXT PRIMARY KEY,
  source_thread_id TEXT NOT NULL,
  target_thread_id TEXT NOT NULL,
  target_run_id TEXT,
  type TEXT NOT NULL,
  status TEXT NOT NULL,
  source_provider TEXT,
  target_provider TEXT,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  source_provider_instance_id TEXT,
  target_provider_instance_id TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_subagents (
  subagent_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  run_id TEXT,
  parent_node_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  provider_thread_id TEXT,
  child_thread_id TEXT,
  origin TEXT NOT NULL,
  status TEXT NOT NULL,
  started_at TEXT,
  completed_at TEXT,
  updated_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  driver TEXT,
  provider_instance_id TEXT,
  completion_delivery_state TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_provider_session_bindings (
  provider_session_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  PRIMARY KEY (provider_session_id, thread_id)
);

CREATE TABLE IF NOT EXISTS orchestration_v2_effect_outbox (
  effect_id TEXT PRIMARY KEY,
  command_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  effect_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (
    status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')
  ),
  attempt_count INTEGER NOT NULL DEFAULT 0,
  available_at TEXT NOT NULL,
  lease_owner TEXT,
  lease_expires_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  last_error TEXT
);

CREATE TABLE IF NOT EXISTS orchestration_v2_turn_item_positions (
  thread_id TEXT NOT NULL,
  turn_item_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  PRIMARY KEY (thread_id, turn_item_id),
  UNIQUE (thread_id, ordinal)
);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_turn_item_counts (
  thread_id TEXT NOT NULL,
  run_key TEXT NOT NULL,
  item_count INTEGER NOT NULL,
  PRIMARY KEY (thread_id, run_key)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_thread_item_counts (
  thread_id TEXT PRIMARY KEY,
  item_count INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS orchestration_v2_thread_launch_workflows (
  command_id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  status TEXT NOT NULL,
  title TEXT NOT NULL,
  worktree_path TEXT,
  branch TEXT,
  setup_committed INTEGER NOT NULL DEFAULT 0,
  thread_committed INTEGER NOT NULL DEFAULT 0,
  message_committed INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
";

pub(crate) fn create_v2_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(ORCHESTRATION_V2_CORE_TABLES_SQL)?;
    conn.execute_batch(ORCHESTRATION_V2_EXTENDED_TABLES_SQL)?;
    super::schema_feature_tables::create_feature_tables(conn)
}
