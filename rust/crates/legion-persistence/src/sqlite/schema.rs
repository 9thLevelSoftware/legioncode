//! Core Orchestration V2 DDL tables, indexes, and FTS5 trigram message search definitions.

use rusqlite::Connection;

use super::auth_peer_schema;

const ATTACHMENT_ONLY_BOOTSTRAP_PROMPT: &str = "[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";

// SQLite ltrim needs every character JavaScript's `\s` matches (`messageSearchIndex.ts:7-8`).
const BOOTSTRAP_PREFIX_WHITESPACE: &str = "\t\n\u{000b}\u{000c}\r \u{00a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";

const CORE_V2_TABLES_SQL: &str = "
CREATE TABLE IF NOT EXISTS effect_sql_migrations (
  migration_id integer PRIMARY KEY NOT NULL,
  created_at datetime NOT NULL DEFAULT current_timestamp,
  name VARCHAR(255) NOT NULL
);

CREATE TABLE IF NOT EXISTS projection_projects (
  project_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  workspace_root TEXT NOT NULL,
  scripts_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT,
  default_model_selection_json TEXT,
  default_thread_env_mode TEXT,
  favicon_path TEXT,
  auto_pull INTEGER NOT NULL DEFAULT 0,
  project_icon_json TEXT,
  pinned_at TEXT,
  archived_at TEXT,
  has_custom_title INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_projection_projects_updated_at
  ON projection_projects(updated_at);
CREATE INDEX IF NOT EXISTS idx_projection_projects_workspace_root_deleted_at
  ON projection_projects(workspace_root, deleted_at);

CREATE TABLE IF NOT EXISTS projection_threads (
  thread_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  title TEXT NOT NULL,
  branch TEXT,
  worktree_path TEXT,
  latest_turn_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT,
  runtime_mode TEXT NOT NULL DEFAULT 'full-access',
  interaction_mode TEXT NOT NULL DEFAULT 'default',
  model_selection_json TEXT,
  archived_at TEXT,
  latest_user_message_at TEXT,
  pending_approval_count INTEGER NOT NULL DEFAULT 0,
  pending_user_input_count INTEGER NOT NULL DEFAULT 0,
  has_actionable_proposed_plan INTEGER NOT NULL DEFAULT 0,
  settled_override TEXT,
  settled_at TEXT,
  snoozed_until TEXT,
  snoozed_at TEXT,
  title_regeneration_request_id TEXT,
  title_regeneration_started_at TEXT,
  pinned_at TEXT,
  pin_order_key TEXT,
  linked_pull_request_json TEXT,
  unsettled_at TEXT,
  branch_pull_request_json TEXT,
  active_order_key TEXT,
  title_state_json TEXT,
  auto_settle_disabled_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_projection_threads_project_archived_at
  ON projection_threads(project_id, archived_at);
CREATE INDEX IF NOT EXISTS idx_projection_threads_project_deleted_created
  ON projection_threads(project_id, deleted_at, created_at);
CREATE INDEX IF NOT EXISTS idx_projection_threads_shell_active
  ON projection_threads(deleted_at, archived_at, project_id, created_at, thread_id);
CREATE INDEX IF NOT EXISTS idx_projection_threads_shell_archived
  ON projection_threads(deleted_at, archived_at, project_id, thread_id);

CREATE TABLE IF NOT EXISTS orchestration_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  aggregate_kind TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_version INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  command_id TEXT,
  causation_event_id TEXT,
  correlation_id TEXT,
  actor_kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  application_event_version INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_orch_events_stream_version
  ON orchestration_events(aggregate_kind, stream_id, stream_version);
CREATE INDEX IF NOT EXISTS idx_orch_events_stream_sequence
  ON orchestration_events(aggregate_kind, stream_id, sequence);
CREATE INDEX IF NOT EXISTS idx_orch_events_command_id
  ON orchestration_events(command_id);
CREATE INDEX IF NOT EXISTS idx_orchestration_events_application_sequence
  ON orchestration_events(application_event_version, sequence);
CREATE INDEX IF NOT EXISTS idx_orchestration_events_application_high_water
  ON orchestration_events(sequence)
  WHERE aggregate_kind = 'project'
    OR (application_event_version = 2 AND aggregate_kind = 'thread');
CREATE INDEX IF NOT EXISTS idx_orchestration_events_agent_stream_sequence
  ON orchestration_events(stream_id, sequence)
  WHERE application_event_version = 2 AND aggregate_kind = 'thread';
CREATE INDEX IF NOT EXISTS orchestration_events_v2_created_threads_idx
  ON orchestration_events(stream_id)
  WHERE application_event_version = 2
    AND aggregate_kind = 'thread'
    AND event_type = 'thread.created';

CREATE TABLE IF NOT EXISTS orchestration_command_receipts (
  command_id TEXT PRIMARY KEY,
  aggregate_kind TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  accepted_at TEXT NOT NULL,
  result_sequence INTEGER NOT NULL,
  status TEXT NOT NULL,
  error TEXT,
  command_type TEXT NOT NULL DEFAULT 'legacy'
);
CREATE INDEX IF NOT EXISTS idx_orch_command_receipts_aggregate
  ON orchestration_command_receipts(aggregate_kind, aggregate_id);
CREATE INDEX IF NOT EXISTS idx_orch_command_receipts_sequence
  ON orchestration_command_receipts(result_sequence);

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
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_project_updated_idx
  ON orchestration_v2_projection_threads(project_id, updated_at);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_worktree_path_idx
  ON orchestration_v2_projection_threads(
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.worktreePath') END
  );
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_root_thread_idx
  ON orchestration_v2_projection_threads(
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.lineage.rootThreadId') END
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
CREATE UNIQUE INDEX IF NOT EXISTS orchestration_v2_projection_runs_thread_ordinal_idx
  ON orchestration_v2_projection_runs(thread_id, ordinal);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_provider_thread_idx
  ON orchestration_v2_projection_runs(provider_thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_thread_status_idx
  ON orchestration_v2_projection_runs(thread_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_recovery_idx
  ON orchestration_v2_projection_runs(status, thread_id)
  WHERE status IN ('queued', 'preparing', 'starting', 'running', 'waiting');
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_run_status_idx
  ON orchestration_v2_projection_runs(run_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_hidden_items_idx
  ON orchestration_v2_projection_runs(thread_id, status, run_id)
  WHERE status IN ('rolled_back', 'queued', 'cancelled');

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
CREATE INDEX IF NOT EXISTS orchestration_v2_effect_outbox_claim_idx
  ON orchestration_v2_effect_outbox(status, available_at, lease_expires_at, created_at);
CREATE INDEX IF NOT EXISTS orchestration_v2_effect_outbox_command_idx
  ON orchestration_v2_effect_outbox(command_id, effect_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_effect_outbox_thread_status_idx
  ON orchestration_v2_effect_outbox(thread_id, status, effect_type);

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
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_thread_created_idx
  ON orchestration_v2_projection_messages(thread_id, created_at, message_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_run_idx
  ON orchestration_v2_projection_messages(run_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_node_idx
  ON orchestration_v2_projection_messages(node_id);

CREATE TABLE IF NOT EXISTS orchestration_v2_projection_metadata (
  projection_name TEXT PRIMARY KEY,
  schema_version INTEGER NOT NULL,
  last_sequence INTEGER NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS orchestration_v2_legacy_imports (
  thread_id TEXT PRIMARY KEY,
  source_updated_at TEXT NOT NULL,
  shell_imported_at TEXT NOT NULL,
  transcript_imported_at TEXT,
  imported_message_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
CREATE INDEX IF NOT EXISTS orchestration_v2_legacy_imports_pending_transcript_idx
  ON orchestration_v2_legacy_imports(transcript_imported_at, shell_imported_at, thread_id);
";

const CREATE_FTS_SEARCH_TABLE_SQL: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS orchestration_v2_message_search USING fts5(
  text,
  content = '',
  contentless_delete = 1,
  tokenize = 'trigram remove_diacritics 1'
);
";

pub(crate) fn apply_core_v2_ddl(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(CORE_V2_TABLES_SQL)?;
    auth_peer_schema::apply_auth_and_peer_ddl(conn)?;
    match conn.execute_batch(CREATE_FTS_SEARCH_TABLE_SQL) {
        Ok(()) => {}
        // Match 069_ThreadMessageSearch.ts: ignore only SQLITE_ERROR (errcode === 1) if FTS5 options are unsupported.
        Err(rusqlite::Error::SqliteFailure(ffi_err, _)) if (ffi_err.extended_code & 0xff) == 1 => {
            return Ok(());
        }
        Err(err) => return Err(err),
    }

    let has_search_text = messages_has_search_text_column(conn)?;
    let insert_cond = indexed_message_condition("new");
    let new_text = folded_message_text_expr("new", has_search_text);
    let old_text = folded_message_text_expr("old", has_search_text);
    let triggers_sql = format!(
        "CREATE TRIGGER IF NOT EXISTS orchestration_v2_message_search_insert
         AFTER INSERT ON orchestration_v2_projection_messages
         WHEN {insert_cond}
         BEGIN
           INSERT INTO orchestration_v2_message_search (rowid, text)
           SELECT new.rowid, {new_text}
           WHERE json_extract(new.payload_json, '$.text') IS NOT NULL;
         END;
         CREATE TRIGGER IF NOT EXISTS orchestration_v2_message_search_update
         AFTER UPDATE ON orchestration_v2_projection_messages
         WHEN (old.streaming = 0 OR new.streaming = 0)
           AND (
             old.streaming IS NOT new.streaming
             OR old.role IS NOT new.role
             OR {old_text} IS NOT {new_text}
             OR json_extract(old.payload_json, '$.createdBy')
               IS NOT json_extract(new.payload_json, '$.createdBy')
           )
         BEGIN
           DELETE FROM orchestration_v2_message_search WHERE rowid = old.rowid AND old.streaming = 0;
           INSERT INTO orchestration_v2_message_search (rowid, text)
           SELECT new.rowid, {new_text}
           WHERE {insert_cond}
             AND json_extract(new.payload_json, '$.text') IS NOT NULL;
         END;
         CREATE TRIGGER IF NOT EXISTS orchestration_v2_message_search_delete
         AFTER DELETE ON orchestration_v2_projection_messages
         WHEN old.streaming = 0
         BEGIN
           DELETE FROM orchestration_v2_message_search WHERE rowid = old.rowid;
         END;"
    );
    conn.execute_batch(&triggers_sql)
}

pub(crate) fn messages_has_search_text_column(conn: &Connection) -> rusqlite::Result<bool> {
    match conn.query_row(
        "SELECT 1 FROM pragma_table_info('orchestration_v2_projection_messages') WHERE name = 'search_text'",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(err) => Err(err),
    }
}

pub(crate) fn folded_message_text_expr(row: &str, has_search_text: bool) -> String {
    let base = if has_search_text {
        format!("COALESCE({row}.search_text, json_extract({row}.payload_json, '$.text'))")
    } else {
        format!("json_extract({row}.payload_json, '$.text')")
    };
    format!("replace(replace({base}, '’', ''''), '‘', '''')")
}

pub(crate) fn indexed_message_condition(row: &str) -> String {
    let escaped_prompt = ATTACHMENT_ONLY_BOOTSTRAP_PROMPT.replace('\'', "''");
    format!(
        "{row}.streaming = 0 \
         AND ({row}.role = 'assistant' OR ({row}.role = 'user' AND COALESCE(json_extract({row}.payload_json, '$.createdBy'), 'user') = 'user')) \
         AND NOT EXISTS ( \
           SELECT 1 FROM orchestration_v2_projection_threads AS subagent_threads \
           WHERE subagent_threads.thread_id = {row}.thread_id \
             AND json_extract(subagent_threads.payload_json, '$.lineage.relationshipToParent') = 'subagent' \
         ) \
         AND CASE \
           WHEN substr(json_extract({row}.payload_json, '$.text'), 1, 11) = 'Ultrathink:' COLLATE NOCASE \
             THEN ltrim(substr(json_extract({row}.payload_json, '$.text'), 12), '{BOOTSTRAP_PREFIX_WHITESPACE}') \
           ELSE json_extract({row}.payload_json, '$.text') \
         END COLLATE BINARY <> '{escaped_prompt}'"
    )
}
