//! V1/V2 SQLite index suite for projects, threads, events, runs, nodes, and providers (`001..=102`).

use rusqlite::Connection;

const STATIC_INDEXES_SQL: &str = "
DROP INDEX IF EXISTS idx_orch_events_correlation_id;
DROP INDEX IF EXISTS orchestration_v2_projection_context_transfers_source_thread_idx;

CREATE INDEX IF NOT EXISTS idx_projection_projects_updated_at
  ON projection_projects(updated_at);
CREATE INDEX IF NOT EXISTS idx_projection_projects_workspace_root_deleted_at
  ON projection_projects(workspace_root, deleted_at);

CREATE INDEX IF NOT EXISTS idx_projection_threads_project_archived_at
  ON projection_threads(project_id, archived_at);
CREATE INDEX IF NOT EXISTS idx_projection_threads_project_deleted_created
  ON projection_threads(project_id, deleted_at, created_at);
CREATE INDEX IF NOT EXISTS idx_projection_threads_shell_active
  ON projection_threads(deleted_at, archived_at, project_id, created_at, thread_id);
CREATE INDEX IF NOT EXISTS idx_projection_threads_shell_archived
  ON projection_threads(deleted_at, archived_at, project_id, thread_id);

CREATE INDEX IF NOT EXISTS idx_provider_session_runtime_status
  ON provider_session_runtime(status);
CREATE INDEX IF NOT EXISTS idx_provider_session_runtime_provider
  ON provider_session_runtime(provider_name);
CREATE INDEX IF NOT EXISTS idx_provider_session_runtime_instance
  ON provider_session_runtime(provider_instance_id);

CREATE INDEX IF NOT EXISTS idx_projection_thread_messages_thread_created_id
  ON projection_thread_messages(thread_id, created_at, message_id);
CREATE INDEX IF NOT EXISTS idx_projection_thread_sessions_provider_session
  ON projection_thread_sessions(provider_session_id);
CREATE INDEX IF NOT EXISTS idx_projection_thread_sessions_instance
  ON projection_thread_sessions(provider_instance_id);
CREATE INDEX IF NOT EXISTS idx_projection_thread_pull_requests_pr
  ON projection_thread_pull_requests(host, repository, number);

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

CREATE INDEX IF NOT EXISTS idx_orch_command_receipts_aggregate
  ON orchestration_command_receipts(aggregate_kind, aggregate_id);
CREATE INDEX IF NOT EXISTS idx_orch_command_receipts_sequence
  ON orchestration_command_receipts(result_sequence);

CREATE INDEX IF NOT EXISTS orchestration_v2_events_command_idx
  ON orchestration_v2_events(command_id, sequence);
CREATE INDEX IF NOT EXISTS orchestration_v2_events_thread_sequence_idx
  ON orchestration_v2_events(thread_id, sequence);
CREATE INDEX IF NOT EXISTS orchestration_v2_events_thread_type_sequence_idx
  ON orchestration_v2_events(thread_id, event_type, sequence);
CREATE INDEX IF NOT EXISTS orchestration_v2_events_run_sequence_idx
  ON orchestration_v2_events(run_id, sequence);
CREATE INDEX IF NOT EXISTS orchestration_v2_events_node_sequence_idx
  ON orchestration_v2_events(node_id, sequence);
CREATE INDEX IF NOT EXISTS orchestration_v2_events_raw_event_idx
  ON orchestration_v2_events(raw_event_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_events_instance_sequence_idx
  ON orchestration_v2_events(provider_instance_id, sequence);

CREATE INDEX IF NOT EXISTS orchestration_v2_command_receipts_thread_sequence_idx
  ON orchestration_v2_command_receipts(thread_id, result_sequence);

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
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_subagent_result_idx
  ON orchestration_v2_projection_threads(
    thread_id, json_extract(payload_json, '$.lineage.parentThreadId')
  )
  WHERE CASE WHEN json_valid(payload_json) THEN
    json_extract(payload_json, '$.lineage.relationshipToParent') = 'subagent'
    AND json_extract(payload_json, '$.lineage.parentThreadId') IS NOT NULL
    AND json_extract(payload_json, '$.forkedFrom.type') = 'node'
    ELSE 0 END;
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_active_shell_idx
  ON orchestration_v2_projection_threads(
    updated_at, thread_id, project_id,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.lineage.relationshipToParent') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.lineage.parentThreadId') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.lineage.rootThreadId') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.lineage.sideChat') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.settledOverride') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.settledAt') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.pinnedAt') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.pinOrderKey') END,
    CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.snoozedUntil') END,
    CASE WHEN json_valid(payload_json) THEN json_type(payload_json, '$.pullRequests[0]') END
  )
  WHERE deleted_at IS NULL AND archived_at IS NULL;

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
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_last_executed_idx
  ON orchestration_v2_projection_runs(
    thread_id, (completed_at IS NULL) DESC, completed_at DESC, ordinal DESC, run_id DESC
  )
  WHERE status <> 'queued'
    AND NOT (status = 'cancelled' AND json_extract(payload_json, '$.startedAt') IS NULL);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_delegated_delivery_idx
  ON orchestration_v2_projection_runs(thread_id)
  WHERE CASE WHEN json_valid(payload_json)
    THEN json_type(payload_json, '$.delegatedCompletion.delivery') = 'object'
    ELSE 0 END;
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_delegated_task_idx
  ON orchestration_v2_projection_runs(thread_id, json_extract(payload_json, '$.delegatedTaskId'))
  WHERE json_valid(payload_json) AND json_extract(payload_json, '$.delegatedTaskId') IS NOT NULL;
