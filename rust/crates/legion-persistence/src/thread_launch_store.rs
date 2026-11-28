//! Crash-safe two-phase persistence for stable thread launch identity over
//! `orchestration_v2_thread_launch_workflows` (`command_id` = `stable_launch_key`).

use rusqlite::{Connection, OptionalExtension as _, Row, params};
use thiserror::Error;

/// Errors returned by [`thread_launch_store`](self) persistence operations.
#[derive(Debug, Error)]
pub enum ThreadLaunchStoreError {
    /// Underlying SQLite error.
    #[error("thread launch store SQLite error during '{operation}': {source}")]
    Sqlite {
        /// Operation label.
        operation: &'static str,
        /// Underlying `rusqlite::Error`.
        #[source]
        source: rusqlite::Error,
    },
    /// Empty `stable_launch_key`, `thread_id`, or `project_id`.
    #[error("invalid thread launch store input: {0}")]
    InvalidInput(String),
}

/// Persisted record in `orchestration_v2_thread_launch_workflows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLaunchRecord {
    /// Stable launch identity stored in `command_id` (`PRIMARY KEY`).
    pub stable_launch_key: String,
    /// Bound thread ID.
    pub thread_id: String,
    /// Bound project ID.
    pub project_id: String,
    /// Current status (`preparing`, `committed`, `failed`).
    pub status: String,
    /// Thread title.
    pub title: String,
    /// Bound worktree path (if `Worktree` or `ExistingWorktree`).
    pub worktree_path: Option<String>,
    /// Bound git branch name (if any).
    pub branch: Option<String>,
    /// Whether workspace setup (`git worktree add` / validation) completed.
    pub setup_committed: bool,
    /// Whether `ThreadCreate` was committed via `Orchestrator`.
    pub thread_committed: bool,
    /// Whether an initial message was committed atomically.
    pub message_committed: bool,
    /// Last launch error message, if any.
    pub last_error: Option<String>,
    /// Creation ISO-8601 timestamp.
    pub created_at: String,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
}

/// Parameters for atomically claiming a `stable_launch_key`.
#[derive(Debug, Clone)]
pub struct ClaimThreadLaunchInput<'a> {
    /// Deterministic launch key stored in `command_id`.
    pub stable_launch_key: &'a str,
    /// Candidate thread ID if this claim inserts a new row.
    pub thread_id: &'a str,
    /// Owning canonical project ID.
    pub project_id: &'a str,
    /// Thread title.
    pub title: &'a str,
    /// ISO-8601 UTC timestamp.
    pub now_iso: &'a str,
}

