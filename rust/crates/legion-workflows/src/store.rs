//! SQLite workflow store: core run lifecycle mutations and change broadcast.

use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::schema::ensure_workflow_and_feedback_schema;
use crate::time::now_iso_utc;
use crate::traits::{NoopMilestoneSink, WorkflowMilestoneEvent, WorkflowMilestoneSink};
use crate::types::{RunParent, WorkflowError, WorkflowRunStatus};

/// Raw persisted row from `workflow_runs`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    /// Run ID.
    pub run_id: String,
    /// Workflow name.
    pub workflow: String,
    /// Idempotency key unique per workflow.
    pub idempotency_key: String,
    /// Workflow version.
    pub version: i64,
    /// `name@version` engine tag.
    pub engine_tag: String,
    /// Display title.
    pub title: String,
    /// Current lifecycle status.
    pub status: WorkflowRunStatus,
    /// Pause, wait, or terminal detail.
    pub detail: Option<String>,
    /// Terminal JSON text.
    pub result_json: Option<String>,
    /// Input JSON text retained for `onEnd` cleanup.
    pub input_json: Option<String>,
    /// Invoking parent run ID, if any.
    pub parent_run_id: Option<String>,
    /// Wait step name on parent run, if any.
    pub parent_wait: Option<String>,
    /// Consecutive step-failure pauses.
    pub pauses: i64,
    /// 1 when held for manual person resume (e.g. agent failure).
    pub held: i64,
    /// ISO-8601 start timestamp.
    pub started_at: String,
    /// ISO-8601 update timestamp.
    pub updated_at: String,
    /// ISO-8601 end timestamp.
    pub ended_at: Option<String>,
}

/// Input to insert a new run row idempotently.
#[derive(Debug, Clone)]
pub struct NewRunRecord {
    /// Run ID.
    pub run_id: String,
    /// Workflow name.
    pub workflow: String,
    /// Workflow version.
    pub version: i64,
    /// `name@version` tag.
    pub engine_tag: String,
    /// Idempotency key unique within `workflow`.
    pub idempotency_key: String,
    /// Input payload JSON value.
    pub input: Value,
    /// Optional invoking parent run.
    pub parent: Option<RunParent>,
}

/// Row returned by `runs_to_end` for cancelled runs needing `onEnd` cleanup.
#[derive(Debug, Clone)]
pub struct EndingRunRow {
    /// Base run row.
    pub run: RunRow,
    /// Prior `onEnd` attempt count.
    pub end_attempts: i64,
    /// Timestamp when the last `onEnd` attempt finished.
    pub end_ended_at: Option<String>,
}

/// SQLite-backed store over all workflow metadata tables and change broadcasts.
#[derive(Clone)]
pub struct WorkflowStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) changes: broadcast::Sender<String>,
    pub(crate) milestones: Arc<dyn WorkflowMilestoneSink>,
}

impl std::fmt::Debug for WorkflowStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowStore").finish_non_exhaustive()
    }
}

