//! Idempotent `PRAGMA table_info` column migrations across all evolving Node & Rust tables.

use std::collections::HashSet;

use rusqlite::Connection;

pub(crate) struct ColumnMigrationReport {
    pub(crate) added_message_search_text: bool,
}

fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let query = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut cols = HashSet::new();
    for row in rows {
        cols.insert(row?);
    }
    Ok(cols)
}

fn add_missing_columns(
    conn: &Connection,
    table: &str,
    columns: &[(&'static str, &'static str)],
) -> rusqlite::Result<Vec<&'static str>> {
    let existing = table_columns(conn, table)?;
    let mut added = Vec::new();
    for &(name, definition) in columns {
        if !existing.contains(name) {
            let alter = format!("ALTER TABLE {table} ADD COLUMN {name} {definition}");
            conn.execute_batch(&alter)?;
            added.push(name);
        }
    }
    Ok(added)
}

pub(crate) fn apply_column_migrations(
    conn: &Connection,
) -> rusqlite::Result<ColumnMigrationReport> {
    add_missing_columns(
        conn,
        "orchestration_events",
        &[("application_event_version", "INTEGER NOT NULL DEFAULT 1")],
    )?;
    add_missing_columns(
        conn,
        "orchestration_command_receipts",
        &[("command_type", "TEXT NOT NULL DEFAULT 'legacy'")],
    )?;
    add_missing_columns(
        conn,
        "provider_session_runtime",
        &[
            ("runtime_mode", "TEXT NOT NULL DEFAULT 'full-access'"),
            ("provider_instance_id", "TEXT"),
        ],
    )?;
    add_missing_columns(
        conn,
        "projection_thread_messages",
        &[("attachments_json", "TEXT"), ("context_json", "TEXT")],
    )?;
    add_missing_columns(
        conn,
        "projection_thread_sessions",
        &[
            ("runtime_mode", "TEXT NOT NULL DEFAULT 'full-access'"),
            ("provider_instance_id", "TEXT"),
        ],
    )?;
    add_missing_columns(
        conn,
        "projection_projects",
        &[
            ("default_model_selection_json", "TEXT"),
            ("default_thread_env_mode", "TEXT"),
            ("favicon_path", "TEXT"),
            ("auto_pull", "INTEGER NOT NULL DEFAULT 0"),
            ("project_icon_json", "TEXT"),
            ("pinned_at", "TEXT"),
            ("archived_at", "TEXT"),
            ("has_custom_title", "INTEGER NOT NULL DEFAULT 0"),
            ("tracked_folders_json", "TEXT NOT NULL DEFAULT '[]'"),
        ],
    )?;
    add_missing_columns(
        conn,
        "projection_threads",
        &[
            ("model_selection_json", "TEXT"),
            ("runtime_mode", "TEXT NOT NULL DEFAULT 'full-access'"),
            ("interaction_mode", "TEXT NOT NULL DEFAULT 'default'"),
            ("archived_at", "TEXT"),
            ("latest_user_message_at", "TEXT"),
            ("pending_approval_count", "INTEGER NOT NULL DEFAULT 0"),
            ("pending_user_input_count", "INTEGER NOT NULL DEFAULT 0"),
            ("has_actionable_proposed_plan", "INTEGER NOT NULL DEFAULT 0"),
            ("settled_override", "TEXT"),
            ("settled_at", "TEXT"),
            ("snoozed_until", "TEXT"),
            ("snoozed_at", "TEXT"),
            ("title_regeneration_request_id", "TEXT"),
            ("title_regeneration_started_at", "TEXT"),
            ("pinned_at", "TEXT"),
            ("pin_order_key", "TEXT"),
            ("linked_pull_request_json", "TEXT"),
            ("unsettled_at", "TEXT"),
            ("branch_pull_request_json", "TEXT"),
            ("active_order_key", "TEXT"),
            ("title_state_json", "TEXT"),
            ("auto_settle_disabled_at", "TEXT"),
        ],
    )?;

    apply_v2_column_migrations(conn)
}

type ColumnSpec = (&'static str, &'static str);

