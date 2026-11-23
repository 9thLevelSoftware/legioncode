//! Read queries on `workflow_runs` implemented on [`WorkflowStore`].

use rusqlite::{OptionalExtension, params};

use crate::store::{RunRow, WorkflowStore, row_to_run, storage_err};
use crate::types::{RunFilter, WorkflowError, WorkflowReferenceInput, WorkflowRunStatus};

pub(crate) type ExistingRefRow = (String, String, Option<String>, Option<String>, String);

impl WorkflowStore {
    /// Reads one run by ID.
    pub fn run(&self, run_id: &str) -> Result<Option<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                        detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                        started_at, updated_at, ended_at
                 FROM workflow_runs WHERE run_id = ?1",
                params![run_id],
                row_to_run,
            )
            .optional()
            .map_err(storage_err)
    }

    /// Finds the unique run registered under `(workflow, key)` across versions.
    pub fn run_by_key(&self, workflow: &str, key: &str) -> Result<Option<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                        detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                        started_at, updated_at, ended_at
                 FROM workflow_runs
                 WHERE workflow = ?1 AND idempotency_key = ?2
                 ORDER BY started_at LIMIT 1",
                params![workflow, key],
                row_to_run,
            )
            .optional()
            .map_err(storage_err)
    }

    /// Lists recent runs, keeping top-level `skipped` runs in a separate 500-row slice when unfiltered.
    pub fn runs(&self, status: Option<WorkflowRunStatus>) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let sql = match status {
            None => {
                "SELECT * FROM (
                   SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                          detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                          started_at, updated_at, ended_at
                   FROM workflow_runs
                   WHERE NOT (status = 'skipped' AND parent_run_id IS NULL)
                   ORDER BY started_at DESC LIMIT 500
                 )
                 UNION ALL
                 SELECT * FROM (
                   SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                          detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                          started_at, updated_at, ended_at
                   FROM workflow_runs
                   WHERE status = 'skipped' AND parent_run_id IS NULL
                   ORDER BY started_at DESC LIMIT 500
                 )
                 ORDER BY started_at DESC"
            }
            Some(_) => {
                "SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                        detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                        started_at, updated_at, ended_at
                 FROM workflow_runs WHERE status = ?1
                 ORDER BY started_at DESC LIMIT 500"
            }
        };
        let mut stmt = guard.prepare(sql).map_err(storage_err)?;
        let mut out = Vec::new();
        if let Some(st) = status {
            let rows = stmt
                .query_map(params![st.as_str()], row_to_run)
                .map_err(storage_err)?;
            for row in rows {
                out.push(row.map_err(storage_err)?);
            }
        } else {
            let rows = stmt.query_map([], row_to_run).map_err(storage_err)?;
            for row in rows {
                out.push(row.map_err(storage_err)?);
            }
        }
        Ok(out)
    }

    /// Queries runs matching optional workflow, status, and idempotency-key prefix filters.
    pub fn runs_where(&self, filter: &RunFilter) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let wf = filter.workflow.as_deref();
        let status = filter.status.map(WorkflowRunStatus::as_str);
        let prefix = filter.key_prefix.as_deref();
        let upper = prefix.map(|p| format!("{p}\u{10FFFF}"));
        let mut stmt = guard
            .prepare(
                "SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                        detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                        started_at, updated_at, ended_at
                 FROM workflow_runs
                 WHERE (?1 IS NULL OR workflow = ?1)
                   AND (?2 IS NULL OR status = ?2)
                   AND (?3 IS NULL OR (idempotency_key >= ?3 AND idempotency_key < ?4))
                 ORDER BY started_at DESC, rowid DESC LIMIT 500",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![wf, status, prefix, upper], row_to_run)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Returns `(workflow, version)` pairs for every unfinished run (without list limits).
    pub fn unfinished(&self) -> Result<Vec<(String, i64)>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare("SELECT workflow, version FROM workflow_runs WHERE ended_at IS NULL")
            .map_err(storage_err)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Returns all runs currently in `status`.
    pub fn every_run(&self, status: WorkflowRunStatus) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                        detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                        started_at, updated_at, ended_at
                 FROM workflow_runs WHERE status = ?1",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![status.as_str()], row_to_run)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Returns unfinished child runs invoked by `run_id`.
    pub fn children(&self, run_id: &str) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT run_id, workflow, idempotency_key, version, engine_tag, title, status,
                        detail, result_json, input_json, parent_run_id, parent_wait, pauses, held,
                        started_at, updated_at, ended_at
                 FROM workflow_runs WHERE parent_run_id = ?1 AND ended_at IS NULL",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![run_id], row_to_run)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Returns unfinished child runs whose parent is already `cancelled`.
    pub fn orphans(&self) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT c.run_id, c.workflow, c.idempotency_key, c.version, c.engine_tag, c.title,
                        c.status, c.detail, c.result_json, c.input_json, c.parent_run_id,
                        c.parent_wait, c.pauses, c.held, c.started_at, c.updated_at, c.ended_at
                 FROM workflow_runs c
                 JOIN workflow_runs p ON p.run_id = c.parent_run_id
                 WHERE c.ended_at IS NULL AND p.status = 'cancelled'",
            )
            .map_err(storage_err)?;
        let rows = stmt.query_map([], row_to_run).map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Returns ended child runs whose parent's invoke wait has not yet been signalled.
    pub fn untold(&self) -> Result<Vec<RunRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT c.run_id, c.workflow, c.idempotency_key, c.version, c.engine_tag, c.title,
                        c.status, c.detail, c.result_json, c.input_json, c.parent_run_id,
                        c.parent_wait, c.pauses, c.held, c.started_at, c.updated_at, c.ended_at
                 FROM workflow_waits w
                 JOIN workflow_runs c ON c.parent_run_id = w.run_id AND c.parent_wait = w.name
                 WHERE w.signalled_at IS NULL AND c.ended_at IS NOT NULL",
            )
            .map_err(storage_err)?;
        let rows = stmt.query_map([], row_to_run).map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal SQLite transaction helper mirrors reference row columns"
)]
pub(crate) fn apply_reference_upsert(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    step: Option<&str>,
    reference: &WorkflowReferenceInput,
    address_json: &str,
    title: &str,
    at: &str,
    existing: Option<ExistingRefRow>,
) -> Result<bool, WorkflowError> {
    match existing {
        None => {
            let steps = step.map_or_else(Vec::new, |s| vec![s]);
            let steps_json = serde_json::to_string(&steps).map_err(storage_err)?;
            let origin = if step.is_none() { "trigger" } else { "step" };
            tx.execute(
                "INSERT INTO workflow_run_references (
                   run_id, kind, key, address_json, title, url, state, steps_json, origin, noted_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    run_id,
                    reference.kind,
                    reference.key,
                    address_json,
                    title,
                    reference.url,
                    reference.state,
                    steps_json,
                    origin,
                    at,
                ],
            )
            .map_err(storage_err)?;
            Ok(true)
        }
        Some((prev_addr, prev_title, prev_url, prev_state, prev_steps_json)) => {
            let mut steps: Vec<String> = serde_json::from_str(&prev_steps_json).unwrap_or_default();
            let next_steps_json = if let Some(s) = step
                && !steps.iter().any(|existing_step| existing_step == s)
            {
                steps.push(s.to_owned());
                serde_json::to_string(&steps).map_err(storage_err)?
            } else {
                prev_steps_json.clone()
            };
            let next_title = if title.is_empty() {
                prev_title.as_str()
            } else {
                title
            };
            let next_url = reference.url.as_deref().or(prev_url.as_deref());
            let next_state = reference.state.as_deref().or(prev_state.as_deref());
            if address_json == prev_addr
                && next_title == prev_title
                && next_url == prev_url.as_deref()
                && next_state == prev_state.as_deref()
                && next_steps_json == prev_steps_json
            {
                return Ok(false);
            }
            tx.execute(
                "UPDATE workflow_run_references
                 SET address_json = ?1, title = ?2, url = ?3, state = ?4, steps_json = ?5
                 WHERE run_id = ?6 AND kind = ?7 AND key = ?8",
                params![
                    address_json,
                    next_title,
                    next_url,
                    next_state,
                    next_steps_json,
                    run_id,
                    reference.kind,
                    reference.key,
                ],
            )
            .map_err(storage_err)?;
            Ok(true)
        }
    }
}