impl WorkflowStore {
    /// Creates a store around `conn`, ensuring workflow/feedback DDL exists.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, WorkflowError> {
        Self::with_milestones(conn, Arc::new(NoopMilestoneSink))
    }

    /// Creates a store with a custom [`WorkflowMilestoneSink`].
    pub fn with_milestones(
        conn: Arc<Mutex<Connection>>,
        milestones: Arc<dyn WorkflowMilestoneSink>,
    ) -> Result<Self, WorkflowError> {
        {
            let guard = conn.lock().map_err(|err| WorkflowError::Storage {
                message: format!("store mutex poisoned: {err}"),
            })?;
            ensure_workflow_and_feedback_schema(&guard).map_err(storage_err)?;
        }
        let (changes, _) = broadcast::channel(512);
        Ok(Self {
            conn,
            changes,
            milestones,
        })
    }

    /// Subscribes to per-`run_id` change notifications.
    pub fn subscribe_changes(&self) -> broadcast::Receiver<String> {
        self.changes.subscribe()
    }

    /// Returns a handle to the underlying SQLite connection.
    pub fn connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Inserts a run row if `(workflow, idempotency_key)` and `run_id` are unused.
    pub fn run_started(&self, run: &NewRunRecord) -> Result<bool, WorkflowError> {
        let at = now_iso_utc();
        let title = format!("{} {}", run.workflow, run.idempotency_key);
        let input_json = serde_json::to_string(&run.input).map_err(storage_err)?;
        let parent_run_id = run.parent.as_ref().map(|p| p.run_id.as_str());
        let parent_wait = run.parent.as_ref().map(|p| p.wait.as_str());
        let inserted = {
            let guard = self.lock_conn()?;
            guard
                .query_row(
                    "INSERT INTO workflow_runs (
                       run_id, workflow, version, engine_tag, idempotency_key, title, status,
                       input_json, parent_run_id, parent_wait, started_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, ?9, ?10, ?10)
                     ON CONFLICT DO NOTHING
                     RETURNING run_id",
                    params![
                        run.run_id,
                        run.workflow,
                        run.version,
                        run.engine_tag,
                        run.idempotency_key,
                        title,
                        input_json,
                        parent_run_id,
                        parent_wait,
                        at,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_err)?
        };
        if inserted.is_none() {
            return Ok(false);
        }
        self.notify_change(&run.run_id);
        self.tell(
            &run.run_id,
            &run.run_id,
            &at,
            "workflow.run_started",
            |_| {},
        );
        Ok(true)
    }

    /// Sets `running` or `waiting` status when the run is neither ended nor paused.
    pub fn run_status(
        &self,
        run_id: &str,
        status: WorkflowRunStatus,
        detail: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "UPDATE workflow_runs
                     SET status = ?1, detail = ?2, updated_at = ?3
                     WHERE run_id = ?4 AND ended_at IS NULL AND status != 'paused'",
                    params![status.as_str(), detail, at, run_id],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Moves a non-paused, non-ended run back to `running` or the next open async step.
    pub fn run_resumed(&self, run_id: &str) -> Result<(), WorkflowError> {
        let at = now_iso_utc();
        {
            let guard = self.lock_conn()?;
            guard
                .execute(
                    "WITH out AS (
                       SELECT CASE WHEN kind IN ('agent', 'sleep') THEN kind || ' ' || name ELSE name END
                         AS detail
                       FROM workflow_steps
                       WHERE run_id = ?1 AND status = 'running'
                         AND kind IN ('wait', 'ask', 'agent', 'sleep')
                       ORDER BY started_at, rowid LIMIT 1
                     )
                     UPDATE workflow_runs
                     SET status = CASE WHEN EXISTS (SELECT 1 FROM out) THEN 'waiting' ELSE 'running' END,
                         detail = (SELECT detail FROM out),
                         updated_at = ?2
                     WHERE run_id = ?1 AND ended_at IS NULL AND status != 'paused'",
                    params![run_id, at],
                )
                .map_err(storage_err)?;
        }
        self.notify_change(run_id);
        Ok(())
    }

    /// Marks an open non-paused run as `paused`, incrementing `pauses` and updating `held` if given.
    pub fn run_paused(
        &self,
        run_id: &str,
        detail: &str,
        held: Option<bool>,
    ) -> Result<bool, WorkflowError> {
        let at = now_iso_utc();
        let reason = truncate_chars(detail, 2_000);
        let paused = {
            let guard = self.lock_conn()?;
            match held {
                Some(h) => {
                    let held_int: i64 = i64::from(h);
                    guard
                        .query_row(
                            "UPDATE workflow_runs
                             SET status = 'paused', detail = ?1, pauses = pauses + 1,
                                 held = ?2, updated_at = ?3
                             WHERE run_id = ?4 AND ended_at IS NULL AND status != 'paused'
                             RETURNING run_id",
                            params![reason, held_int, at, run_id],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(storage_err)?
                }
                None => guard
                    .query_row(
                        "UPDATE workflow_runs
                         SET status = 'paused', detail = ?1, pauses = pauses + 1,
                             updated_at = ?2
                         WHERE run_id = ?3 AND ended_at IS NULL AND status != 'paused'
                         RETURNING run_id",
                        params![reason, at, run_id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(storage_err)?,
            }
        };
        self.notify_change(run_id);
        if paused.is_some() {
            self.tell(
                run_id,
                &format!("{run_id}:{at}"),
                &at,
                "workflow.run_paused",
                |ev| {
                    ev.reason = Some(reason.clone());
                },
            );
            return Ok(true);
        }
        Ok(false)
    }

    /// Transitions a paused run back to `running`, optionally guarded by `(pauses, updated_at)`.
    pub(crate) fn notify_change(&self, run_id: &str) {
        let _ = self.changes.send(run_id.to_owned());
    }

    pub(crate) fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, WorkflowError> {
        self.conn.lock().map_err(|err| WorkflowError::Storage {
            message: format!("store mutex poisoned: {err}"),
        })
    }

    pub(crate) fn tell(
        &self,
        run_id: &str,
        key: &str,
        at: &str,
        event_type: &str,
        customize: impl FnOnce(&mut WorkflowMilestoneEvent),
    ) {
        let Ok(guard) = self.lock_conn() else {
            return;
        };
        let row: Option<(String, String, Option<String>)> = guard
            .query_row(
                "SELECT workflow, title, parent_run_id FROM workflow_runs WHERE run_id = ?1",
                params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ok()
            .flatten();
        drop(guard);
        let Some((workflow, run_title, parent_run_id)) = row else {
            return;
        };
        let mut event = WorkflowMilestoneEvent {
            id: format!("{event_type}:{key}"),
            event_type: event_type.to_owned(),
            occurred_at: at.to_owned(),
            is_subagent_child: parent_run_id.is_some(),
            workflow_run_id: run_id.to_owned(),
            workflow,
            run_title,
            parent_workflow_run_id: parent_run_id,
            step: None,
            question: None,
            resolution: None,
            reason: None,
            outcome: None,
            detail: None,
        };
        customize(&mut event);
        self.milestones.publish(event);
    }
}

pub(crate) use crate::store_run_ext::{row_to_run, storage_err, truncate_chars};