const V2_INSTANCE_BACKFILLS: &[(&str, &[ColumnSpec], &str)] = &[
    (
        "orchestration_v2_events",
        &[("driver", "TEXT"), ("provider_instance_id", "TEXT")],
        "UPDATE orchestration_v2_events SET
           driver = COALESCE(driver, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.driver') END, provider),
           provider_instance_id = COALESCE(provider_instance_id, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, provider);",
    ),
    (
        "orchestration_v2_projection_threads",
        &[("provider_instance_id", "TEXT")],
        "UPDATE orchestration_v2_projection_threads SET provider_instance_id = COALESCE(CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, default_provider);",
    ),
    (
        "orchestration_v2_projection_runs",
        &[("provider_instance_id", "TEXT")],
        "UPDATE orchestration_v2_projection_runs SET provider_instance_id = COALESCE(CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, provider);",
    ),
    (
        "orchestration_v2_projection_run_attempts",
        &[("provider_instance_id", "TEXT")],
        "UPDATE orchestration_v2_projection_run_attempts SET provider_instance_id = COALESCE(CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, provider);",
    ),
    (
        "orchestration_v2_projection_provider_sessions",
        &[("driver", "TEXT"), ("provider_instance_id", "TEXT")],
        "UPDATE orchestration_v2_projection_provider_sessions SET
           driver = COALESCE(driver, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.driver') END, provider),
           provider_instance_id = COALESCE(provider_instance_id, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, provider);",
    ),
    (
        "orchestration_v2_projection_provider_threads",
        &[("driver", "TEXT"), ("provider_instance_id", "TEXT")],
        "UPDATE orchestration_v2_projection_provider_threads SET
           driver = COALESCE(driver, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.driver') END, provider),
           provider_instance_id = COALESCE(provider_instance_id, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, provider);",
    ),
    (
        "orchestration_v2_projection_subagents",
        &[
            ("driver", "TEXT"),
            ("provider_instance_id", "TEXT"),
            ("completion_delivery_state", "TEXT"),
        ],
        "UPDATE orchestration_v2_projection_subagents SET
           driver = COALESCE(driver, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.driver') END),
           provider_instance_id = COALESCE(provider_instance_id, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.providerInstanceId') END, provider),
           completion_delivery_state = COALESCE(
             completion_delivery_state,
             CASE WHEN json_valid(payload_json) AND json_type(payload_json, '$.completionDelivery') IS NOT NULL
               THEN json_extract(payload_json, '$.completionDelivery.state')
             END
           );",
    ),
    (
        "orchestration_v2_projection_context_transfers",
        &[
            ("source_provider_instance_id", "TEXT"),
            ("target_provider_instance_id", "TEXT"),
        ],
        "UPDATE orchestration_v2_projection_context_transfers SET
           source_provider_instance_id = COALESCE(source_provider_instance_id, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.sourceProviderInstanceId') END, source_provider),
           target_provider_instance_id = COALESCE(target_provider_instance_id, CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.targetProviderInstanceId') END, target_provider);",
    ),
];

fn apply_v2_column_migrations(conn: &Connection) -> rusqlite::Result<ColumnMigrationReport> {
    for &(table, columns, backfill_sql) in V2_INSTANCE_BACKFILLS {
        if !add_missing_columns(conn, table, columns)?.is_empty() {
            conn.execute_batch(backfill_sql)?;
        }
    }

    let msg_added = add_missing_columns(
        conn,
        "orchestration_v2_projection_messages",
        &[("search_text", "TEXT")],
    )?;
    add_missing_columns(
        conn,
        "orchestration_v2_projection_turn_items",
        &[("inline_work_output", "INTEGER NOT NULL DEFAULT 0")],
    )?;
    add_missing_columns(
        conn,
        "scheduled_tasks",
        &[
            ("webhook_token", "TEXT"),
            ("webhook_secret", "TEXT"),
            ("last_run_webhook_delivery_id", "TEXT"),
            ("thread_mode", "TEXT NOT NULL DEFAULT 'new_thread'"),
        ],
    )?;
    add_missing_columns(
        conn,
        "issue_tracker_bindings",
        &[
            ("additional_scopes", "TEXT NOT NULL DEFAULT '[]'"),
            ("account_id", "TEXT"),
        ],
    )?;

    backfill_item_counts_if_empty(conn)?;

    Ok(ColumnMigrationReport {
        added_message_search_text: msg_added.contains(&"search_text"),
    })
}

fn backfill_item_counts_if_empty(conn: &Connection) -> rusqlite::Result<()> {
    let has_items: bool = conn
        .query_row(
            "SELECT 1 FROM orchestration_v2_projection_turn_items LIMIT 1",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_items {
        return Ok(());
    }
    let has_run_counts: bool = conn
        .query_row(
            "SELECT 1 FROM orchestration_v2_projection_turn_item_counts LIMIT 1",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_run_counts {
        conn.execute_batch(
            "INSERT OR IGNORE INTO orchestration_v2_projection_turn_item_counts (thread_id, run_key, item_count)
             SELECT thread_id, COALESCE(run_id, ''), COUNT(*)
             FROM orchestration_v2_projection_turn_items
             GROUP BY thread_id, run_id;",
        )?;
    }
    let has_thread_counts: bool = conn
        .query_row(
            "SELECT 1 FROM orchestration_v2_projection_thread_item_counts LIMIT 1",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_thread_counts {
        conn.execute_batch(
            "INSERT OR IGNORE INTO orchestration_v2_projection_thread_item_counts (thread_id, item_count)
             SELECT c.thread_id, SUM(c.item_count)
             FROM orchestration_v2_projection_turn_item_counts AS c
             WHERE c.run_key = ''
                OR EXISTS (SELECT 1 FROM orchestration_v2_projection_runs AS r WHERE r.run_id = c.run_key)
             GROUP BY c.thread_id;",
        )?;
    }
    Ok(())
}
