//! Durable workflows tables (`110_Workflows`) and Rust replay journal (`workflow_engine_journal`).

use rusqlite::Connection;

pub(crate) const WORKFLOW_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS workflow_runs (
  run_id TEXT PRIMARY KEY,
  workflow TEXT NOT NULL,
  version INTEGER NOT NULL,
  engine_tag TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL,
  detail TEXT,
  result_json TEXT,
  pauses INTEGER NOT NULL DEFAULT 0,
  started_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  ended_at TEXT,
  input_json TEXT,
  parent_run_id TEXT,
  parent_wait TEXT,
  held INTEGER NOT NULL DEFAULT 0,
  engine_pruned_at TEXT
);

CREATE TABLE IF NOT EXISTS workflow_steps (
  run_id TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  detail TEXT,
  thread_id TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  environment_id TEXT,
  result_json TEXT,
  said TEXT,
  turn_id TEXT,
  PRIMARY KEY (run_id, name)
);

CREATE TABLE IF NOT EXISTS workflow_waits (
  run_id TEXT NOT NULL,
  name TEXT NOT NULL,
  workflow TEXT NOT NULL,
  token TEXT NOT NULL,
  key TEXT,
  schema_json TEXT NOT NULL,
  request_json TEXT,
  deadline TEXT,
  opened_at TEXT NOT NULL,
  signalled_at TEXT,
  watch_json TEXT,
  woke_json TEXT,
  PRIMARY KEY (run_id, name)
);

CREATE TABLE IF NOT EXISTS workflow_mail (
  signal_id TEXT PRIMARY KEY,
  address TEXT NOT NULL,
  value_json TEXT NOT NULL,
  expires_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_marks (
  run_id TEXT NOT NULL,
  name TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (run_id, name)
);

CREATE TABLE IF NOT EXISTS workflow_signals (
  signal_id TEXT PRIMARY KEY,
  kept_until TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_triggers (
  trigger_id TEXT PRIMARY KEY,
  next_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_hooks (
  hook_id TEXT PRIMARY KEY,
  token TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_store (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workflow_run_references (
  run_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  key TEXT NOT NULL,
  address_json TEXT NOT NULL,
  title TEXT NOT NULL,
  url TEXT,
  state TEXT,
  steps_json TEXT NOT NULL,
  origin TEXT NOT NULL,
  noted_at TEXT NOT NULL,
  PRIMARY KEY (run_id, kind, key)
);

CREATE TABLE IF NOT EXISTS workflow_engine_journal (
  run_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  entry_key TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  payload_json TEXT,
  wake_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, entry_key)
);
";

pub(crate) const WORKFLOW_INDEXES_SQL: &str = "
CREATE INDEX IF NOT EXISTS workflow_runs_status ON workflow_runs (status, updated_at);
CREATE INDEX IF NOT EXISTS workflow_runs_parent ON workflow_runs (parent_run_id);
CREATE UNIQUE INDEX IF NOT EXISTS workflow_runs_key ON workflow_runs (workflow, idempotency_key);
CREATE INDEX IF NOT EXISTS workflow_runs_unpruned ON workflow_runs (ended_at)
  WHERE ended_at IS NOT NULL AND engine_pruned_at IS NULL;
CREATE INDEX IF NOT EXISTS workflow_waits_key ON workflow_waits (workflow, key);
CREATE INDEX IF NOT EXISTS workflow_mail_address ON workflow_mail (address);
CREATE INDEX IF NOT EXISTS workflow_engine_journal_seq ON workflow_engine_journal (run_id, seq);
";

pub(crate) fn create_workflow_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(WORKFLOW_TABLES_SQL)
}

pub(crate) fn create_workflow_indexes(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(WORKFLOW_INDEXES_SQL)
}
