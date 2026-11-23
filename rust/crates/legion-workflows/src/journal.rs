//! Replay journal persisted in `workflow_engine_journal` with 1-hour ended-run pruning.

use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::time::{format_millis_iso, now_iso_utc};
use crate::types::WorkflowError;
pub use crate::types_extra::{JournalEntryKind, JournalEntryStatus};

/// Default delay after a run ends before its replay journal rows are pruned (`1 hour`).
pub const JOURNAL_PRUNE_AFTER_MS: i64 = 3_600_000;
/// Maximum ended runs pruned per sweep pass.
pub const JOURNAL_PRUNE_BATCH: usize = 100;

/// One persisted journal record inside `workflow_engine_journal`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    /// Owning workflow run ID.
    pub run_id: String,
    /// Monotonic insertion sequence within the run.
    pub seq: i64,
    /// Unique key within the run (`step/<name>`, `sleep/<name>`, `wait/<name>`, ...).
    pub entry_key: String,
    /// Kind of entry.
    pub kind: JournalEntryKind,
    /// Status of entry.
    pub status: JournalEntryStatus,
    /// Serialized result or failure detail JSON.
    pub payload_json: Option<String>,
    /// Optional ISO-8601 wakeup deadline for `Sleep` entries.
    pub wake_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// SQLite-backed replay journal for durable workflow runs.
#[derive(Debug, Clone)]
pub struct WorkflowJournal {
    conn: Arc<Mutex<Connection>>,
}

