//! Run lifecycle extensions and row mapping helpers for [`WorkflowStore`].

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::store::{RunRow, WorkflowStore};
use crate::time::now_iso_utc;
use crate::types::{WorkflowError, WorkflowRun, WorkflowRunStatus};

impl WorkflowStore {
    /// Transitions a paused run back to `running`, optionally guarded against concurrent updates.
    pub fn run_unpaused(
        &self,
        run_id: &str,
        expected: Option<(i64, &str)>,
    ) -> Result<bool, WorkflowError> {
        let at = now_iso_utc();
        let updated = {
            let guard = self.lock_conn()?;
            let (flag, pauses, updated_at) = match expected {
                Some((p, u)) => (0_i64, p, u),
                None => (1_i64, 0, ""),
            };
            guard
                .query_row(
                    "UPDATE workflow_runs
                     SET status = 'running', detail = NULL, updated_at = ?1
                     WHERE run_id = ?2 AND ended_at IS NULL AND status = 'paused'
                       AND (?3 = 1 OR (pauses = ?4 AND updated_at = ?5))
                     RETURNING run_id",
                    params![at, run_id, flag, pauses, updated_at],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_err)?
        };
        self.notify_change(run_id);
        if updated.is_some() {
            self.tell(
                run_id,
                &format!("{run_id}:{at}"),
                &at,
                "workflow.run_resumed",
                |_| {},
            );
            return Ok(true);
        }
        Ok(false)
    }

    /// Clears `held` after a person-triggered resume reaches the engine.
    pub fn run_unheld(&self, run_id: &str) -> Result<(), WorkflowError> {
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_runs SET held = 0 WHERE run_id = ?1 AND status != 'paused'",
                    params![run_id],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Ends an open run atomically, deleting open waits and emitting `input_resolved`/`run_ended`.
    pub fn run_ended(
        &self,
        run_id: &str,
        status: WorkflowRunStatus,
        detail: Option<&str>,
        result: &Value,
    ) -> Result<bool, WorkflowError> {
        let at = now_iso_utc();
        let result_json = serde_json::to_string(result).map_err(storage_err)?;
        let (ended, closed_waits) = {
            let mut guard = self.lock_conn()?;
            let tx = guard.transaction().map_err(storage_err)?;
            let updated: Option<String> = tx
                .query_row(
                    "UPDATE workflow_runs
                     SET status = ?1, detail = ?2, result_json = ?3,
                         pauses = 0, updated_at = ?4, ended_at = ?4
                     WHERE run_id = ?5 AND ended_at IS NULL
                     RETURNING run_id",
                    params![status.as_str(), detail, result_json, at, run_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?;
            let mut closed = Vec::new();
            {
                let mut stmt = tx
                    .prepare(
                        "DELETE FROM workflow_waits WHERE run_id = ?1
                         RETURNING name, request_json, opened_at",
                    )
                    .map_err(storage_err)?;
                let rows = stmt
                    .query_map(params![run_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(storage_err)?;
                for row in rows {
                    closed.push(row.map_err(storage_err)?);
                }
            }
            tx.commit().map_err(storage_err)?;
            (updated.is_some(), closed)
        };
        self.notify_change(run_id);
        if !ended {
            return Ok(false);
        }
        for (wait_name, request_json, opened_at) in closed_waits {
            if request_json.is_some() {
                self.tell(
                    run_id,
                    &format!("{run_id}:{wait_name}:{opened_at}"),
                    &at,
                    "workflow.input_resolved",
                    |ev| {
                        ev.step = Some(wait_name.clone());
                        ev.resolution = Some("cancelled".to_owned());
                    },
                );
            }
        }
        self.tell(run_id, run_id, &at, "workflow.run_ended", |ev| {
            ev.outcome = Some(status.as_str().to_owned());
            ev.detail = detail.map(str::to_owned);
        });
        Ok(true)
    }

    /// Sets a human-readable run title capped at 200 characters.
    pub fn run_title(&self, run_id: &str, title: &str) -> Result<(), WorkflowError> {
        let capped = truncate_chars(title, 200);
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_runs SET title = ?1 WHERE run_id = ?2",
                    params![capped, run_id],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Converts a [`RunRow`] into a wire [`WorkflowRun`].
    pub fn to_wire_run(row: &RunRow, retired: bool, resumes_at: Option<String>) -> WorkflowRun {
        WorkflowRun {
            run_id: row.run_id.clone(),
            workflow: row.workflow.clone(),
            version: row.version,
            title: row.title.clone(),
            status: row.status,
            detail: row.detail.clone(),
            result: row
                .result_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok()),
            pauses: row.pauses,
            retired,
            resumes_at,
            parent_run_id: row.parent_run_id.clone(),
            parent_step: row.parent_wait.clone(),
            started_at: row.started_at.clone(),
            updated_at: row.updated_at.clone(),
            ended_at: row.ended_at.clone(),
        }
    }
}

pub(crate) fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let status_raw: String = row.get(6)?;
    Ok(RunRow {
        run_id: row.get(0)?,
        workflow: row.get(1)?,
        idempotency_key: row.get(2)?,
        version: row.get(3)?,
        engine_tag: row.get(4)?,
        title: row.get(5)?,
        status: WorkflowRunStatus::parse(&status_raw).unwrap_or(WorkflowRunStatus::Running),
        detail: row.get(7)?,
        result_json: row.get(8)?,
        input_json: row.get(9)?,
        parent_run_id: row.get(10)?,
        parent_wait: row.get(11)?,
        pauses: row.get(12)?,
        held: row.get(13)?,
        started_at: row.get(14)?,
        updated_at: row.get(15)?,
        ended_at: row.get(16)?,
    })
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

pub(crate) fn storage_err(err: impl std::fmt::Display) -> WorkflowError {
    WorkflowError::Storage {
        message: err.to_string(),
    }
}
