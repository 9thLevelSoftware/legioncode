//! Step, mark, reference, trigger, hook, key-value store, and query helpers on [`WorkflowStore`].

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::store::{EndingRunRow, RunRow, WorkflowStore, row_to_run, storage_err, truncate_chars};
use crate::time::now_iso_utc;
use crate::types::{
    WorkflowError, WorkflowQuestionThread, WorkflowStep, WorkflowStepKind, WorkflowStepStatus,
};

const STEP_RESULT_KEPT: usize = 8_000;

/// Active thread info for interrupting running agent steps of a cancelled run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningStepThread {
    /// Thread ID.
    pub thread_id: String,
    /// Optional linked environment ID.
    pub environment: Option<String>,
    /// Step's turn ID.
    pub turn_id: Option<String>,
}

impl WorkflowStore {
    /// Records step start/retry and returns the updated attempt count.
    pub fn step_started(
        &self,
        run_id: &str,
        name: &str,
        kind: WorkflowStepKind,
    ) -> Result<i64, WorkflowError> {
        let at = now_iso_utc();
        let attempts: i64 = {
            let guard = self.lock_conn()?;
            guard
                .query_row(
                    "INSERT INTO workflow_steps (run_id, name, kind, status, attempts, started_at)
                     VALUES (?1, ?2, ?3, 'running', 1, ?4)
                     ON CONFLICT (run_id, name) DO UPDATE
                     SET status = 'running', attempts = attempts + 1, detail = NULL, ended_at = NULL,
                         started_at = excluded.started_at
                     RETURNING attempts",
                    params![run_id, name, kind.as_str(), at],
                    |row| row.get(0),
                )
                .map_err(storage_err)?
        };
        self.notify_change(run_id);
        Ok(attempts)
    }