impl WorkflowJournal {
    /// Wraps a shared SQLite connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Reads an entry by `(run_id, entry_key)`.
    pub fn get_entry(
        &self,
        run_id: &str,
        entry_key: &str,
    ) -> Result<Option<JournalEntry>, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT run_id, seq, entry_key, kind, status, payload_json, wake_at, created_at, updated_at
                 FROM workflow_engine_journal
                 WHERE run_id = ?1 AND entry_key = ?2",
                params![run_id, entry_key],
                row_to_entry,
            )
            .optional()
            .map_err(storage_err)
    }

    /// Lists all journal entries for `run_id` in insertion sequence order.
    pub fn list_entries(&self, run_id: &str) -> Result<Vec<JournalEntry>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT run_id, seq, entry_key, kind, status, payload_json, wake_at, created_at, updated_at
                 FROM workflow_engine_journal
                 WHERE run_id = ?1
                 ORDER BY seq ASC",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![run_id], row_to_entry)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Upserts a completed activity result so replays return `value` without re-execution.
    pub fn record_activity_success(
        &self,
        run_id: &str,
        entry_key: &str,
        value: &Value,
    ) -> Result<(), WorkflowError> {
        let payload = serde_json::to_string(value).map_err(storage_err)?;
        self.upsert_entry(
            run_id,
            entry_key,
            JournalEntryKind::Activity,
            JournalEntryStatus::Completed,
            Some(&payload),
            None,
        )
    }

    /// Upserts a failed activity entry causing the run to suspend until resumed.
    pub fn record_activity_failure(
        &self,
        run_id: &str,
        entry_key: &str,
        reason: &str,
    ) -> Result<(), WorkflowError> {
        let payload =
            serde_json::to_string(&serde_json::json!({ "reason": reason })).map_err(storage_err)?;
        self.upsert_entry(
            run_id,
            entry_key,
            JournalEntryKind::Activity,
            JournalEntryStatus::Failed,
            Some(&payload),
            None,
        )
    }

    /// Records a durable sleep wake timestamp.
    pub fn record_sleep_open(
        &self,
        run_id: &str,
        entry_key: &str,
        wake_at: &str,
    ) -> Result<(), WorkflowError> {
        self.upsert_entry(
            run_id,
            entry_key,
            JournalEntryKind::Sleep,
            JournalEntryStatus::Waiting,
            None,
            Some(wake_at),
        )
    }

    /// Marks a durable sleep completed.
    pub fn record_sleep_completed(
        &self,
        run_id: &str,
        entry_key: &str,
    ) -> Result<(), WorkflowError> {
        self.upsert_entry(
            run_id,
            entry_key,
            JournalEntryKind::Sleep,
            JournalEntryStatus::Completed,
            Some("null"),
            None,
        )
    }

    /// Returns `(run_id, entry_key)` pairs for sleeping entries whose `wake_at <= now_iso`.
    pub fn due_sleeps(&self, now_iso: &str) -> Result<Vec<(String, String)>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT j.run_id, j.entry_key
                 FROM workflow_engine_journal j
                 JOIN workflow_runs r ON r.run_id = j.run_id
                 WHERE j.kind = 'sleep' AND j.status = 'waiting'
                   AND j.wake_at IS NOT NULL AND j.wake_at <= ?1
                   AND r.ended_at IS NULL AND r.status != 'paused'",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![now_iso], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Clears failed entries for `run_id` when resuming a paused run so only failed steps re-run.
    pub fn clear_failed_entries(&self, run_id: &str) -> Result<usize, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .execute(
                "DELETE FROM workflow_engine_journal WHERE run_id = ?1 AND status = 'failed'",
                params![run_id],
            )
            .map_err(storage_err)
    }

    /// Deletes a specific journal entry (for example when a wait is reopened).
    pub fn delete_entry(&self, run_id: &str, entry_key: &str) -> Result<(), WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .execute(
                "DELETE FROM workflow_engine_journal WHERE run_id = ?1 AND entry_key = ?2",
                params![run_id, entry_key],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Deletes `workflow_engine_journal` entries for runs that ended before `now_ms - prune_after_ms`
    /// with no running steps left, and stamps `workflow_runs.engine_pruned_at`.
    pub fn prune_ended_runs(
        &self,
        now_ms: i64,
        prune_after_ms: i64,
        batch_limit: usize,
    ) -> Result<Vec<String>, WorkflowError> {
        let before_iso = format_millis_iso(now_ms.saturating_sub(prune_after_ms));
        let now_iso = format_millis_iso(now_ms);
        let limit_i64 = i64::try_from(batch_limit).unwrap_or(100);
        let mut guard = self.lock_conn()?;
        let tx = guard.transaction().map_err(storage_err)?;
        let mut run_ids = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT r.run_id
                     FROM workflow_runs r
                     WHERE r.ended_at IS NOT NULL AND r.ended_at < ?1 AND r.engine_pruned_at IS NULL
                       AND NOT EXISTS (
                         SELECT 1 FROM workflow_steps s
                         WHERE s.run_id = r.run_id AND s.status = 'running'
                       )
                     ORDER BY r.ended_at
                     LIMIT ?2",
                )
                .map_err(storage_err)?;
            let rows = stmt
                .query_map(params![before_iso, limit_i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(storage_err)?;
            for row in rows {
                run_ids.push(row.map_err(storage_err)?);
            }
        }
        for run_id in &run_ids {
            tx.execute(
                "DELETE FROM workflow_engine_journal WHERE run_id = ?1",
                params![run_id],
            )
            .map_err(storage_err)?;
            tx.execute(
                "UPDATE workflow_runs SET engine_pruned_at = ?1 WHERE run_id = ?2",
                params![now_iso, run_id],
            )
            .map_err(storage_err)?;
        }
        tx.commit().map_err(storage_err)?;
        Ok(run_ids)
    }

    fn upsert_entry(
        &self,
        run_id: &str,
        entry_key: &str,
        kind: JournalEntryKind,
        status: JournalEntryStatus,
        payload_json: Option<&str>,
        wake_at: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        let mut guard = self.lock_conn()?;
        let tx = guard.transaction().map_err(storage_err)?;
        let next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM workflow_engine_journal WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .map_err(storage_err)?;
        tx.execute(
            "INSERT INTO workflow_engine_journal (
               run_id, seq, entry_key, kind, status, payload_json, wake_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT (run_id, entry_key) DO UPDATE
             SET kind = excluded.kind,
                 status = excluded.status,
                 payload_json = excluded.payload_json,
                 wake_at = excluded.wake_at,
                 updated_at = excluded.updated_at",
            params![
                run_id,
                next_seq,
                entry_key,
                kind.as_str(),
                status.as_str(),
                payload_json,
                wake_at,
                at,
            ],
        )
        .map_err(storage_err)?;
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, WorkflowError> {
        self.conn.lock().map_err(|err| WorkflowError::Storage {
            message: format!("journal mutex poisoned: {err}"),
        })
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<JournalEntry> {
    let kind_raw: String = row.get(3)?;
    let status_raw: String = row.get(4)?;
    Ok(JournalEntry {
        run_id: row.get(0)?,
        seq: row.get(1)?,
        entry_key: row.get(2)?,
        kind: JournalEntryKind::parse(&kind_raw).unwrap_or(JournalEntryKind::Activity),
        status: JournalEntryStatus::parse(&status_raw).unwrap_or(JournalEntryStatus::Completed),
        payload_json: row.get(5)?,
        wake_at: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn storage_err(err: impl std::fmt::Display) -> WorkflowError {
    WorkflowError::Storage {
        message: err.to_string(),
    }
}