/// Outcome of [`claim_thread_launch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimThreadLaunchOutcome {
    /// Newly claimed row ready for workspace preparation and thread creation.
    Claimed(ThreadLaunchRecord),
    /// Pre-existing row found for the same `stable_launch_key`.
    Existing(ThreadLaunchRecord),
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<ThreadLaunchRecord> {
    Ok(ThreadLaunchRecord {
        stable_launch_key: row.get(0)?,
        thread_id: row.get(1)?,
        project_id: row.get(2)?,
        status: row.get(3)?,
        title: row.get(4)?,
        worktree_path: row.get(5)?,
        branch: row.get(6)?,
        setup_committed: row.get::<_, i64>(7)? != 0,
        thread_committed: row.get::<_, i64>(8)? != 0,
        message_committed: row.get::<_, i64>(9)? != 0,
        last_error: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// Reads an existing launch record by `stable_launch_key`, if present.
pub fn get_thread_launch(
    conn: &Connection,
    stable_launch_key: &str,
) -> Result<Option<ThreadLaunchRecord>, ThreadLaunchStoreError> {
    let key = stable_launch_key.trim();
    if key.is_empty() {
        return Err(ThreadLaunchStoreError::InvalidInput(
            "stable_launch_key cannot be empty".to_owned(),
        ));
    }
    conn.query_row(
        "SELECT command_id, thread_id, project_id, status, title, worktree_path, branch,
                setup_committed, thread_committed, message_committed, last_error,
                created_at, updated_at
         FROM orchestration_v2_thread_launch_workflows
         WHERE command_id = ?1",
        params![key],
        map_row,
    )
    .optional()
    .map_err(|source| ThreadLaunchStoreError::Sqlite {
        operation: "get_thread_launch",
        source,
    })
}

/// Atomically claims `stable_launch_key` or returns the existing [`ThreadLaunchRecord`].
pub fn claim_thread_launch(
    conn: &Connection,
    input: &ClaimThreadLaunchInput<'_>,
) -> Result<ClaimThreadLaunchOutcome, ThreadLaunchStoreError> {
    let key = input.stable_launch_key.trim();
    let tid = input.thread_id.trim();
    let pid = input.project_id.trim();
    if key.is_empty() || tid.is_empty() || pid.is_empty() {
        return Err(ThreadLaunchStoreError::InvalidInput(
            "stable_launch_key, thread_id, and project_id must be non-empty".to_owned(),
        ));
    }
    let inserted = conn
        .execute(
            "INSERT INTO orchestration_v2_thread_launch_workflows (
               command_id, thread_id, project_id, status, title,
               worktree_path, branch, setup_committed, thread_committed,
               message_committed, last_error, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'preparing', ?4, NULL, NULL, 0, 0, 0, NULL, ?5, ?5)
             ON CONFLICT(command_id) DO NOTHING",
            params![key, tid, pid, input.title, input.now_iso],
        )
        .map_err(|source| ThreadLaunchStoreError::Sqlite {
            operation: "claim_thread_launch_insert",
            source,
        })?;
    let record = get_thread_launch(conn, key)?.ok_or_else(|| {
        ThreadLaunchStoreError::InvalidInput(format!("claimed launch row missing for key `{key}`"))
    })?;
    if inserted > 0 {
        Ok(ClaimThreadLaunchOutcome::Claimed(record))
    } else {
        Ok(ClaimThreadLaunchOutcome::Existing(record))
    }
}

/// Records workspace setup completion (`setup_committed = 1`) before `ThreadCreate` dispatch.
pub fn mark_thread_launch_setup_committed(
    conn: &Connection,
    stable_launch_key: &str,
    worktree_path: Option<&str>,
    branch: Option<&str>,
    updated_at: &str,
) -> Result<(), ThreadLaunchStoreError> {
    conn.execute(
        "UPDATE orchestration_v2_thread_launch_workflows
         SET worktree_path = ?2,
             branch = ?3,
             setup_committed = 1,
             last_error = NULL,
             updated_at = ?4
         WHERE command_id = ?1",
        params![stable_launch_key.trim(), worktree_path, branch, updated_at],
    )
    .map_err(|source| ThreadLaunchStoreError::Sqlite {
        operation: "mark_thread_launch_setup_committed",
        source,
    })?;
    Ok(())
}

/// Records `ThreadCreate` commit (`thread_committed = 1`, `status = 'committed'`).
pub fn mark_thread_launch_committed(
    conn: &Connection,
    stable_launch_key: &str,
    message_committed: bool,
    updated_at: &str,
) -> Result<(), ThreadLaunchStoreError> {
    let msg_flag = i64::from(message_committed);
    conn.execute(
        "UPDATE orchestration_v2_thread_launch_workflows
         SET status = 'committed',
             thread_committed = 1,
             message_committed = MAX(message_committed, ?2),
             last_error = NULL,
             updated_at = ?3
         WHERE command_id = ?1",
        params![stable_launch_key.trim(), msg_flag, updated_at],
    )
    .map_err(|source| ThreadLaunchStoreError::Sqlite {
        operation: "mark_thread_launch_committed",
        source,
    })?;
    Ok(())
}

/// Records a launch error (`status = 'failed'`, `last_error`) unless already committed.
pub fn mark_thread_launch_failed(
    conn: &Connection,
    stable_launch_key: &str,
    error: &str,
    updated_at: &str,
) -> Result<(), ThreadLaunchStoreError> {
    conn.execute(
        "UPDATE orchestration_v2_thread_launch_workflows
         SET status = 'failed',
             last_error = ?2,
             updated_at = ?3
         WHERE command_id = ?1
           AND thread_committed = 0",
        params![stable_launch_key.trim(), error, updated_at],
    )
    .map_err(|source| ThreadLaunchStoreError::Sqlite {
        operation: "mark_thread_launch_failed",
        source,
    })?;
    Ok(())
}
