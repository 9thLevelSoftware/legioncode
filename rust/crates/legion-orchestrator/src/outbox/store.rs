//! `SQLite` persistence operations for `orchestration_v2_effect_outbox`.

use std::sync::{Arc, Mutex};

use legion_contracts::ids::{CommandId, ThreadId};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use serde_json::Value;

use crate::outbox::maintenance::placeholders;
use crate::outbox::types::{
    OutboxEffect, OutboxEffectRequest, OutboxStatus, OutboxStoreError, PendingOutboxEffect,
};
use crate::time::add_millis_iso;

const SELECT_OUTBOX_COLS: &str = "effect_id, command_id, thread_id, effect_type, payload_json, \
    status, attempt_count, available_at, lease_owner, created_at, updated_at, completed_at, last_error";

const CLAIMABLE_FILTER_SQL: &str = "
  FROM orchestration_v2_effect_outbox AS candidate
  WHERE candidate.status = 'pending'
    AND candidate.available_at <= ?1
    AND NOT EXISTS (
      SELECT 1
      FROM orchestration_v2_effect_outbox AS active
      WHERE active.thread_id = candidate.thread_id
        AND (
          active.status = 'running'
          OR (active.status = 'pending' AND active.rowid < candidate.rowid)
        )
        AND (
          (candidate.effect_type = 'thread-title.generate' AND active.effect_type = 'thread-title.generate')
          OR (candidate.effect_type != 'thread-title.generate' AND active.effect_type != 'thread-title.generate')
        )
    )";

const CLAIM_NEXT_SQL: &str = "
UPDATE orchestration_v2_effect_outbox
SET status = 'running', attempt_count = attempt_count + 1,
    lease_owner = ?1, updated_at = ?2, last_error = NULL
WHERE effect_id = (
  SELECT candidate.effect_id
  FROM orchestration_v2_effect_outbox AS candidate
  WHERE candidate.status = 'pending'
    AND candidate.available_at <= ?2
    AND NOT EXISTS (
      SELECT 1
      FROM orchestration_v2_effect_outbox AS active
      WHERE active.thread_id = candidate.thread_id
        AND (
          active.status = 'running'
          OR (active.status = 'pending' AND active.rowid < candidate.rowid)
        )
        AND (
          (candidate.effect_type = 'thread-title.generate' AND active.effect_type = 'thread-title.generate')
          OR (candidate.effect_type != 'thread-title.generate' AND active.effect_type != 'thread-title.generate')
        )
    )
  ORDER BY candidate.available_at ASC, candidate.created_at ASC, candidate.effect_id ASC
  LIMIT 1
)
RETURNING effect_id, command_id, thread_id, effect_type, payload_json, status,
          attempt_count, available_at, lease_owner, created_at, updated_at,
          completed_at, last_error
";

/// Inserts pending outbox effects inside the caller's active `SQLite` transaction.
pub fn enqueue_effects(
    conn: &Connection,
    effects: &[PendingOutboxEffect],
    now_iso: &str,
) -> Result<(), OutboxStoreError> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO orchestration_v2_effect_outbox (
           effect_id, command_id, thread_id, effect_type, payload_json,
           status, attempt_count, available_at, created_at, updated_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, ?6, ?7, ?7)
         ON CONFLICT(effect_id) DO NOTHING",
    )?;
    for effect in effects {
        let payload_json = serde_json::to_string(&effect.request)
            .map_err(|err| OutboxStoreError::Decode(err.to_string()))?;
        let available_at = effect.available_at.as_deref().unwrap_or(now_iso);
        stmt.execute(params![
            effect.id,
            effect.command_id.as_str(),
            effect.thread_id.as_str(),
            effect.request.effect_type(),
            payload_json,
            available_at,
            now_iso,
        ])?;
    }
    Ok(())
}

