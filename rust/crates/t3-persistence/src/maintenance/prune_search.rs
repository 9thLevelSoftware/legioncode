//! Legacy imported history pruning, FTS5 message search rebuild, and transcript cleanup helpers.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use rusqlite::Connection;

use super::MaintenanceError;
use super::prune::table_exists;
use crate::sqlite::schema::{
    folded_message_text_expr, indexed_message_condition, messages_has_search_text_column,
};

pub(crate) fn prune_imported_legacy_history(
    conn: &mut Connection,
    has_events: bool,
    has_receipts: bool,
    through_sequence: i64,
    through_receipt_row_id: i64,
) -> rusqlite::Result<(u64, u64)> {
    let mut deleted_events = 0_u64;
    if has_events && through_sequence > 0 {
        let count = conn.execute(
            "DELETE FROM orchestration_events
             WHERE application_event_version = 1
               AND sequence <= ?1
               AND aggregate_kind = 'thread'
               AND EXISTS (
                 SELECT 1 FROM orchestration_v2_legacy_imports AS legacy_import
                 WHERE legacy_import.thread_id = orchestration_events.stream_id
                   AND legacy_import.transcript_imported_at IS NOT NULL
               )",
            [through_sequence],
        )?;
        deleted_events = u64::try_from(count).unwrap_or(0);
    }

    let mut deleted_receipts = 0_u64;
    if has_receipts && through_receipt_row_id > 0 {
        let count = conn.execute(
            "DELETE FROM orchestration_command_receipts
             WHERE rowid <= ?1
               AND command_type = 'legacy'
               AND aggregate_kind = 'thread'
               AND EXISTS (
                 SELECT 1 FROM orchestration_v2_legacy_imports AS legacy_import
                 WHERE legacy_import.thread_id = orchestration_command_receipts.aggregate_id
                   AND legacy_import.transcript_imported_at IS NOT NULL
               )",
            [through_receipt_row_id],
        )?;
        deleted_receipts = u64::try_from(count).unwrap_or(0);
    }

    Ok((deleted_events, deleted_receipts))
}

pub(crate) fn rebuild_message_search(conn: &mut Connection) -> rusqlite::Result<()> {
    if !table_exists(conn, "orchestration_v2_message_search")?
        || !table_exists(conn, "orchestration_v2_projection_messages")?
    {
        return Ok(());
    }

    let has_search_text = messages_has_search_text_column(conn)?;
    let text_expr = folded_message_text_expr("messages", has_search_text);
    let filter_sql = indexed_message_condition("messages");

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO orchestration_v2_message_search (orchestration_v2_message_search) VALUES ('delete-all')",
        [],
    )?;
    let refill_sql = format!(
        "INSERT INTO orchestration_v2_message_search (rowid, text)
         SELECT messages.rowid, {text_expr}
         FROM orchestration_v2_projection_messages AS messages
         WHERE {filter_sql}
           AND json_extract(messages.payload_json, '$.text') IS NOT NULL"
    );
    tx.execute(&refill_sql, [])?;
    tx.commit()
}

/// Deletes an imported transcript artifact on disk and cleans up its legacy import row,
/// surfacing the specific filesystem or SQLite reason when cleanup fails (`1458a42419`).
pub fn cleanup_imported_transcript(
    conn: &Connection,
    thread_id: &str,
    transcript_path: &Path,
) -> Result<bool, MaintenanceError> {
    let mut removed_file = false;
    match fs::remove_file(transcript_path) {
        Ok(()) => removed_file = true,
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(MaintenanceError::TranscriptCleanup {
                path: transcript_path.to_path_buf(),
                reason: format!("filesystem remove failed: {err}"),
            });
        }
    }

    if table_exists(conn, "orchestration_v2_legacy_imports").map_err(|err| {
        MaintenanceError::TranscriptCleanup {
            path: transcript_path.to_path_buf(),
            reason: format!("SQLite schema lookup failed: {err}"),
        }
    })? {
        conn.execute(
            "DELETE FROM orchestration_v2_legacy_imports WHERE thread_id = ?1",
            [thread_id],
        )
        .map_err(|err| MaintenanceError::TranscriptCleanup {
            path: transcript_path.to_path_buf(),
            reason: format!("SQLite cleanup failed: {err}"),
        })?;
    }

    Ok(removed_file)
}
