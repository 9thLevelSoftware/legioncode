//! Durable wait rows (`workflow_waits`) and claim/deadline queries on [`WorkflowStore`].

use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::store::{WorkflowStore, storage_err};
use crate::time::{format_millis_iso, now_epoch_ms, now_iso_utc};
use crate::types::{SignalAddress, WorkflowError};
use crate::waits_queries::{extract_question_title, row_to_wait};

/// Default signal mail retention (`7 days`).
pub const DEFAULT_SIGNAL_TTL_MS: i64 = 7 * 86_400_000;
/// Claims older than 30 seconds whose runs are still open have their wake re-issued.
pub const WAKE_REDONE_AFTER_MS: i64 = 30_000;

/// Raw row from `workflow_waits`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitRow {
    /// Owning run ID.
    pub run_id: String,
    /// Step/wait name.
    pub name: String,
    /// Workflow name.
    pub workflow: String,
    /// Unique wait token for this opening.
    pub token: String,
    /// Optional channel key.
    pub key: Option<String>,
    /// JSON Schema string validating incoming signals.
    pub schema_json: String,
    /// Question JSON (`ask`), or `None`.
    pub request_json: Option<String>,
    /// Pull-request watch JSON, or `None`.
    pub watch_json: Option<String>,
    /// Optional timeout ISO-8601 deadline.
    pub deadline: Option<String>,
    /// Opening ISO-8601 timestamp.
    pub opened_at: String,
    /// Claimed wake payload JSON (`{ expired, value }`), if signalled.
    pub woke_json: Option<String>,
}

/// Input to open a durable wait row (`wait_opened`).
#[derive(Debug, Clone)]
pub struct NewWaitInput {
    /// Owning run ID.
    pub run_id: String,
    /// Step/wait name.
    pub name: String,
    /// Workflow name.
    pub workflow: String,
    /// Optional channel key.
    pub key: Option<String>,
    /// Unique wake token.
    pub token: String,
    /// JSON Schema string.
    pub schema_json: String,
    /// Question JSON for `ask`.
    pub request_json: Option<String>,
    /// Pull-request watch JSON.
    pub watch_json: Option<String>,
    /// Optional ISO-8601 timeout deadline.
    pub deadline: Option<String>,
}

impl WorkflowStore {
    /// Opens a wait row if not already present and emits `workflow.needs_input` for `ask`.
    pub fn wait_opened(&self, wait: &NewWaitInput) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        let opened: Option<String> = {
            let guard = self.lock_conn()?;
            guard
                .query_row(
                    "INSERT INTO workflow_waits (
                       run_id, name, workflow, token, key, schema_json, request_json,
                       watch_json, deadline, opened_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT (run_id, name) DO NOTHING
                     RETURNING name",
                    params![
                        wait.run_id,
                        wait.name,
                        wait.workflow,
                        wait.token,
                        wait.key,
                        wait.schema_json,
                        wait.request_json,
                        wait.watch_json,
                        wait.deadline,
                        at,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?
        };
        self.notify_change(&wait.run_id);
        if opened.is_some()
            && let Some(req_raw) = wait.request_json.as_deref()
        {
            let question = extract_question_title(&wait.name, req_raw);
            self.tell(
                &wait.run_id,
                &format!("{}:{}:{at}", wait.run_id, wait.name),
                &at,
                "workflow.needs_input",
                |ev| {
                    ev.step = Some(wait.name.clone());
                    ev.question = Some(question.clone());
                },
            );
        }
        Ok(())
    }

    /// Removes a completed wait row.
    pub fn wait_closed(&self, run_id: &str, name: &str) -> Result<(), WorkflowError> {
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "DELETE FROM workflow_waits WHERE run_id = ?1 AND name = ?2",
                    params![run_id, name],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Reopens a wait under a fresh `token` when a value passed JSON Schema but failed runtime codec.
    pub fn wait_reopened(
        &self,
        run_id: &str,
        name: &str,
        token: &str,
    ) -> Result<(), WorkflowError> {
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_waits
                     SET token = ?1, signalled_at = NULL, woke_json = NULL
                     WHERE run_id = ?2 AND name = ?3",
                    params![token, run_id, name],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Atomically claims an open wait for `woke`, recording `signal_id` dedup if given.
    pub fn wait_signalled(
        &self,
        run_id: &str,
        name: &str,
        woke: &Value,
        signal_id: Option<&str>,
    ) -> Result<bool, WorkflowError> {
        let at = now_iso_utc();
        let woke_json = serde_json::to_string(woke).map_err(storage_err)?;
        let keep_until = format_millis_iso(now_epoch_ms().saturating_add(DEFAULT_SIGNAL_TTL_MS));
        let taken = {
            let mut guard = self.lock_conn()?;
            let tx = guard.transaction().map_err(storage_err)?;
            let claimed: Option<String> = tx
                .query_row(
                    "UPDATE workflow_waits SET signalled_at = ?1, woke_json = ?2
                     WHERE run_id = ?3 AND name = ?4 AND signalled_at IS NULL
                     RETURNING name",
                    params![at, woke_json, run_id, name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_err)?;
            if claimed.is_some() {
                if let Some(sid) = signal_id {
                    let receipt: Option<String> = tx
                        .query_row(
                            "INSERT INTO workflow_signals (signal_id, kept_until)
                             VALUES (?1, MAX(?2, COALESCE(
                               (SELECT expires_at FROM workflow_mail WHERE signal_id = ?1), ''
                             )))
                             ON CONFLICT (signal_id) DO NOTHING
                             RETURNING signal_id",
                            params![sid, keep_until],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(storage_err)?;
                    if receipt.is_none() {
                        return Ok(false);
                    }
                }
                tx.commit().map_err(storage_err)?;
                true
            } else {
                false
            }
        };
        if taken {
            self.notify_change(run_id);
            self.emit_question_resolved(run_id, name, woke, &at);
        }
        Ok(taken)
    }

    /// Finds an open wait by direct `(run_id, wait)` or oldest open wait on `(workflow, channel)`.
    pub fn wait_at(&self, address: &SignalAddress) -> Result<Option<WaitRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        match address {
            SignalAddress::Run { run_id, wait } => guard
                .query_row(
                    "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                            watch_json, deadline, opened_at, woke_json
                     FROM workflow_waits
                     WHERE run_id = ?1 AND name = ?2 AND signalled_at IS NULL",
                    params![run_id, wait],
                    row_to_wait,
                )
                .optional()
                .map_err(storage_err),
            SignalAddress::Channel { workflow, channel } => guard
                .query_row(
                    "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                            watch_json, deadline, opened_at, woke_json
                     FROM workflow_waits
                     WHERE workflow = ?1 AND key = ?2 AND signalled_at IS NULL
                     ORDER BY opened_at LIMIT 1",
                    params![workflow, channel],
                    row_to_wait,
                )
                .optional()
                .map_err(storage_err),
        }
    }

    /// Reads one wait row regardless of whether it was already signalled.
    pub fn wait_row(&self, run_id: &str, name: &str) -> Result<Option<WaitRow>, WorkflowError> {
        let guard = self.lock_conn()?;
        guard
            .query_row(
                "SELECT run_id, name, workflow, token, key, schema_json, request_json,
                        watch_json, deadline, opened_at, woke_json
                 FROM workflow_waits WHERE run_id = ?1 AND name = ?2",
                params![run_id, name],
                row_to_wait,
            )
            .optional()
            .map_err(storage_err)
    }
}
