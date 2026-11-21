//! Read/projection loading & UTC timestamp helpers for `RuntimeOutboxExecutor`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use t3_contracts::ids::{CheckpointId, CheckpointScopeId, MessageId, RunId, ThreadId};
use t3_contracts::orchestration::{AppThread, Checkpoint, DomainEventPayload, Run, RunAttempt};
use t3_orchestrator::sink::event_codec::load_thread_events;
use t3_orchestrator::sink::projection_apply::load_decider_state;
use t3_persistence::project_store::find_project_by_id_or_workspace;

use crate::error::ServerRuntimeError;

/// Snapshot of a thread's state extracted from `Orchestrator` events and projections.
#[derive(Debug, Clone)]
pub(crate) struct LoadedThreadContext {
    pub thread: AppThread,
    pub runs: Vec<Run>,
    pub attempts: Vec<RunAttempt>,
    pub messages_by_id: BTreeMap<String, String>,
    pub checkpoints_by_id: BTreeMap<String, Checkpoint>,
    pub scope_ids: Vec<CheckpointScopeId>,
    pub latest_checkpoint_by_scope: BTreeMap<String, Checkpoint>,
    pub next_item_ordinal: u64,
}

impl LoadedThreadContext {
    pub fn load(conn: &Connection, thread_id: &ThreadId) -> Result<Self, ServerRuntimeError> {
        let decider = load_decider_state(conn, thread_id)?;
        let thread = decider.thread.ok_or_else(|| {
            ServerRuntimeError::NotFound(format!("thread `{thread_id}` not found"))
        })?;
        let persisted = load_thread_events(conn, thread_id, 0)?;

        let mut attempts_by_id: BTreeMap<String, RunAttempt> = BTreeMap::new();
        let mut messages_by_id: BTreeMap<String, String> = BTreeMap::new();
        let mut checkpoints_by_id: BTreeMap<String, Checkpoint> = BTreeMap::new();
        let mut scope_ids: Vec<CheckpointScopeId> = Vec::new();
        let mut latest_checkpoint_by_scope: BTreeMap<String, Checkpoint> = BTreeMap::new();
        let mut max_item_ordinal = 0_u64;

        for row in persisted {
            match row.event.payload {
                DomainEventPayload::RunAttemptUpdated { attempt, .. } => {
                    attempts_by_id.insert(attempt.id.as_str().to_owned(), attempt);
                }
                DomainEventPayload::MessageUpdated(msg) => {
                    messages_by_id.insert(msg.id.as_str().to_owned(), msg.text);
                }
                DomainEventPayload::TurnItemUpdated(item) => {
                    if item.base.ordinal > max_item_ordinal {
                        max_item_ordinal = item.base.ordinal;
                    }
                }
                DomainEventPayload::CheckpointScopeCreated(scope) => {
                    if !scope_ids.contains(&scope.id) {
                        scope_ids.push(scope.id);
                    }
                }
                DomainEventPayload::CheckpointCaptured(cp) => {
                    let scope_key = cp.scope_id.as_str().to_owned();
                    let replace = latest_checkpoint_by_scope
                        .get(&scope_key)
                        .is_none_or(|prev| cp.ordinal_within_scope >= prev.ordinal_within_scope);
                    if replace {
                        latest_checkpoint_by_scope.insert(scope_key, cp.clone());
                    }
                    checkpoints_by_id.insert(cp.id.as_str().to_owned(), cp);
                }
                _ => {}
            }
        }

        Ok(Self {
            thread,
            runs: decider.runs,
            attempts: attempts_by_id.into_values().collect(),
            messages_by_id,
            checkpoints_by_id,
            scope_ids,
            latest_checkpoint_by_scope,
            next_item_ordinal: max_item_ordinal.saturating_add(1),
        })
    }

    pub fn find_run(&self, run_id: &RunId) -> Result<&Run, ServerRuntimeError> {
        self.runs
            .iter()
            .find(|r| &r.id == run_id)
            .ok_or_else(|| ServerRuntimeError::NotFound(format!("run `{run_id}` not found")))
    }

    pub fn find_attempt(&self, run: &Run) -> Option<RunAttempt> {
        if let Some(ref aid) = run.active_attempt_id
            && let Some(found) = self.attempts.iter().find(|a| &a.id == aid)
        {
            return Some(found.clone());
        }
        self.attempts.iter().rfind(|a| a.run_id == run.id).cloned()
    }

    pub fn message_text(&self, message_id: &MessageId) -> String {
        self.messages_by_id
            .get(message_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn checkpoint_by_id(&self, checkpoint_id: &CheckpointId) -> Option<&Checkpoint> {
        self.checkpoints_by_id.get(checkpoint_id.as_str())
    }
}

/// Resolve the git/working directory for a thread from worktree, overrides, or `projection_projects`.
pub(crate) fn resolve_workspace_root(
    conn: &Connection,
    thread: &AppThread,
    overrides: &BTreeMap<String, PathBuf>,
) -> Result<PathBuf, ServerRuntimeError> {
    if let Some(ref wt) = thread.worktree_path {
        let trimmed = wt.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Some(path) = overrides.get(thread.project_id.as_str()) {
        return Ok(path.clone());
    }
    if let Some(record) = find_project_by_id_or_workspace(conn, thread.project_id.as_str())? {
        return Ok(PathBuf::from(record.workspace_root));
    }
    Err(ServerRuntimeError::NotFound(format!(
        "workspace root for project `{}` not found",
        thread.project_id
    )))
}

/// Build a concise single-line thread title from user message text.
pub(crate) fn derive_thread_title(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let truncated: String = collapsed.chars().take(60).collect();
    if collapsed.chars().count() > 60 {
        Some(format!("{truncated}..."))
    } else {
        Some(truncated)
    }
}

/// Format current UTC wall-clock time as ISO-8601 (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
pub fn utc_now_iso() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format_unix_ms_iso(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// Format signed Unix epoch milliseconds as ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
pub fn format_unix_ms_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let sub_ms = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem_secs = secs.rem_euclid(86_400);
    let hour = rem_secs / 3600;
    let minute = (rem_secs % 3600) / 60;
    let second = rem_secs % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub_ms:03}Z")
}

/// Howard Hinnant's civil-from-days conversion from Unix day count (`1970-01-01` = 0).
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(m <= 2);
    (year, m, d)
}