";

const STATIC_INDEXES_PART2_SQL: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS orchestration_v2_projection_run_attempts_run_ordinal_idx
  ON orchestration_v2_projection_run_attempts(run_id, attempt_ordinal);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_run_attempts_thread_idx
  ON orchestration_v2_projection_run_attempts(thread_id, run_id);

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_nodes_thread_run_idx
  ON orchestration_v2_projection_nodes(thread_id, run_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_nodes_parent_idx
  ON orchestration_v2_projection_nodes(parent_node_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_nodes_provider_turn_idx
  ON orchestration_v2_projection_nodes(provider_turn_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_nodes_live_idx
  ON orchestration_v2_projection_nodes(thread_id)
  WHERE status IN ('pending', 'starting', 'running', 'waiting');
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_nodes_settled_leaf_idx
  ON orchestration_v2_projection_nodes(
    thread_id,
    (kind IN ('tool_call', 'assistant_message', 'reasoning')
      AND status IN ('completed', 'interrupted', 'failed', 'cancelled', 'rolled_back')
      AND run_id IS NOT NULL)
  );

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_sessions_thread_idx
  ON orchestration_v2_projection_provider_sessions(thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_sessions_provider_status_idx
  ON orchestration_v2_projection_provider_sessions(provider, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_sessions_instance_status_idx
  ON orchestration_v2_projection_provider_sessions(provider_instance_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_session_bindings_thread_idx
  ON orchestration_v2_projection_provider_session_bindings(thread_id);

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_threads_thread_idx
  ON orchestration_v2_projection_provider_threads(thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_threads_session_idx
  ON orchestration_v2_projection_provider_threads(provider_session_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_threads_owner_idx
  ON orchestration_v2_projection_provider_threads(owner_node_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_threads_instance_status_idx
  ON orchestration_v2_projection_provider_threads(provider_instance_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_threads_pending_tasks_idx
  ON orchestration_v2_projection_provider_threads(thread_id)
  WHERE CASE WHEN json_valid(payload_json)
    THEN json_array_length(payload_json, '$.pendingBackgroundTasks') > 0
    ELSE 0 END;

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_turns_thread_idx
  ON orchestration_v2_projection_provider_turns(thread_id);
CREATE UNIQUE INDEX IF NOT EXISTS orchestration_v2_projection_provider_turns_thread_ordinal_idx
  ON orchestration_v2_projection_provider_turns(provider_thread_id, ordinal);
CREATE INDEX IF NOT EXISTS orchestration_v2_provider_turns_codex_native_id_idx
  ON orchestration_v2_projection_provider_turns(json_extract(payload_json, '$.nativeTurnRef.nativeId'))
  WHERE json_extract(payload_json, '$.nativeTurnRef.driver') = 'codex';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_provider_turns_attempt_idx
  ON orchestration_v2_projection_provider_turns(run_attempt_id);

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runtime_requests_thread_status_idx
  ON orchestration_v2_projection_runtime_requests(thread_id, status);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runtime_requests_provider_turn_idx
  ON orchestration_v2_projection_runtime_requests(provider_turn_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_requests_recovery_idx
  ON orchestration_v2_projection_runtime_requests(thread_id)
  WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_thread_created_idx
  ON orchestration_v2_projection_messages(thread_id, created_at, message_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_run_idx
  ON orchestration_v2_projection_messages(run_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_node_idx
  ON orchestration_v2_projection_messages(node_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_latest_user_idx
  ON orchestration_v2_projection_messages(thread_id, updated_at DESC, message_id DESC)
  WHERE role = 'user';
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_messages_delegated_task_idx
  ON orchestration_v2_projection_messages(thread_id, created_at, message_id)
  WHERE role = 'user' AND json_valid(payload_json) AND (
    json_extract(payload_json, '$.notification.source.kind') = 'monitor'
    OR json_extract(payload_json, '$.delegatedCompletion') IS NOT NULL
  );

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_plans_thread_idx
  ON orchestration_v2_projection_plans(thread_id);
CREATE INDEX IF NOT EXISTS orchestration_v2_projection_plans_run_idx
  ON orchestration_v2_projection_plans(run_id);
";

pub(crate) fn apply_indexes_and_count_triggers(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(STATIC_INDEXES_SQL)?;
    conn.execute_batch(STATIC_INDEXES_PART2_SQL)?;
    super::schema_v2_triggers::apply_extended_indexes_and_triggers(conn)
}
