//! Wait queries (`waits_on`, `waits_past_deadline`, `questions`, etc.) on [`WorkflowStore`].

use rusqlite::params;
use serde_json::Value;

use crate::store::{WorkflowStore, storage_err};
use crate::types::{WorkflowError, WorkflowQuestion};
use crate::waits_mail::WaitRow;

impl WorkflowStore {
    /// Lists all open waits listening on `channel` (optionally scoped to `workflow`).
    pub fn waits_on(
        &self,
        channel: &str,
        workflow: Option<&str>,
    ) -> Result<Vec<WaitRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                        watch_json, deadline, opened_at, woke_json
                 FROM workflow_waits
                 WHERE key = ?1 AND signalled_at IS NULL
                   AND (?2 IS NULL OR workflow = ?2)
                 ORDER BY opened_at",
            )
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(params![channel, workflow], row_to_wait)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Lists open waits whose timeout deadline is `<= now_iso`.
    pub fn waits_past_deadline(&self, now_iso: &str) -> Result<Vec<WaitRow>, WorkflowError> {
        self.query_wait_list(
            "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                    watch_json, deadline, opened_at, woke_json
             FROM workflow_waits
             WHERE deadline IS NOT NULL AND deadline <= ?1 AND signalled_at IS NULL",
            now_iso,
        )
    }

    /// Lists claimed waits whose wake has gone unanswered for at least 30s on open runs.
    pub fn waits_to_wake(&self, before_iso: &str) -> Result<Vec<WaitRow>, WorkflowError> {
        self.query_wait_list(
            "SELECT w.run_id, w.name, w.workflow, w.token, w.key, w.schema_json,
                    w.request_json, w.watch_json, w.deadline, w.opened_at, w.woke_json
             FROM workflow_waits w
             WHERE w.woke_json IS NOT NULL AND w.signalled_at <= ?1
               AND w.run_id IN (SELECT run_id FROM workflow_runs WHERE ended_at IS NULL)",
            before_iso,
        )
    }

    /// Lists claimed waits still open on `run_id` (sent when resuming a paused run).
    pub fn waits_claimed(&self, run_id: &str) -> Result<Vec<WaitRow>, WorkflowError> {
        self.query_wait_list(
            "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                    watch_json, deadline, opened_at, woke_json
             FROM workflow_waits WHERE run_id = ?1 AND woke_json IS NOT NULL",
            run_id,
        )
    }

    /// Lists open pull-request watch waits in opening order.
    pub fn watches(&self) -> Result<Vec<WaitRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard
            .prepare(
                "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                        watch_json, deadline, opened_at, woke_json
                 FROM workflow_waits
                 WHERE watch_json IS NOT NULL AND signalled_at IS NULL
                 ORDER BY opened_at",
            )
            .map_err(storage_err)?;
        let rows = stmt.query_map([], row_to_wait).map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }

    /// Lists open human questions (`ask`) in opening order.
    pub fn questions(&self) -> Result<Vec<WorkflowQuestion>, WorkflowError> {
        let waits = {
            let guard = self.lock_conn()?;
            let mut stmt = guard
                .prepare(
                    "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                            watch_json, deadline, opened_at, woke_json
                     FROM workflow_waits
                     WHERE request_json IS NOT NULL AND signalled_at IS NULL
                     ORDER BY opened_at",
                )
                .map_err(storage_err)?;
            let rows = stmt.query_map([], row_to_wait).map_err(storage_err)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(storage_err)?);
            }
            list
        };
        let mut out = Vec::with_capacity(waits.len());
        for wait in waits {
            let request: Value = wait
                .request_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or(Value::Null);
            let thread = self.latest_thread(&wait.run_id)?;
            out.push(WorkflowQuestion {
                run_id: wait.run_id,
                name: wait.name,
                request,
                asked_at: wait.opened_at,
                thread,
            });
        }
        Ok(out)
    }

    pub(crate) fn emit_question_resolved(&self, run_id: &str, name: &str, woke: &Value, at: &str) {
        let Ok(Some(wait)) = self.wait_row(run_id, name) else {
            return;
        };
        if wait.request_json.is_none() {
            return;
        }
        let expired = woke
            .get("expired")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.tell(
            run_id,
            &format!("{run_id}:{name}:{}", wait.opened_at),
            at,
            "workflow.input_resolved",
            |ev| {
                ev.step = Some(name.to_owned());
                ev.resolution = Some(if expired { "expired" } else { "answered" }.to_owned());
            },
        );
    }

    fn query_wait_list(&self, sql: &str, arg: &str) -> Result<Vec<WaitRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        let mut stmt = guard.prepare(sql).map_err(storage_err)?;
        let rows = stmt
            .query_map(params![arg], row_to_wait)
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(storage_err)?);
        }
        Ok(out)
    }
}

pub(crate) fn extract_question_title(wait_name: &str, request_json: &str) -> String {
    serde_json::from_str::<Value>(request_json)
        .ok()
        .and_then(|v| v.get("title")?.as_str().map(str::to_owned))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| wait_name.to_owned())
}

pub(crate) fn row_to_wait(row: &rusqlite::Row<'_>) -> rusqlite::Result<WaitRow> {
    Ok(WaitRow {
        run_id: row.get(0)?,
        name: row.get(1)?,
        workflow: row.get(2)?,
        token: row.get(3)?,
        key: row.get(4)?,
        schema_json: row.get(5)?,
        request_json: row.get(6)?,
        watch_json: row.get(7)?,
        deadline: row.get(8)?,
        opened_at: row.get(9)?,
        woke_json: row.get(10)?,
    })
}
