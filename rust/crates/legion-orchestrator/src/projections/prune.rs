//! 24-hour V2 event store retention pruning preserving V1 rows, `thread.created`, active runs, and stream heads.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::projections::ProjectionError;
use crate::time::{format_millis_iso, now_iso_utc, parse_iso_millis};

/// 24-hour retention window applied to historical V2 domain events (`ProjectionMaintenance.ts`).
pub const RETAINED_EVENT_HISTORY_MS: i64 = 24 * 60 * 60 * 1_000;

/// Projection metadata key tracking when event pruning was initialized/last run.
pub const EVENT_LOG_PRUNE_MARK_PROJECTION: &str = "event-log-prune-mark";

/// Projection metadata key storing the highest deleted event sequence.
pub const EVENT_LOG_PRUNED_THROUGH_PROJECTION: &str = "event-log-pruned-through";

const METADATA_SCHEMA_VERSION: i64 = 1;

/// Report returned by [`prune_event_store`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneEventStoreReport {
    /// Whether the 24-hour mark had elapsed and pruning executed.
    pub ran: bool,
    /// ISO-8601 UTC cutoff timestamp applied (`now_iso - 24h`), if pruning ran.
    pub cutoff_iso: Option<String>,
    /// High-water `orchestration_events.sequence` deleted by pruning (`0` if none yet).
    pub pruned_through_sequence: i64,
    /// Number of `orchestration_events` rows deleted in this call.
    pub deleted_event_count: usize,
    /// Number of completed `orchestration_v2_legacy_imports` rows deleted in this call.
    pub deleted_legacy_import_count: usize,
    /// Estimated `SQLite` freelist bytes reclaimable after deletes (`freelist_count * page_size`).
    pub reclaimable_bytes: i64,
}

/// Runs 24-hour retention pruning against `orchestration_events` at `now_iso`.
///
/// Matches `ProjectionMaintenance.ts`:
/// - On first call (`0h`), writes `event-log-prune-mark` and performs no deletes.
/// - Within `< 24h` since `event-log-prune-mark.updated_at`, returns `ran: false`.
/// - At `>= 24h`, deletes eligible `application_event_version = 2` events with
///   `occurred_at <= now_iso - 24h` while preserving `application_event_version = 1`,
///   `thread.created`, active runs, stream heads, and `promptSequence` anchors.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use legion_orchestrator::projections::prune_event_store;
/// use legion_persistence::sqlite::SqliteStore;
///
/// let store = SqliteStore::open_in_memory()?;
/// let first = prune_event_store(store.connection(), "2026-11-20T00:00:00.000Z")?;
/// assert!(!first.ran);
/// let noop = prune_event_store(store.connection(), "2026-11-20T23:00:00.000Z")?;
/// assert!(!noop.ran);
/// let pruned = prune_event_store(store.connection(), "2026-11-21T00:00:00.000Z")?;
/// assert!(pruned.ran);
/// # Ok(())
/// # }
/// ```
pub fn prune_event_store(
    conn: &Connection,
    now_iso: &str,
) -> Result<PruneEventStoreReport, ProjectionError> {
    let now_ms = parse_iso_millis(now_iso)
        .ok_or_else(|| ProjectionError::Codec(format!("invalid ISO timestamp `{now_iso}`")))?;

    conn.execute_batch("SAVEPOINT legion_prune_events;")?;
    let res = prune_in_savepoint(conn, now_iso, now_ms);
    match res {
        Ok(report) => {
            conn.execute_batch("RELEASE SAVEPOINT legion_prune_events;")?;
            Ok(report)
        }
        Err(err) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT legion_prune_events; RELEASE SAVEPOINT legion_prune_events;",
            );
            Err(err)
        }
    }
}

/// Convenience wrapper running [`prune_event_store`] using the current wall-clock UTC time.
pub fn prune_event_store_now(conn: &Connection) -> Result<PruneEventStoreReport, ProjectionError> {
    prune_event_store(conn, &now_iso_utc())
}

