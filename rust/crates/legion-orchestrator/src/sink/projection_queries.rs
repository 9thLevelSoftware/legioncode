//! Index-hinted `SQLite` queries for settled subagents and provider threads (`#37902`,
//! `apps/server/src/orchestration-v2/ProjectionStore.ts`).

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::sink::types::SinkError;

/// Subagent projection SQL fragment reading `run.status` via `runs_run_status_idx` (`#37902`).
pub const UNSETTLED_SUBAGENT_ROWIDS_SQL: &str = "\
SELECT rowid FROM orchestration_v2_projection_subagents AS sub
WHERE sub.thread_id = ?1
  AND (
    sub.status NOT IN ('completed', 'failed', 'interrupted', 'cancelled')
    OR COALESCE(sub.completion_delivery_state, '') NOT IN ('acknowledged', 'delivered', 'disposed')
    OR EXISTS (
      SELECT 1 FROM orchestration_v2_projection_runs AS run
        INDEXED BY orchestration_v2_projection_runs_run_status_idx
      WHERE run.run_id = sub.run_id
        AND run.status IN ('preparing', 'queued', 'starting', 'running', 'waiting')
    )
  )";

/// `todo_list` SQL predicate reading `todo_run.status` via `runs_run_status_idx` (`#37902`).
pub const TODO_LIST_SETTLED_RUN_PREDICATE_SQL: &str = "\
type = 'todo_list' AND run_id IS NOT NULL AND NOT EXISTS (
  SELECT 1 FROM orchestration_v2_projection_runs AS todo_run
    INDEXED BY orchestration_v2_projection_runs_run_status_idx
  WHERE todo_run.run_id = orchestration_v2_projection_turn_items.run_id
    AND todo_run.status IN ('queued', 'preparing', 'starting', 'running', 'waiting')
)";

/// Executes `UNSETTLED_SUBAGENT_ROWIDS_SQL` for `thread_id` and returns unsettled subagent payloads.
pub fn query_unsettled_subagents(
    conn: &Connection,
    thread_id: &str,
) -> Result<Vec<Value>, SinkError> {
    let sql = format!(
        "SELECT payload_json FROM orchestration_v2_projection_subagents
         WHERE rowid IN ({UNSETTLED_SUBAGENT_ROWIDS_SQL})
         ORDER BY updated_at ASC, subagent_id ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let raw_rows = stmt
        .query_map(params![thread_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    parse_json_rows(raw_rows)
}

/// Queries `todo_list` turn items whose owning run is already settled, using
/// `orchestration_v2_projection_runs_run_status_idx` (`#37902`).
pub fn query_settled_todo_turn_items(
    conn: &Connection,
    thread_id: &str,
) -> Result<Vec<Value>, SinkError> {
    let sql = format!(
        "SELECT payload_json FROM orchestration_v2_projection_turn_items
         WHERE thread_id = ?1
           AND {TODO_LIST_SETTLED_RUN_PREDICATE_SQL}
         ORDER BY ordinal ASC, turn_item_id ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let raw_rows = stmt
        .query_map(params![thread_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    parse_json_rows(raw_rows)
}

/// Queries provider threads visible to `thread_id`, using `UNION` seek branches + explicit
/// `INDEXED BY` hints when `omit_settled_subagent_records` is enabled (`#37902`).
pub fn query_provider_threads_for_thread(
    conn: &Connection,
    thread_id: &str,
    omit_settled_subagent_records: bool,
    provider_thread_ids: Option<&[String]>,
) -> Result<Vec<Value>, SinkError> {
    let where_body = if omit_settled_subagent_records {
        format!(
            "rowid IN (
              SELECT rowid FROM orchestration_v2_projection_provider_threads
              WHERE thread_id = ?1
              UNION
              SELECT rowid FROM orchestration_v2_projection_provider_threads
                INDEXED BY orchestration_v2_projection_provider_threads_owner_idx
              WHERE owner_node_id IN (
                SELECT node_id FROM orchestration_v2_projection_nodes WHERE thread_id = ?1
              )
              UNION
              SELECT rowid FROM orchestration_v2_projection_provider_threads
              WHERE provider_thread_id IN (
                SELECT provider_thread_id
                FROM orchestration_v2_projection_subagents
                WHERE rowid IN ({UNSETTLED_SUBAGENT_ROWIDS_SQL})
                  AND provider_thread_id IS NOT NULL
              )
              UNION
              SELECT rowid FROM orchestration_v2_projection_provider_threads AS shared
                INDEXED BY orchestration_v2_projection_provider_threads_session_idx
              WHERE provider_session_id IN (
                  SELECT provider_session_id
                  FROM orchestration_v2_projection_provider_session_bindings
                  WHERE thread_id = ?1
                )
                AND EXISTS (
                  SELECT 1 FROM orchestration_v2_projection_subagents AS owner
                  WHERE owner.provider_thread_id = shared.provider_thread_id
                    AND owner.thread_id = ?1
                )
            )"
        )
    } else {
        "(thread_id = ?1
           OR owner_node_id IN (
             SELECT node_id FROM orchestration_v2_projection_nodes WHERE thread_id = ?1
           )
           OR provider_thread_id IN (
             SELECT provider_thread_id
             FROM orchestration_v2_projection_subagents
             WHERE thread_id = ?1
               AND provider_thread_id IS NOT NULL
           ))"
        .to_owned()
    };

    let sql = if provider_thread_ids.is_some() {
        format!(
            "SELECT payload_json
             FROM orchestration_v2_projection_provider_threads
             WHERE {where_body}
               AND provider_thread_id IN (SELECT value FROM json_each(?2))
             ORDER BY COALESCE(first_run_ordinal, 0), provider_thread_id ASC"
        )
    } else {
        format!(
            "SELECT payload_json
             FROM orchestration_v2_projection_provider_threads
             WHERE {where_body}
             ORDER BY COALESCE(first_run_ordinal, 0), provider_thread_id ASC"
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let raw_rows = if let Some(filter_ids) = provider_thread_ids {
        let encoded = serde_json::to_string(filter_ids).unwrap_or_else(|_| "[]".to_owned());
        stmt.query_map(params![thread_id, encoded], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map(params![thread_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    parse_json_rows(raw_rows)
}

fn parse_json_rows(raw_rows: Vec<String>) -> Result<Vec<Value>, SinkError> {
    let mut out = Vec::with_capacity(raw_rows.len());
    for raw in raw_rows {
        let parsed: Value =
            serde_json::from_str(&raw).map_err(|err| SinkError::Codec(err.to_string()))?;
        out.push(parsed);
    }
    Ok(out)
}
