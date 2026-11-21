//! Event-store pruning and FTS5 trigram search rebuild used during database compaction.

use std::collections::HashSet;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use crate::ownership::{format_iso8601_utc, parse_iso8601_utc_millis};
use crate::sqlite::schema::{
    folded_message_text_expr, indexed_message_condition, messages_has_search_text_column,
};

const EVENT_LOG_PRUNED_THROUGH: &str = "event-log-pruned-through";
const EVENT_LOG_PRUNE_MARK: &str = "event-log-prune-mark";
const ORCHESTRATION_V2_PROJECTION_SCHEMA_VERSION: i64 = 2;
const RETAINED_EVENT_HISTORY_MS: i64 = 86_400_000;
const PAGE_SIZE: i64 = 50;
const REFERENCE_SCAN_WINDOW: i64 = 1000;

pub(crate) fn prune_event_store(conn: &mut Connection) -> rusqlite::Result<(u64, u64)> {
    let has_events = table_exists(conn, "orchestration_events")?;
    let has_receipts = table_exists(conn, "orchestration_command_receipts")?;
    let has_metadata = table_exists(conn, "orchestration_v2_projection_metadata")?;
    let has_legacy_imports = table_exists(conn, "orchestration_v2_legacy_imports")?;

    let head: i64 = if has_events {
        conn.query_row(
            "SELECT COALESCE((SELECT MAX(sequence) FROM orchestration_events), 0)",
            [],
            |row| row.get(0),
        )?
    } else {
        0
    };
    let receipt_head: i64 = if has_receipts {
        conn.query_row(
            "SELECT COALESCE((SELECT MAX(rowid) FROM orchestration_command_receipts), 0)",
            [],
            |row| row.get(0),
        )?
    } else {
        0
    };

    let mut deleted_events = 0_u64;
    if has_events && has_metadata {
        deleted_events += prune_superseded_events(conn, head)?;
    }
    let mut deleted_receipts = 0_u64;
    if has_legacy_imports {
        let (legacy_events, legacy_receipts) =
            prune_imported_legacy_history(conn, has_events, has_receipts, head, receipt_head)?;
        deleted_events += legacy_events;
        deleted_receipts += legacy_receipts;
    }

    Ok((deleted_events, deleted_receipts))
}

fn prune_superseded_events(conn: &mut Connection, head: i64) -> rusqlite::Result<u64> {
    let now = SystemTime::now();
    let now_iso = format_iso8601_utc(now);
    let now_ms = parse_iso8601_utc_millis(&now_iso).unwrap_or(0);

    let projected = read_metadata_seq(conn, "thread-projections")?.map_or(0, |r| r.0);
    let mark = read_metadata_seq(conn, EVENT_LOG_PRUNE_MARK)?;
    let pruned_through = read_metadata_seq(conn, EVENT_LOG_PRUNED_THROUGH)?.map_or(0, |r| r.0);

    let mark_is_due = match &mark {
        Some((_, updated_at)) => parse_iso8601_utc_millis(updated_at)
            .is_some_and(|mark_ms| now_ms.saturating_sub(mark_ms) >= RETAINED_EVENT_HISTORY_MS),
        None => false,
    };

    let mut deleted = 0_u64;
    if let Some((mark_seq, _)) = mark
        && mark_is_due
    {
        let through_sequence = mark_seq.min(projected).max(pruned_through);
        if through_sequence > pruned_through {
            write_metadata(conn, EVENT_LOG_PRUNED_THROUGH, through_sequence, &now_iso)?;
        }
        deleted += delete_prunable_events(conn, through_sequence, head)?;
    }

    if mark.is_none() || mark_is_due {
        write_metadata(conn, EVENT_LOG_PRUNE_MARK, head, &now_iso)?;
    }

    Ok(deleted)
}

fn read_metadata_seq(conn: &Connection, name: &str) -> rusqlite::Result<Option<(i64, String)>> {
    match conn.query_row(
        "SELECT last_sequence, updated_at
         FROM orchestration_v2_projection_metadata
         WHERE projection_name = ?1",
        [name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ) {
        Ok(pair) => Ok(Some(pair)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err),
    }
}

fn write_metadata(
    conn: &Connection,
    name: &str,
    sequence: i64,
    updated_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO orchestration_v2_projection_metadata (
           projection_name, schema_version, last_sequence, updated_at
         )
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(projection_name) DO UPDATE SET
           last_sequence = excluded.last_sequence,
           updated_at = excluded.updated_at",
        params![
            name,
            ORCHESTRATION_V2_PROJECTION_SCHEMA_VERSION,
            sequence,
            updated_at
        ],
    )?;
    Ok(())
}