    /// Marks a step `completed` or `failed`, resetting `pauses = 0` on a non-paused run when completed.
    pub fn step_ended(
        &self,
        run_id: &str,
        name: &str,
        status: WorkflowStepStatus,
        detail: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        let capped = detail.map(|d| truncate_chars(d, 2_000));
        {
            let mut guard = self.lock_conn()?;
            let tx = guard.transaction().map_err(storage_err)?;
            tx.execute(
                "UPDATE workflow_steps
                 SET status = ?1, detail = ?2, ended_at = ?3
                 WHERE run_id = ?4 AND name = ?5",
                params![status.as_str(), capped, at, run_id, name],
            )
            .map_err(storage_err)?;
            if status == WorkflowStepStatus::Completed {
                tx.execute(
                    "UPDATE workflow_runs SET pauses = 0
                     WHERE run_id = ?1 AND ended_at IS NULL AND status != 'paused'",
                    params![run_id],
                )
                .map_err(storage_err)?;
            }
            tx.commit().map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Stores step result JSON (capped at 8,000 chars) and optional agent closing words.
    pub fn step_result(
        &self,
        run_id: &str,
        name: &str,
        value: &Value,
        said: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let full = serde_json::to_string(value).map_err(storage_err)?;
        let kept = if full.chars().count() <= STEP_RESULT_KEPT {
            full
        } else {
            let prefix: String = full.chars().take(STEP_RESULT_KEPT).collect();
            serde_json::to_string(&format!("{prefix}…")).map_err(storage_err)?
        };
        let kept_said = said.and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.chars().count() <= STEP_RESULT_KEPT {
                Some(trimmed.to_owned())
            } else {
                let prefix: String = trimmed.chars().take(STEP_RESULT_KEPT).collect();
                Some(format!("{prefix}…"))
            }
        });
        let guard = self.lock_conn()?;
        guard
            .execute(
                "UPDATE workflow_steps SET result_json = ?1, said = ?2
                 WHERE run_id = ?3 AND name = ?4",
                params![kept, kept_said, run_id, name],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    /// Updates a step's human-readable detail (such as sleep wake time or wait reopen problem).
    pub fn step_detail(&self, run_id: &str, name: &str, detail: &str) -> Result<(), WorkflowError> {
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_steps SET detail = ?1 WHERE run_id = ?2 AND name = ?3",
                    params![detail, run_id, name],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Associates a launched/continued thread and turn with an agent step.
    pub fn step_thread(
        &self,
        run_id: &str,
        name: &str,
        thread_id: &str,
        environment_id: Option<&str>,
        turn_id: &str,
    ) -> Result<(), WorkflowError> {
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_steps
                     SET thread_id = ?1, environment_id = ?2, turn_id = ?3
                     WHERE run_id = ?4 AND name = ?5",
                    params![thread_id, environment_id, turn_id, run_id, name],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Lists running agent threads for `run_id`.
    pub fn running_threads(&self, run_id: &str) -> Result<Vec<RunningStepThread>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT thread_id, environment_id, turn_id
                 FROM workflow_steps
                 WHERE run_id = ?1 AND status = 'running' AND thread_id IS NOT NULL",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok(RunningStepThread {
                    thread_id: row.get(0)?,
                    environment: row.get(1)?,
                    turn_id: row.get(2)?,
                })
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Returns the most recently started agent thread of `run_id`, if any.
    pub fn latest_thread(
        &self,
        run_id: &str,
    ) -> Result<Option<WorkflowQuestionThread>, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT thread_id, environment_id FROM workflow_steps
                 WHERE run_id = ?1 AND thread_id IS NOT NULL
                 ORDER BY started_at DESC, rowid DESC LIMIT 1",
                params![run_id],
                |row| {
                    Ok(WorkflowQuestionThread {
                        thread_id: row.get(0)?,
                        environment_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(storage_err)
    }

    /// Marks all running steps of a cancelled run failed except `except` (`"onEnd"`).
    pub fn steps_cancelled(&self, run_id: &str, except: &str) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_steps
                     SET status = 'failed', detail = 'cancelled', ended_at = ?1
                     WHERE run_id = ?2 AND status = 'running' AND name != ?3",
                    params![at, run_id, except],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Lists steps for `run_id` in start order.
    pub fn steps(&self, run_id: &str) -> Result<Vec<WorkflowStep>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT name, kind, status, attempts, detail, thread_id, environment_id,
                        result_json, said, started_at, ended_at
                 FROM workflow_steps
                 WHERE run_id = ?1 ORDER BY started_at, rowid",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                let kind_raw: String = row.get(1)?;
                let status_raw: String = row.get(2)?;
                let result_raw: Option<String> = row.get(7)?;
                Ok(WorkflowStep {
                    name: row.get(0)?,
                    kind: WorkflowStepKind::parse(&kind_raw).unwrap_or(WorkflowStepKind::Step),
                    status: WorkflowStepStatus::parse(&status_raw)
                        .unwrap_or(WorkflowStepStatus::Running),
                    attempts: row.get(3)?,
                    detail: row.get(4)?,
                    thread_id: row.get(5)?,
                    environment_id: row.get(6)?,
                    result: result_raw
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok()),
                    said: row.get(8)?,
                    started_at: row.get(9)?,
                    ended_at: row.get(10)?,
                })
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Cancelled runs whose `onEnd` step has not completed and has retries left.
    pub fn runs_to_end(
        &self,
        step: &str,
        max_tries: i64,
    ) -> Result<Vec<EndingRunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT r.run_id, r.workflow, r.idempotency_key, r.version, r.engine_tag, r.title,
                        r.status, r.detail, r.result_json, r.input_json, r.parent_run_id,
                        r.parent_wait, r.pauses, r.held, r.started_at, r.updated_at, r.ended_at,
                        COALESCE(s.attempts, 0) AS end_attempts, s.ended_at AS end_ended_at
                 FROM workflow_runs r
                 LEFT JOIN workflow_steps s ON s.run_id = r.run_id AND s.name = ?1
                 WHERE r.status = 'cancelled'
                   AND (s.status IS NULL OR (s.status != 'completed' AND s.attempts < ?2))",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![step, max_tries], |row| {
                Ok(EndingRunRow {
                    run: row_to_run(row)?,
                    end_attempts: row.get(17)?,
                    end_ended_at: row.get(18)?,
                })
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Cancelled runs that still have non-`onEnd` steps marked `running`.
    pub fn cancels_cut_short(&self, except: &str) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT r.run_id, r.workflow, r.idempotency_key, r.version, r.engine_tag, r.title,
                        r.status, r.detail, r.result_json, r.input_json, r.parent_run_id,
                        r.parent_wait, r.pauses, r.held, r.started_at, r.updated_at, r.ended_at
                 FROM workflow_runs r
                 WHERE r.status = 'cancelled' AND EXISTS (
                   SELECT 1 FROM workflow_steps s
                   WHERE s.run_id = r.run_id AND s.status = 'running' AND s.name != ?1
                 )",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![except], row_to_run)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Atomically increments a run mark counter (`workflow_marks`) and returns the new count.
    pub fn mark(&self, run_id: &str, name: &str) -> Result<i64, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "INSERT INTO workflow_marks (run_id, name, count) VALUES (?1, ?2, 1)
                 ON CONFLICT (run_id, name) DO UPDATE SET count = count + 1
                 RETURNING count",
                params![run_id, name],
                |row| row.get(0),
            )
            .map_err(storage_err)
    }
}
