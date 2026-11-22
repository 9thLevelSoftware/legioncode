//! Shell sidebar snapshots, incremental wire projections, thread paging, and transcripts.

pub mod prune;
pub mod shell;
pub mod shell_focus;
pub mod shell_types;
pub mod wire_codec;
pub mod wire_projects;
pub mod wire_shell;
mod wire_shell_load;
pub mod wire_shell_rows;
mod wire_shell_trim;
pub mod wire_thread;
pub mod wire_thread_paging;
mod wire_thread_types;
mod wire_thread_visibility;
pub mod wire_transcript;
pub mod wire_types;

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
pub use wire_projects::query_wire_projects;
pub use wire_shell::{
    build_wire_shell_patch_from_events, list_wire_older_settled_threads,
    query_wire_shell_http_snapshot, query_wire_shell_http_snapshot_with_options,
    query_wire_shell_stream_reset,
};
pub use wire_thread::query_wire_thread_detail;
pub use wire_thread_paging::{query_wire_thread_bounded, query_wire_thread_history_page};
pub use wire_transcript::{query_wire_thread_transcript, render_wire_thread_transcript};
pub use wire_types::{
    WireOlderSettledCount, WireOlderSettledCursor, WireOlderSettledThreads,
    WireOlderSettledThreadsPage, WirePendingRuntimeRequestSummary, WireProject, WireProjectShell,
    WireProjectSnapshot, WireProjectedTurnItem, WireShellHttpSnapshot, WireShellOptions,
    WireShellSnapshot, WireShellStreamEvent, WireThreadBoundedSnapshot, WireThreadDetailSnapshot,
    WireThreadHistoryPage, WireThreadProjection, WireThreadShell, WireThreadTranscript,
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
    /// Target thread was not found in materialized projections.
    #[error("thread `{0}` not found")]
    ThreadNotFound(String),
    /// Client supplied a malformed or out-of-range pagination cursor.
    #[error("invalid thread history cursor")]
    InvalidCursor,
}