fn delete_prunable_events(
    conn: &mut Connection,
    through_sequence: i64,
    head: i64,
) -> rusqlite::Result<u64> {
    if through_sequence <= 0 {
        return Ok(0);
    }

    let mut referenced = read_prompt_references(conn, through_sequence, head)?;
    let mut references_through = head;
    let mut deleted = 0_u64;
    let mut cursor = 0_i64;

    while cursor < through_sequence {
        let mut stmt = conn.prepare(
            "SELECT
               event.sequence,
               CASE
                 WHEN event.application_event_version = 1 THEN 0
                 WHEN event.event_type = 'thread.created' THEN 0
                 WHEN event.event_type = 'run.updated'
                   AND json_extract(event.payload_json, '$.status') = 'running' THEN 0
                 WHEN event.sequence = (
                   SELECT MAX(newer.sequence) FROM orchestration_events AS newer
                   WHERE newer.aggregate_kind = event.aggregate_kind
                     AND newer.stream_id = event.stream_id
                 ) THEN 0
                 ELSE 1
               END AS prunable
             FROM orchestration_events AS event
             WHERE event.sequence > ?1 AND event.sequence <= ?2
             ORDER BY event.sequence ASC
             LIMIT ?3",
        )?;
        let rows: Vec<(i64, i64)> = stmt
            .query_map(params![cursor, through_sequence, PAGE_SIZE], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        if rows.is_empty() {
            break;
        }
        let next_cursor = rows.last().map_or(through_sequence, |r| r.0);
        let candidates: Vec<i64> = rows
            .iter()
            .filter_map(|(seq, prunable)| (*prunable == 1).then_some(*seq))
            .collect();

        if !candidates.is_empty() {
            let tx = conn.transaction()?;
            let mut app_stmt = tx.prepare(
                "SELECT
                   sequence,
                   CASE
                     WHEN event_type IN ('subagent.updated', 'turn-item.updated')
                       AND json_type(payload_json, '$.promptSequence') = 'integer'
                     THEN json_extract(payload_json, '$.promptSequence')
                   END AS prompt_sequence
                 FROM orchestration_events
                 WHERE sequence > ?1",
            )?;
            let appended = app_stmt
                .query_map([references_through], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(app_stmt);

            for (seq, prompt_seq) in appended {
                if let Some(prompt_seq) = prompt_seq {
                    referenced.insert(prompt_seq);
                }
                references_through = references_through.max(seq);
            }

            let obsolete: Vec<i64> = candidates
                .into_iter()
                .filter(|seq| !referenced.contains(seq))
                .collect();
            for seq in &obsolete {
                deleted += u64::try_from(tx.execute(
                    "DELETE FROM orchestration_events WHERE sequence = ?1",
                    [seq],
                )?)
                .unwrap_or(0);
            }
            tx.commit()?;
        }

        if i64::try_from(rows.len()).unwrap_or(PAGE_SIZE) < PAGE_SIZE {
            break;
        }
        cursor = next_cursor;
    }

    Ok(deleted)
}

fn read_prompt_references(
    conn: &Connection,
    after: i64,
    through: i64,
) -> rusqlite::Result<HashSet<i64>> {
    let mut referenced = HashSet::new();
    let mut cursor = after;
    while cursor < through {
        let window_end = (cursor + REFERENCE_SCAN_WINDOW).min(through);
        let mut stmt = conn.prepare(
            "SELECT json_extract(payload_json, '$.promptSequence')
             FROM orchestration_events
             WHERE sequence > ?1
               AND sequence <= ?2
               AND event_type IN ('subagent.updated', 'turn-item.updated')
               AND json_type(payload_json, '$.promptSequence') = 'integer'",
        )?;
        for row in stmt.query_map([cursor, window_end], |row| row.get::<_, i64>(0))? {
            referenced.insert(row?);
        }
        cursor = window_end;
    }

    let mut head_stmt = conn.prepare(
        "SELECT json_extract(event.payload_json, '$.promptSequence')
         FROM (
           SELECT MAX(sequence) AS sequence FROM orchestration_events
           GROUP BY aggregate_kind, stream_id
         ) AS heads
         JOIN orchestration_events AS event ON event.sequence = heads.sequence
         WHERE heads.sequence <= ?1
           AND event.event_type IN ('subagent.updated', 'turn-item.updated')
           AND json_type(event.payload_json, '$.promptSequence') = 'integer'",
    )?;
    for row in head_stmt.query_map([after], |row| row.get::<_, i64>(0))? {
        referenced.insert(row?);
    }

    Ok(referenced)
}

fn prune_imported_legacy_history(
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

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    match conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE name = ?1",
        [name],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(err) => Err(err),
    }
}
