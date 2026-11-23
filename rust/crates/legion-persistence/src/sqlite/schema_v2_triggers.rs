//! Turn-item, checkpoint, context, subagent, feature indexes and item-count triggers (`085`/`102`).

use rusqlite::Connection;

// Matches `apps/server/src/orchestration-v2/deferredCommandMessage.ts` verbatim so SQLite's planner uses the partial index.
const JAVASCRIPT_TRIM_WHITESPACE: &str = " \t\n\r\u{000b}\u{000c}\u{00a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";

const STATIC_INDEXES_PART3_SQL: &str = "
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_thread_ordinal_idx
  ON orchestration_v2_projection_turn_items(thread_id, ordinal, turn_item_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_run_ordinal_idx
  ON orchestration_v2_projection_turn_items(run_id, ordinal);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_node_ordinal_idx
  ON orchestration_v2_projection_turn_items(node_id, ordinal);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_provider_turn_idx
  ON orchestration_v2_projection_turn_items(provider_turn_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_thread_run_idx
  ON orchestration_v2_projection_turn_items(thread_id, run_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_shell_pending_idx
  ON orchestration_v2_projection_turn_items(thread_id, run_id)
  WHERE type IN ('command_execution', 'dynamic_tool', 'subagent')
    AND status NOT IN ('completed', 'interrupted', 'failed', 'cancelled');
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_recovery_idx
  ON orchestration_v2_projection_turn_items(thread_id)
  WHERE type IN ('command_execution', 'dynamic_tool', 'subagent')
    AND status IN ('pending', 'running', 'waiting');
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_user_message_idx
  ON orchestration_v2_projection_turn_items(thread_id, ordinal, turn_item_id)
  WHERE type = 'user_message';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_failed_error_idx
  ON orchestration_v2_projection_turn_items(
    thread_id, run_id, node_id, updated_at DESC, ordinal DESC, turn_item_id DESC
  )
  WHERE type = 'error' AND status = 'failed';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_stop_request_idx
  ON orchestration_v2_projection_turn_items(thread_id, run_id)
  WHERE type = 'run_interrupt_request';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_waiting_link_idx
  ON orchestration_v2_projection_turn_items(thread_id, updated_at DESC, turn_item_id DESC)
  WHERE type = 'link_request' AND status = 'waiting';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_active_ordinal_idx
  ON orchestration_v2_projection_turn_items(thread_id, ordinal, turn_item_id)
  WHERE status IN ('pending', 'running', 'waiting');
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_active_run_idx
  ON orchestration_v2_projection_turn_items(thread_id, run_id)
  WHERE status IN ('pending', 'running', 'waiting');
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_conversation_idx
  ON orchestration_v2_projection_turn_items(thread_id, ordinal, type, run_id)
  WHERE type NOT IN (
    'command_execution', 'reasoning', 'file_change', 'file_search', 'web_search', 'dynamic_tool'
  );
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_queued_input_idx
  ON orchestration_v2_projection_turn_items(run_id)
  WHERE type = 'user_message'
    AND json_valid(payload_json)
    AND json_extract(payload_json, '$.inputIntent') = 'queued_turn';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_thread_type_idx
  ON orchestration_v2_projection_turn_items(thread_id, type, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_message_ordinal_idx
  ON orchestration_v2_projection_turn_items(
    thread_id, json_extract(payload_json, '$.messageId'), ordinal
  )
  WHERE type IN ('user_message', 'assistant_message') AND json_valid(payload_json);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_turn_items_usage_limit_idx
  ON orchestration_v2_projection_turn_items(thread_id)
  WHERE type = 'error' AND status = 'failed'
    AND CASE WHEN json_valid(payload_json) THEN
      json_extract(payload_json, '$.failure.class') = 'usage_limit'
      ELSE 0 END;

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_checkpoint_scopes_thread_idx
  ON orchestration_v2_projection_checkpoint_scopes(thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_checkpoint_scopes_parent_idx
  ON orchestration_v2_projection_checkpoint_scopes(parent_scope_id);

CREATE UNIQUE INDEX IF NOT EXISTS orchestration_v2_projection_checkpoints_scope_ordinal_idx
  ON orchestration_v2_projection_checkpoints(scope_id, ordinal_within_scope);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_checkpoints_thread_idx
  ON orchestration_v2_projection_checkpoints(thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_checkpoints_parent_idx
  ON orchestration_v2_projection_checkpoints(parent_checkpoint_id);

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_context_handoffs_thread_idx
  ON orchestration_v2_projection_context_handoffs(thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_context_handoffs_target_run_idx
  ON orchestration_v2_projection_context_handoffs(target_run_id);

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_context_transfers_target_thread_idx
  ON orchestration_v2_projection_context_transfers(target_thread_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_context_transfers_target_run_idx
  ON orchestration_v2_projection_context_transfers(target_run_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_context_transfers_source_thread_status_idx
  ON orchestration_v2_projection_context_transfers(source_thread_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_context_transfers_fork_idx
  ON orchestration_v2_projection_context_transfers(target_thread_id)
  WHERE type = 'fork';

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_subagents_thread_idx
  ON orchestration_v2_projection_subagents(thread_id, started_at, subagent_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_subagents_parent_node_idx
  ON orchestration_v2_projection_subagents(parent_node_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_subagents_provider_thread_idx
  ON orchestration_v2_projection_subagents(provider_thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_subagents_child_thread_idx
  ON orchestration_v2_projection_subagents(child_thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_subagents_settled_idx
  ON orchestration_v2_projection_subagents(thread_id, status, completion_delivery_state, run_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_subagents_routable_idx
  ON orchestration_v2_projection_subagents(thread_id, subagent_id)
  WHERE origin = 'provider_native' AND status = 'completed';

CREATE INDEX IF NOT EXISTS orchestration_v2_effect_outbox_claim_idx
  ON orchestration_v2_effect_outbox(status, available_at, lease_expires_at, created_at);
CREATE INDEX IF NOT EXISTS orchestration_v2_effect_outbox_command_idx
  ON orchestration_v2_effect_outbox(command_id, effect_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_effect_outbox_thread_status_idx
  ON orchestration_v2_effect_outbox(thread_id, status, effect_type);

CREATE INDEX IF NOT EXISTS orchestration_v2_legacy_imports_pending_transcript_idx
  ON orchestration_v2_legacy_imports(transcript_imported_at, shell_imported_at, thread_id);

CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due
  ON scheduled_tasks(enabled, next_run_at)
  WHERE enabled = 1 AND next_run_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_project
  ON scheduled_tasks(project_id, updated_at);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_webhook_deliveries_task
  ON scheduled_task_webhook_deliveries(task_id, received_at);
CREATE INDEX IF NOT EXISTS idx_scheduled_task_webhook_relay_deliveries_seen
  ON scheduled_task_webhook_relay_deliveries(seen_at);
CREATE INDEX IF NOT EXISTS idx_webhook_replay_expiry
  ON scheduled_task_webhook_replay_keys(expires_at);
CREATE INDEX IF NOT EXISTS idx_webhook_replay_delivery
  ON scheduled_task_webhook_replay_keys(delivery_id);
CREATE INDEX IF NOT EXISTS idx_webhook_pending_due
  ON scheduled_task_webhook_pending(state, due_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_webhook_pending_key
  ON scheduled_task_webhook_pending(task_id, coalesce_key) WHERE state = 'pending';

CREATE INDEX IF NOT EXISTS idx_task_folders_real_root
  ON task_folders(real_root);
";

const ITEM_COUNT_TRIGGERS_SQL: &str = "
CREATE TRIGGER IF NOT EXISTS orchestration_v2_turn_item_counts_insert
AFTER INSERT ON orchestration_v2_projection_turn_items
BEGIN
  INSERT INTO orchestration_v2_projection_turn_item_counts (thread_id, run_key, item_count)
  VALUES (new.thread_id, COALESCE(new.run_id, ''), 1)
  ON CONFLICT (thread_id, run_key) DO UPDATE SET item_count = item_count + 1;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_turn_item_counts_update
AFTER UPDATE OF thread_id, run_id ON orchestration_v2_projection_turn_items
WHEN old.thread_id IS NOT new.thread_id OR old.run_id IS NOT new.run_id
BEGIN
  UPDATE orchestration_v2_projection_turn_item_counts SET item_count = item_count - 1
  WHERE thread_id = old.thread_id AND run_key = COALESCE(old.run_id, '');
  DELETE FROM orchestration_v2_projection_turn_item_counts
  WHERE thread_id = old.thread_id AND run_key = COALESCE(old.run_id, '') AND item_count <= 0;
  INSERT INTO orchestration_v2_projection_turn_item_counts (thread_id, run_key, item_count)
  VALUES (new.thread_id, COALESCE(new.run_id, ''), 1)
  ON CONFLICT (thread_id, run_key) DO UPDATE SET item_count = item_count + 1;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_turn_item_counts_delete
AFTER DELETE ON orchestration_v2_projection_turn_items
BEGIN
  UPDATE orchestration_v2_projection_turn_item_counts SET item_count = item_count - 1
  WHERE thread_id = old.thread_id AND run_key = COALESCE(old.run_id, '');
  DELETE FROM orchestration_v2_projection_turn_item_counts
  WHERE thread_id = old.thread_id AND run_key = COALESCE(old.run_id, '') AND item_count <= 0;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_thread_item_counts_count_insert
AFTER INSERT ON orchestration_v2_projection_turn_item_counts
WHEN new.run_key = ''
  OR EXISTS (SELECT 1 FROM orchestration_v2_projection_runs WHERE run_id = new.run_key)
BEGIN
  INSERT INTO orchestration_v2_projection_thread_item_counts (thread_id, item_count)
  VALUES (new.thread_id, new.item_count)
  ON CONFLICT (thread_id) DO UPDATE SET item_count = item_count + excluded.item_count;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_thread_item_counts_count_update
AFTER UPDATE ON orchestration_v2_projection_turn_item_counts
BEGIN
  UPDATE orchestration_v2_projection_thread_item_counts
  SET item_count = item_count - old.item_count
  WHERE thread_id = old.thread_id
    AND (old.run_key = ''
      OR EXISTS (SELECT 1 FROM orchestration_v2_projection_runs WHERE run_id = old.run_key));
  INSERT INTO orchestration_v2_projection_thread_item_counts (thread_id, item_count)
  SELECT new.thread_id, new.item_count
  WHERE new.run_key = ''
    OR EXISTS (SELECT 1 FROM orchestration_v2_projection_runs WHERE run_id = new.run_key)
  ON CONFLICT (thread_id) DO UPDATE SET item_count = item_count + excluded.item_count;
  DELETE FROM orchestration_v2_projection_thread_item_counts
  WHERE thread_id = old.thread_id AND item_count <= 0;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_thread_item_counts_count_delete
AFTER DELETE ON orchestration_v2_projection_turn_item_counts
WHEN old.run_key = ''
  OR EXISTS (SELECT 1 FROM orchestration_v2_projection_runs WHERE run_id = old.run_key)
BEGIN
  UPDATE orchestration_v2_projection_thread_item_counts
  SET item_count = item_count - old.item_count
  WHERE thread_id = old.thread_id;
  DELETE FROM orchestration_v2_projection_thread_item_counts
  WHERE thread_id = old.thread_id AND item_count <= 0;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_thread_item_counts_run_insert
AFTER INSERT ON orchestration_v2_projection_runs
BEGIN
  INSERT INTO orchestration_v2_projection_thread_item_counts (thread_id, item_count)
  SELECT thread_id, COUNT(*)
  FROM orchestration_v2_projection_turn_items
  WHERE run_id = new.run_id
  GROUP BY thread_id
  ON CONFLICT (thread_id) DO UPDATE SET item_count = item_count + excluded.item_count;
END;

CREATE TRIGGER IF NOT EXISTS orchestration_v2_thread_item_counts_run_delete
AFTER DELETE ON orchestration_v2_projection_runs
BEGIN
  UPDATE orchestration_v2_projection_thread_item_counts
  SET item_count = item_count - (
    SELECT COUNT(*)
    FROM orchestration_v2_projection_turn_items item
    WHERE item.run_id = old.run_id
      AND item.thread_id = orchestration_v2_projection_thread_item_counts.thread_id
  )
  WHERE thread_id IN (
    SELECT thread_id FROM orchestration_v2_projection_turn_items WHERE run_id = old.run_id
  );
  DELETE FROM orchestration_v2_projection_thread_item_counts
  WHERE item_count <= 0 AND thread_id IN (
    SELECT thread_id FROM orchestration_v2_projection_turn_items WHERE run_id = old.run_id
  );
END;
";

pub(crate) fn apply_extended_indexes_and_triggers(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(STATIC_INDEXES_PART3_SQL)?;
    let deferred_sql = format!(
        "CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_deferred_command_idx
         ON orchestration_v2_projection_messages(thread_id, created_at, message_id)
         WHERE role = 'user' AND (
           lower(trim(json_extract(payload_json, '$.text'), '{JAVASCRIPT_TRIM_WHITESPACE}')) = '/compact'
           OR trim(json_extract(payload_json, '$.text'), '{JAVASCRIPT_TRIM_WHITESPACE}') GLOB '/goal*'
         );"
    );
    conn.execute_batch(&deferred_sql)?;
    conn.execute_batch(ITEM_COUNT_TRIGGERS_SQL)
}
