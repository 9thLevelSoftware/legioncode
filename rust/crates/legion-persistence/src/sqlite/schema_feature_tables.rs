//! Feature tables: scheduled tasks, webhooks, issue tracker bindings/accounts, feedback, task folders, and workflows.

use rusqlite::Connection;

const FEATURE_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS scheduled_tasks (
  task_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  prompt TEXT NOT NULL,
  enabled INTEGER NOT NULL,
  schedule_json TEXT NOT NULL,
  project_id TEXT NOT NULL,
  thread_id TEXT,
  workspace_strategy_json TEXT NOT NULL,
  model_selection_json TEXT NOT NULL,
  runtime_mode TEXT NOT NULL,
  interaction_mode TEXT NOT NULL,
  created_by TEXT NOT NULL,
  creation_source TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  next_run_at TEXT,
  last_run_at TEXT,
  last_run_status TEXT NOT NULL,
  last_run_error TEXT,
  run_count INTEGER NOT NULL,
  webhook_token TEXT,
  webhook_secret TEXT,
  last_run_webhook_delivery_id TEXT,
  thread_mode TEXT NOT NULL DEFAULT 'new_thread'
);

CREATE TABLE IF NOT EXISTS scheduled_task_webhook_deliveries (
  delivery_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  received_at TEXT NOT NULL,
  method TEXT NOT NULL,
  query TEXT NOT NULL,
  headers_json TEXT NOT NULL,
  body TEXT NOT NULL,
  body_bytes INTEGER NOT NULL,
  body_truncated INTEGER NOT NULL,
  outcome TEXT NOT NULL,
  signature_verified INTEGER NOT NULL,
  missing_fields_json TEXT NOT NULL,
  rendered_prompt TEXT,
  error TEXT
);

CREATE TABLE IF NOT EXISTS scheduled_task_webhook_relay_deliveries (
  relay_delivery_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  seen_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS scheduled_task_webhook_replay_keys (
  task_id TEXT NOT NULL REFERENCES scheduled_tasks(task_id) ON DELETE CASCADE,
  replay_key TEXT NOT NULL,
  delivery_id TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (task_id, replay_key)
);

CREATE TABLE IF NOT EXISTS scheduled_task_webhook_pending (
  delivery_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES scheduled_tasks(task_id) ON DELETE CASCADE,
  coalesce_key TEXT NOT NULL,
  due_at INTEGER NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  task_json TEXT NOT NULL,
  request_json TEXT NOT NULL,
  delivery_ids_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS issue_tracker_bindings (
  project_id TEXT PRIMARY KEY,
  tracker TEXT NOT NULL,
  scope TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  additional_scopes TEXT NOT NULL DEFAULT '[]',
  account_id TEXT
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS issue_tracker_accounts (
  tracker TEXT NOT NULL,
  account_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (tracker, account_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS agent_feedback (
  id TEXT PRIMARY KEY,
  thread_id TEXT,
  run_id TEXT,
  created_at TEXT NOT NULL,
  kind TEXT NOT NULL,
  subject TEXT NOT NULL,
  expected TEXT NOT NULL,
  actual TEXT NOT NULL,
  workaround TEXT NOT NULL,
  user_requested INTEGER NOT NULL DEFAULT 0,
  trace TEXT,
  source TEXT NOT NULL,
  sent INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS task_folders (
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
";

pub(crate) fn create_feature_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(FEATURE_TABLES_SQL)?;
    super::schema_workflow_tables::create_workflow_tables(conn)
}
