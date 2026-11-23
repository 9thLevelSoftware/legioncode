//! Schema parity integration tests verifying Migration-102/105 Node & Rust DB compatibility.

use std::collections::HashSet;

use legion_persistence::sqlite::SqliteStore;
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type ExpectedTableSpec = (&'static str, &'static [&'static str]);

const EXPECTED_TABLES: &[ExpectedTableSpec] = &[
    (
        "effect_sql_migrations",
        &["migration_id", "created_at", "name"],
    ),
    (
        "projection_projects",
        &[
            "project_id",
            "title",
            "workspace_root",
            "scripts_json",
            "default_model_selection_json",
            "default_thread_env_mode",
            "favicon_path",
            "auto_pull",
            "project_icon_json",
            "pinned_at",
            "archived_at",
            "has_custom_title",
            "tracked_folders_json",
        ],
    ),
    (
        "projection_threads",
        &[
            "thread_id",
            "project_id",
            "title",
            "model_selection_json",
            "settled_override",
            "settled_at",
            "snoozed_until",
            "pinned_at",
            "pin_order_key",
            "linked_pull_request_json",
            "branch_pull_request_json",
            "active_order_key",
            "title_state_json",
            "auto_settle_disabled_at",
        ],
    ),
    (
        "orchestration_events",
        &[
            "sequence",
            "event_id",
            "aggregate_kind",
            "application_event_version",
        ],
    ),
    (
        "orchestration_command_receipts",
        &["command_id", "result_sequence", "status", "command_type"],
    ),
    (
        "provider_session_runtime",
        &["thread_id", "provider_instance_id"],
    ),
    (
        "projection_thread_messages",
        &["message_id", "attachments_json", "context_json"],
    ),
    (
        "projection_thread_sessions",
        &["thread_id", "runtime_mode", "provider_instance_id"],
    ),
    (
        "projection_thread_pull_requests",
        &[
            "thread_id",
            "host",
            "repository",
            "number",
            "snapshot_json",
            "stack_json",
        ],
    ),
    (
        "pull_request_files_viewed",
        &[
            "provider",
            "host",
            "repository",
            "number",
            "viewer",
            "path",
            "viewed_at",
        ],
    ),
    (
        "pull_request_review_guides",
        &["host", "repository", "number", "guide_json"],
    ),
    (
        "mcp_app_model_context",
        &["thread_id", "item_id", "server", "tool", "text"],
    ),
    (
        "auth_pairing_links",
        &[
            "id",
            "scopes",
            "proof_key_thumbprint",
            "requires_connect_approval",
            "requested_client_json",
            "connect_denied",
        ],
    ),
    (
        "auth_sessions",
        &[
            "session_id",
            "scopes",
            "client_surface",
            "client_app_version",
            "proof_key_thumbprint",
            "bearer_token_hash",
        ],
    ),
    (
        "auth_connect_devices",
        &["proof_key_thumbprint", "client_json", "approved_at"],
    ),
    ("peer_environment_links", &["environment_id", "fenced"]),
    (
        "orchestration_v2_events",
        &["sequence", "event_id", "driver", "provider_instance_id"],
    ),
    (
        "orchestration_v2_command_receipts",
        &["command_id", "thread_id", "command_type"],
    ),
    (
        "orchestration_v2_projection_threads",
        &["thread_id", "provider_instance_id"],
    ),
    (
        "orchestration_v2_projection_runs",
        &["run_id", "provider_instance_id"],
    ),
    (
        "orchestration_v2_projection_run_attempts",
        &["attempt_id", "provider_instance_id"],
    ),
    (
        "orchestration_v2_projection_nodes",
        &["node_id", "root_node_id", "kind"],
    ),
    (
        "orchestration_v2_projection_provider_sessions",
        &["provider_session_id", "driver", "provider_instance_id"],
    ),
    (
        "orchestration_v2_projection_provider_threads",
        &["provider_thread_id", "driver", "provider_instance_id"],
    ),
    (
        "orchestration_v2_projection_provider_turns",
        &["provider_turn_id", "run_attempt_id"],
    ),
    (
        "orchestration_v2_projection_runtime_requests",
        &["runtime_request_id", "kind"],
    ),
    (
        "orchestration_v2_projection_messages",
        &["message_id", "search_text"],
    ),
    (
        "orchestration_v2_projection_plans",
        &["plan_id", "kind", "status"],
    ),
    (
        "orchestration_v2_projection_turn_items",
        &["turn_item_id", "inline_work_output"],
    ),
    (
        "orchestration_v2_projection_checkpoint_scopes",
        &["scope_id", "ordinal_within_parent"],
    ),
    (
        "orchestration_v2_projection_checkpoints",
        &["checkpoint_id", "app_run_ordinal"],
    ),
    (
        "orchestration_v2_projection_context_handoffs",
        &["context_handoff_id", "strategy"],
    ),
    (
        "orchestration_v2_projection_context_transfers",
        &[
            "context_transfer_id",
            "source_provider_instance_id",
            "target_provider_instance_id",
        ],
    ),
    (
        "orchestration_v2_projection_subagents",
        &[
            "subagent_id",
            "driver",
            "provider_instance_id",
            "completion_delivery_state",
        ],
    ),
    (
        "orchestration_v2_projection_provider_session_bindings",
        &["provider_session_id", "thread_id"],
    ),
    (
        "orchestration_v2_effect_outbox",
        &["effect_id", "status", "lease_owner"],
    ),
    (
        "orchestration_v2_turn_item_positions",
        &["thread_id", "turn_item_id", "ordinal"],
    ),
    (
        "orchestration_v2_projection_turn_item_counts",
        &["thread_id", "run_key", "item_count"],
    ),
    (
        "orchestration_v2_projection_thread_item_counts",
        &["thread_id", "item_count"],
    ),
    (
        "orchestration_v2_thread_launch_workflows",
        &["command_id", "thread_id", "status"],
    ),
    (
        "orchestration_v2_projection_metadata",
        &["projection_name", "last_sequence"],
    ),
    (
        "orchestration_v2_legacy_imports",
        &["thread_id", "transcript_imported_at"],
    ),
    (
        "scheduled_tasks",
        &[
            "task_id",
            "webhook_token",
            "webhook_secret",
            "last_run_webhook_delivery_id",
            "thread_mode",
        ],
    ),
    (
        "scheduled_task_webhook_deliveries",
        &["delivery_id", "task_id"],
    ),
    (
        "scheduled_task_webhook_relay_deliveries",
        &["relay_delivery_id", "task_id"],
    ),
    (
        "scheduled_task_webhook_replay_keys",
        &["task_id", "replay_key"],
    ),
    (
        "scheduled_task_webhook_pending",
        &["delivery_id", "coalesce_key"],
    ),
    (
        "issue_tracker_bindings",
        &["project_id", "tracker", "scope", "additional_scopes"],
    ),
    (
        "task_folders",
        &["root", "real_root", "project_id", "repositories_json"],
    ),
];

fn columns_of(conn: &Connection, table: &str) -> Result<HashSet<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut cols = HashSet::new();
    for col in rows {
        cols.insert(col?);
    }
    Ok(cols)
}

#[test]
fn fresh_store_creates_all_node_and_rust_v105_tables_columns_and_triggers() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    for &(table, req_cols) in EXPECTED_TABLES {
        let cols = columns_of(conn, table)?;
        assert!(!cols.is_empty(), "missing table `{table}`");
        for col in req_cols {
            assert!(
                cols.contains(*col),
                "table `{table}` missing column `{col}`"
            );
        }
    }

    // Verify 085 + 102 item-count triggers keep per-run and per-thread counts accurate.
    conn.execute(
        "INSERT INTO orchestration_v2_projection_runs (
           run_id, thread_id, ordinal, provider, provider_thread_id, status, requested_at, completed_at, payload_json, provider_instance_id
         ) VALUES ('r1', 't1', 1, 'codex', NULL, 'completed', '2026-10-01T00:00:00Z', '2026-10-01T00:01:00Z', '{}', 'codex')",
        [],
    )?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_turn_items (
           turn_item_id, thread_id, run_id, node_id, provider_thread_id, provider_turn_id,
           parent_item_id, ordinal, type, status, updated_at, payload_json
         ) VALUES ('i1', 't1', 'r1', NULL, NULL, NULL, NULL, 1, 'user_message', 'completed', '2026-10-01T00:00:00Z', '{}')",
        [],
    )?;
    let run_count: i64 = conn.query_row(
        "SELECT item_count FROM orchestration_v2_projection_turn_item_counts WHERE thread_id = 't1' AND run_key = 'r1'",
        [],
        |r| r.get(0),
    )?;
    let thread_count: i64 = conn.query_row(
        "SELECT item_count FROM orchestration_v2_projection_thread_item_counts WHERE thread_id = 't1'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(run_count, 1);
    assert_eq!(thread_count, 1);
    Ok(())
}
