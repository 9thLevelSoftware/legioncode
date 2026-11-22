//! Shell sidebar snapshots (`ShellSnapshot`) and 24-hour event-store retention maintenance.

pub mod prune;
pub mod shell;
pub mod shell_focus;
pub mod shell_types;

use thiserror::Error;

pub use prune::{
    EVENT_LOG_PRUNE_MARK_PROJECTION, EVENT_LOG_PRUNED_THROUGH_PROJECTION, PruneEventStoreReport,
    RETAINED_EVENT_HISTORY_MS, prune_event_store, prune_event_store_now,
};
pub use shell::{
    OlderSettledProjectCount, OlderSettledSummary, SHELL_NEWEST_SETTLED_KEPT, ShellSnapshot,
    ShellSnapshotOptions, ShellThread, ShellThreadFamily, ShellThreadStatus,
    list_older_settled_threads, query_shell_snapshot,
};

/// Errors returned by projection queries and event-log retention maintenance.
#[derive(Debug, Error)]
pub enum ProjectionError {
    /// Underlying `SQLite` query or DDL failure.
    #[error("projection SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Stored projection JSON or ISO-8601 timestamp could not be decoded.
    #[error("projection codec error: {0}")]
    Codec(String),
}
