//! Process-loss recovery and 7-day retention pruning for `orchestration_v2_effect_outbox`.

use rusqlite::{Connection, params, params_from_iter, types::Value as SqlValue};

use crate::outbox::types::{
    OutboxStoreError, PROCESS_BOUND_EFFECT_TYPES, REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS,
};
use crate::time::{format_millis_iso, parse_iso_millis};

const SETTLED_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// Reconciles in-flight outbox rows after an unclean process restart (`EffectOutbox.ts:549-577`).
pub fn reconcile_after_process_loss(
    conn: &Connection,
    now_iso: &str,
) -> Result<(usize, usize), OutboxStoreError> {
    let process_ph = placeholders(2, PROCESS_BOUND_EFFECT_TYPES.len());
    let cancel_sql = format!(
        "UPDATE orchestration_v2_effect_outbox
         SET status = 'cancelled', lease_owner = NULL, completed_at = ?1, updated_at = ?1,
             last_error = 'Cancelled because the server process ended before the effect completed.'
         WHERE status IN ('pending', 'running') AND effect_type IN ({process_ph})"
    );
    let mut cancel_vals = Vec::with_capacity(1 + PROCESS_BOUND_EFFECT_TYPES.len());
    cancel_vals.push(SqlValue::Text(now_iso.to_owned()));
    for et in PROCESS_BOUND_EFFECT_TYPES {
        cancel_vals.push(SqlValue::Text((*et).to_owned()));
    }
    let cancelled = conn.execute(&cancel_sql, params_from_iter(cancel_vals))?;

    let replay_ph = placeholders(2, REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS.len());
    let requeue_sql = format!(
        "UPDATE orchestration_v2_effect_outbox
         SET status = 'pending', lease_owner = NULL, available_at = ?1, updated_at = ?1,
             last_error = 'Requeued after the previous server process ended.'
         WHERE status = 'running' AND effect_type IN ({replay_ph})"
    );
    let mut requeue_vals =
        Vec::with_capacity(1 + REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS.len());
    requeue_vals.push(SqlValue::Text(now_iso.to_owned()));
    for et in REPLAY_SAFE_EFFECT_TYPES_AFTER_PROCESS_LOSS {
        requeue_vals.push(SqlValue::Text((*et).to_owned()));
    }
    let requeued = conn.execute(&requeue_sql, params_from_iter(requeue_vals))?;
    Ok((requeued, cancelled))
}

/// Prunes settled (`succeeded`/`cancelled`) outbox rows older than 7 days.
pub fn prune_settled_effects(conn: &Connection, now_iso: &str) -> Result<usize, OutboxStoreError> {
    let now_ms = parse_iso_millis(now_iso)
        .ok_or_else(|| OutboxStoreError::Decode(format!("invalid ISO timestamp `{now_iso}`")))?;
    let cutoff = format_millis_iso(now_ms.saturating_sub(SETTLED_RETENTION_MS));
    let deleted = conn.execute(
        "DELETE FROM orchestration_v2_effect_outbox
         WHERE status IN ('succeeded', 'cancelled') AND completed_at < ?1",
        params![cutoff],
    )?;
    Ok(deleted)
}

pub(crate) fn placeholders(start_idx: usize, count: usize) -> String {
    (0..count)
        .map(|offset| format!("?{}", start_idx + offset))
        .collect::<Vec<_>>()
        .join(", ")
}