fn prune_in_savepoint(
    conn: &Connection,
    now_iso: &str,
    now_ms: i64,
) -> Result<PruneEventStoreReport, ProjectionError> {
    let existing_mark = load_metadata(conn, EVENT_LOG_PRUNE_MARK_PROJECTION)?;
    let prev_pruned_through =
        load_metadata(conn, EVENT_LOG_PRUNED_THROUGH_PROJECTION)?.map_or(0, |(seq, _)| seq);

    match existing_mark {
        None => {
            upsert_metadata(conn, EVENT_LOG_PRUNE_MARK_PROJECTION, 0, now_iso)?;
            return Ok(PruneEventStoreReport {
                ran: false,
                cutoff_iso: None,
                pruned_through_sequence: prev_pruned_through,
                deleted_event_count: 0,
                deleted_legacy_import_count: 0,
                reclaimable_bytes: query_reclaimable_bytes(conn)?,
            });
        }
        Some((_, mark_updated_at)) => {
            let mark_ms = parse_iso_millis(&mark_updated_at).ok_or_else(|| {
                ProjectionError::Codec(format!("corrupt prune mark timestamp `{mark_updated_at}`"))
            })?;
            if now_ms.saturating_sub(mark_ms) < RETAINED_EVENT_HISTORY_MS {
                return Ok(PruneEventStoreReport {
                    ran: false,
                    cutoff_iso: None,
                    pruned_through_sequence: prev_pruned_through,
                    deleted_event_count: 0,
                    deleted_legacy_import_count: 0,
                    reclaimable_bytes: query_reclaimable_bytes(conn)?,
                });
            }
        }
    }

    let cutoff_iso = format_millis_iso(now_ms.saturating_sub(RETAINED_EVENT_HISTORY_MS));
    let deleted_seqs = delete_prunable_v2_events(conn, &cutoff_iso)?;
    let deleted_event_count = deleted_seqs.len();
    let max_deleted_seq = deleted_seqs
        .into_iter()
        .max()
        .unwrap_or(prev_pruned_through)
        .max(prev_pruned_through);

    let deleted_legacy_import_count = conn.execute(
        "DELETE FROM orchestration_v2_legacy_imports
         WHERE transcript_imported_at IS NOT NULL AND transcript_imported_at <= ?1",
        params![cutoff_iso],
    )?;

    upsert_metadata(
        conn,
        EVENT_LOG_PRUNED_THROUGH_PROJECTION,
        max_deleted_seq,
        now_iso,
    )?;
    upsert_metadata(
        conn,
        EVENT_LOG_PRUNE_MARK_PROJECTION,
        max_deleted_seq,
        now_iso,
    )?;

    Ok(PruneEventStoreReport {
        ran: true,
        cutoff_iso: Some(cutoff_iso),
        pruned_through_sequence: max_deleted_seq,
        deleted_event_count,
        deleted_legacy_import_count,
        reclaimable_bytes: query_reclaimable_bytes(conn)?,
    })
}

fn delete_prunable_v2_events(
    conn: &Connection,
    cutoff_iso: &str,
) -> Result<Vec<i64>, ProjectionError> {
    let mut stmt = conn.prepare(
        "DELETE FROM orchestration_events
         WHERE application_event_version = 2
           AND occurred_at <= ?1
           AND event_type != 'thread.created'
           AND NOT (
             event_type IN ('run.created', 'run.updated')
             AND COALESCE(
               (
                 SELECT ar.status
                 FROM orchestration_v2_projection_runs AS ar
                 WHERE ar.run_id = COALESCE(
                   json_extract(orchestration_events.payload_json, '$.id'),
                   json_extract(orchestration_events.payload_json, '$.run.id')
                 )
               ),
               json_extract(orchestration_events.payload_json, '$.status'),
               json_extract(orchestration_events.payload_json, '$.run.status'),
               ''
             ) IN ('preparing', 'queued', 'starting', 'running', 'waiting')
           )
           AND json_extract(payload_json, '$.promptSequence') IS NULL
           AND json_extract(payload_json, '$.run.promptSequence') IS NULL
           AND sequence != COALESCE(
             (
               SELECT MAX(head.sequence)
               FROM orchestration_events AS head
               WHERE head.aggregate_kind = orchestration_events.aggregate_kind
                 AND head.stream_id = orchestration_events.stream_id
             ),
             -1
           )
         RETURNING sequence",
    )?;
    let seqs = stmt
        .query_map(params![cutoff_iso], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(seqs)
}

fn load_metadata(conn: &Connection, name: &str) -> Result<Option<(i64, String)>, ProjectionError> {
    conn.query_row(
        "SELECT last_sequence, updated_at
         FROM orchestration_v2_projection_metadata
         WHERE projection_name = ?1
         LIMIT 1",
        params![name],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
    .map_err(ProjectionError::Sqlite)
}

fn upsert_metadata(
    conn: &Connection,
    name: &str,
    last_sequence: i64,
    updated_at: &str,
) -> Result<(), ProjectionError> {
    conn.execute(
        "INSERT INTO orchestration_v2_projection_metadata (
           projection_name, schema_version, last_sequence, updated_at
         )
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(projection_name) DO UPDATE SET
           schema_version = excluded.schema_version,
           last_sequence = excluded.last_sequence,
           updated_at = excluded.updated_at",
        params![name, METADATA_SCHEMA_VERSION, last_sequence, updated_at],
    )?;
    Ok(())
}

fn query_reclaimable_bytes(conn: &Connection) -> Result<i64, ProjectionError> {
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |row| row.get(0))?;
    Ok(page_size.saturating_mul(freelist_count))
}