/// Cancels unsettled (`pending`/`running`) effects for `thread_ids` matching `effect_types`.
pub fn cancel_unsettled_effects(
    conn: &Connection,
    thread_ids: &[ThreadId],
    effect_types: &[&str],
    reason: &str,
    now_iso: &str,
) -> Result<Vec<String>, OutboxStoreError> {
    if thread_ids.is_empty() || effect_types.is_empty() {
        return Ok(Vec::new());
    }
    let thread_placeholders = placeholders(4, thread_ids.len());
    let type_placeholders = placeholders(4 + thread_ids.len(), effect_types.len());
    let sql = format!(
        "UPDATE orchestration_v2_effect_outbox
         SET status = 'cancelled', lease_owner = NULL, completed_at = ?1,
             updated_at = ?1, last_error = ?2
         WHERE status IN ('pending', 'running')
           AND thread_id IN ({thread_placeholders})
           AND effect_type IN ({type_placeholders})
         RETURNING effect_id"
    );
    let mut values = Vec::with_capacity(2 + thread_ids.len() + effect_types.len());
    values.push(SqlValue::Text(now_iso.to_owned()));
    values.push(SqlValue::Text(reason.to_owned()));
    for tid in thread_ids {
        values.push(SqlValue::Text(tid.as_str().to_owned()));
    }
    for et in effect_types {
        values.push(SqlValue::Text((*et).to_owned()));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(values), |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(crate) fn claim_next_effect(
    conn: &Connection,
    worker_id: &str,
    now_iso: &str,
) -> Result<Option<OutboxEffect>, OutboxStoreError> {
    let mut stmt = conn.prepare_cached(CLAIM_NEXT_SQL)?;
    let row = stmt
        .query_row(params![worker_id, now_iso], row_to_effect_tuple)
        .optional()?;
    row.map(decode_effect_tuple).transpose()
}

pub(crate) fn has_claimable_work(
    conn: &Connection,
    now_iso: &str,
) -> Result<bool, OutboxStoreError> {
    let sql = format!("SELECT EXISTS (SELECT 1 {CLAIMABLE_FILTER_SQL})");
    let exists: i64 = conn.query_row(&sql, params![now_iso], |row| row.get(0))?;
    Ok(exists != 0)
}

pub(crate) fn mark_succeeded(
    conn: &Connection,
    effect_id: &str,
    worker_id: &str,
    now_iso: &str,
) -> Result<bool, OutboxStoreError> {
    let changed = conn.execute(
        "UPDATE orchestration_v2_effect_outbox
         SET status = 'succeeded', lease_owner = NULL, completed_at = ?1,
             updated_at = ?1, last_error = NULL
         WHERE effect_id = ?2 AND status = 'running' AND lease_owner = ?3",
        params![now_iso, effect_id, worker_id],
    )?;
    Ok(changed == 1)
}

pub(crate) fn mark_retry(
    conn: &Connection,
    effect_id: &str,
    worker_id: &str,
    error: &str,
    delay_ms: i64,
    now_iso: &str,
) -> Result<bool, OutboxStoreError> {
    let available_at =
        add_millis_iso(now_iso, delay_ms.max(0)).unwrap_or_else(|| now_iso.to_owned());
    let changed = conn.execute(
        "UPDATE orchestration_v2_effect_outbox
         SET status = 'pending', available_at = ?1, lease_owner = NULL,
             updated_at = ?2, last_error = ?3
         WHERE effect_id = ?4 AND status = 'running' AND lease_owner = ?5",
        params![available_at, now_iso, error, effect_id, worker_id],
    )?;
    Ok(changed == 1)
}

pub(crate) fn mark_failed(
    conn: &Connection,
    effect_id: &str,
    worker_id: &str,
    error: &str,
    now_iso: &str,
) -> Result<bool, OutboxStoreError> {
    let changed = conn.execute(
        "UPDATE orchestration_v2_effect_outbox
         SET status = 'failed', lease_owner = NULL, completed_at = ?1,
             updated_at = ?1, last_error = ?2
         WHERE effect_id = ?3 AND status = 'running' AND lease_owner = ?4",
        params![now_iso, error, effect_id, worker_id],
    )?;
    Ok(changed == 1)
}

/// Fetches a single outbox row by `effect_id`.
pub fn get_effect(
    conn: &Connection,
    effect_id: &str,
) -> Result<Option<OutboxEffect>, OutboxStoreError> {
    let sql = format!(
        "SELECT {SELECT_OUTBOX_COLS} FROM orchestration_v2_effect_outbox WHERE effect_id = ?1 LIMIT 1"
    );
    let row = conn
        .query_row(&sql, params![effect_id], row_to_effect_tuple)
        .optional()?;
    row.map(decode_effect_tuple).transpose()
}

/// Lists all outbox rows created by `command_id` in deterministic creation order.
pub fn list_by_command_id(
    conn: &Connection,
    command_id: &CommandId,
) -> Result<Vec<OutboxEffect>, OutboxStoreError> {
    let sql = format!(
        "SELECT {SELECT_OUTBOX_COLS} FROM orchestration_v2_effect_outbox
         WHERE command_id = ?1 ORDER BY created_at ASC, effect_id ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let tuples = stmt
        .query_map(params![command_id.as_str()], row_to_effect_tuple)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tuples.into_iter().map(decode_effect_tuple).collect()
}

pub(crate) fn with_locked_conn<T>(
    db: &Arc<Mutex<Connection>>,
    f: impl FnOnce(&Connection) -> Result<T, OutboxStoreError>,
) -> Result<T, OutboxStoreError> {
    let guard = db.lock().map_err(|_| OutboxStoreError::LockPoisoned)?;
    f(&guard)
}

type EffectTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn row_to_effect_tuple(row: &rusqlite::Row<'_>) -> rusqlite::Result<EffectTuple> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

fn decode_effect_tuple(row: EffectTuple) -> Result<OutboxEffect, OutboxStoreError> {
    let command_id =
        CommandId::new(row.1).map_err(|err| OutboxStoreError::Decode(err.to_string()))?;
    let thread_id =
        ThreadId::new(row.2).map_err(|err| OutboxStoreError::Decode(err.to_string()))?;
    let raw_payload: Value =
        serde_json::from_str(&row.4).map_err(|err| OutboxStoreError::Decode(err.to_string()))?;
    let request: OutboxEffectRequest = serde_json::from_value(raw_payload.clone())
        .map_err(|err| OutboxStoreError::Decode(err.to_string()))?;
    let status = OutboxStatus::from_db(&row.5)?;
    let attempt_count = u64::try_from(row.6.max(0)).unwrap_or(0);
    Ok(OutboxEffect {
        id: row.0,
        command_id,
        thread_id,
        effect_type: row.3,
        request,
        raw_payload,
        status,
        attempt_count,
        available_at: row.7,
        lease_owner: row.8,
        created_at: row.9,
        updated_at: row.10,
        completed_at: row.11,
        last_error: row.12,
    })
}
