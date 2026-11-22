//! Wire shell snapshots, incremental stream patch builders, and older-settled thread paging.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use rusqlite::{Connection, OptionalExtension, params};

use crate::projections::ProjectionError;
use crate::projections::wire_projects::{
    compute_project_identities_digest, query_wire_project_shell_by_id, query_wire_project_shells,
};
use crate::projections::wire_shell_rows::load_wire_thread_shells;
use crate::projections::wire_shell_trim::trim_wire_shell_threads;
use crate::projections::wire_types::{
    WireOlderSettledCount, WireOlderSettledCursor, WireOlderSettledThreadsPage,
    WireShellHttpSnapshot, WireShellOptions, WireShellSnapshot, WireShellStreamEvent,
    WireThreadShell,
};
use crate::time::now_iso_utc;

const ORCHESTRATION_V2_PROJECTION_SCHEMA_VERSION: i64 = 2;
const EVENT_LOG_PRUNED_THROUGH: &str = "event-log-pruned-through";
const SHELL_RESUME_MAX_GAP: i64 = 1_000;

/// Queries the full HTTP shell snapshot (`OrchestrationV2ShellHttpSnapshot`) using default trimming.
pub fn query_wire_shell_http_snapshot(
    conn: &Connection,
    environment_id: &str,
) -> Result<WireShellHttpSnapshot, ProjectionError> {
    query_wire_shell_http_snapshot_with_options(conn, environment_id, WireShellOptions::default())
}

/// Queries `OrchestrationV2ShellHttpSnapshot` with explicit subscriber trimming options.
pub fn query_wire_shell_http_snapshot_with_options(
    conn: &Connection,
    environment_id: &str,
    options: WireShellOptions,
) -> Result<WireShellHttpSnapshot, ProjectionError> {
    let _ = environment_id;
    let snapshot_sequence = query_latest_application_sequence(conn)?;
    let projects = query_wire_project_shells(conn)?;
    let raw_threads = load_wire_thread_shells(conn, None, false)?;
    let now = now_iso_utc();
    let (threads, older_settled) =
        trim_wire_shell_threads(raw_threads, options, snapshot_sequence, &now);
    let project_identities = Some(compute_project_identities_digest(&projects));
    Ok(WireShellHttpSnapshot {
        schema_version: ORCHESTRATION_V2_PROJECTION_SCHEMA_VERSION,
        snapshot_sequence,
        threads,
        archived_threads: Vec::new(),
        projects,
        older_settled,
        project_identities,
    })
}

/// Builds the full `OrchestrationV2ShellStreamItem` `"snapshot"` reset payload.
pub fn query_wire_shell_stream_reset(
    conn: &Connection,
    environment_id: &str,
) -> Result<WireShellStreamEvent, ProjectionError> {
    let http = query_wire_shell_http_snapshot(conn, environment_id)?;
    Ok(WireShellStreamEvent::Snapshot {
        snapshot: Box::new(WireShellSnapshot {
            schema_version: http.schema_version,
            snapshot_sequence: http.snapshot_sequence,
            threads: http.threads,
            archived_threads: http.archived_threads,
            projects: http.projects,
            older_settled: http.older_settled,
        }),
        resolved_repository_identity_roots: None,
    })
}

/// Builds coalesced incremental `OrchestrationV2ShellStreamItem` patch events after `after_sequence`.
///
/// Returns `[WireShellStreamEvent::SnapshotRequired]` when the replay cursor falls behind the
/// retained event log prune mark or exceeds the replay gap ceiling (`1_000` sequences).
#[expect(
    clippy::implicit_hasher,
    reason = "concrete std HashSet lets callers pass None without turbofish inference errors"
)]
pub fn build_wire_shell_patch_from_events(
    conn: &Connection,
    after_sequence: i64,
    environment_id: &str,
    mut subagent_thread_ids: Option<&mut HashSet<String>>,
) -> Result<Vec<WireShellStreamEvent>, ProjectionError> {
    let _ = environment_id;
    let head = query_latest_application_sequence(conn)?;
    if after_sequence == head {
        return Ok(Vec::new());
    }
    let gap = head - after_sequence;
    let pruned_through = query_pruned_through_sequence(conn)?;
    if !(0..=SHELL_RESUME_MAX_GAP).contains(&gap) || after_sequence < pruned_through {
        return Ok(vec![WireShellStreamEvent::SnapshotRequired]);
    }

    let mut stmt = conn.prepare(
        "SELECT sequence, aggregate_kind, stream_id, event_type, payload_json
         FROM orchestration_events
         WHERE sequence > ?1
           AND sequence <= ?2
           AND (aggregate_kind = 'project' OR (application_event_version = 2 AND aggregate_kind = 'thread'))
         ORDER BY sequence ASC",
    )?;
    let rows = stmt
        .query_map(params![after_sequence, head], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut latest_by_aggregate: BTreeMap<String, (i64, String, String, String)> = BTreeMap::new();
    for (seq, kind, stream_id, event_type, payload_json) in rows {
        if kind == "thread"
            && let Some(ref mut set) = subagent_thread_ids
            && should_skip_subagent_event(&event_type, &stream_id, &payload_json, set)
        {
            continue;
        }
        let key = format!("{kind}:{stream_id}");
        latest_by_aggregate.insert(key, (seq, kind, stream_id, event_type));
    }

    let mut coalesced: Vec<(i64, String, String, String)> =
        latest_by_aggregate.into_values().collect();
    coalesced.sort_by_key(|entry| entry.0);

    let mut events = Vec::with_capacity(coalesced.len());
    for (seq, kind, stream_id, event_type) in coalesced {
        if kind == "project" {
            if event_type == "project.deleted" {
                events.push(WireShellStreamEvent::ProjectRemoved {
                    sequence: seq,
                    project_id: stream_id,
                });
            } else if let Some(project) = query_wire_project_shell_by_id(conn, &stream_id)? {
                events.push(WireShellStreamEvent::ProjectUpdated {
                    sequence: seq,
                    project: Box::new(project),
                });
            } else {
                events.push(WireShellStreamEvent::ProjectRemoved {
                    sequence: seq,
                    project_id: stream_id,
                });
            }
        } else if let Some(shell) = load_wire_thread_shells(conn, Some(&stream_id), true)?
            .into_iter()
            .next()
        {
            if shell.archived_at.is_some() {
                events.push(WireShellStreamEvent::ThreadRemoved {
                    sequence: seq,
                    location: "active".to_owned(),
                    thread_id: stream_id,
                });
            } else {
                events.push(WireShellStreamEvent::ThreadUpdated {
                    sequence: seq,
                    location: "active".to_owned(),
                    thread: Box::new(shell),
                });
            }
        } else {
            events.push(WireShellStreamEvent::ThreadRemoved {
                sequence: seq,
                location: "active".to_owned(),
                thread_id: stream_id,
            });
        }
    }
    Ok(events)
}

/// Queries paged older settled threads (`OrchestrationV2OlderSettledThreadsPage`).
pub fn list_wire_older_settled_threads(
    conn: &Connection,
    project_id: Option<&str>,
    cursor: Option<&WireOlderSettledCursor>,
    limit: usize,
) -> Result<WireOlderSettledThreadsPage, ProjectionError> {
    let snapshot_sequence = query_latest_application_sequence(conn)?;
    let mut base_sql = String::from(
        "SELECT t.thread_id, t.project_id, json_extract(t.payload_json, '$.settledAt') AS settled_at
         FROM orchestration_v2_projection_threads t
         WHERE t.deleted_at IS NULL
           AND t.archived_at IS NULL
           AND json_extract(t.payload_json, '$.lineage.relationshipToParent') IS NOT 'subagent'
           AND json_extract(t.payload_json, '$.lineage.sideChat') IS NOT 1
           AND json_extract(t.payload_json, '$.settledOverride') = 'settled'
           AND json_extract(t.payload_json, '$.settledAt') IS NOT NULL",
    );
    let mut owned_params: Vec<String> = Vec::new();
    if let Some(pid) = project_id {
        owned_params.push(pid.to_owned());
        let _ = write!(base_sql, " AND t.project_id = ?{}", owned_params.len());
    }
    if let Some(c) = cursor {
        owned_params.push(c.settled_at.clone());
        let p_ts = owned_params.len();
        if let Some(ref tid) = c.thread_id {
            owned_params.push(tid.clone());
            let p_tid = owned_params.len();
            let _ = write!(
                base_sql,
                " AND (json_extract(t.payload_json, '$.settledAt') < ?{p_ts}
                   OR (json_extract(t.payload_json, '$.settledAt') = ?{p_ts} AND t.thread_id > ?{p_tid}))"
            );
        } else {
            let _ = write!(
                base_sql,
                " AND json_extract(t.payload_json, '$.settledAt') < ?{p_ts}"
            );
        }
    }

    if limit == 0 {
        let count_sql = format!(
            "WITH older AS ({base_sql})
             SELECT project_id, COUNT(*) AS count FROM older GROUP BY project_id ORDER BY project_id ASC"
        );
        let mut stmt = conn.prepare(&count_sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = owned_params
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let counts = stmt
            .query_map(param_refs.as_slice(), |r| {
                Ok(WireOlderSettledCount {
                    project_id: r.get(0)?,
                    count: r.get::<_, i64>(1)?.max(0).cast_unsigned(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        return Ok(WireOlderSettledThreadsPage {
            snapshot_sequence,
            threads: Vec::new(),
            counts,
        });
    }

    let capped_limit = i64::try_from(limit.min(50)).unwrap_or(50);
    let page_sql =
        format!("{base_sql} ORDER BY settled_at DESC, t.thread_id ASC LIMIT {capped_limit}");
    let mut stmt = conn.prepare(&page_sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = owned_params
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();
    let thread_ids = stmt
        .query_map(param_refs.as_slice(), |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut threads: Vec<WireThreadShell> = Vec::with_capacity(thread_ids.len());
    for tid in &thread_ids {
        if let Some(shell) = load_wire_thread_shells(conn, Some(tid), false)?
            .into_iter()
            .next()
        {
            threads.push(shell);
        }
    }
    Ok(WireOlderSettledThreadsPage {
        snapshot_sequence,
        threads,
        counts: Vec::new(),
    })
}

pub(crate) fn query_latest_application_sequence(conn: &Connection) -> Result<i64, ProjectionError> {
    let ev_seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0)
         FROM orchestration_events
         WHERE aggregate_kind = 'project'
            OR (application_event_version = 2 AND aggregate_kind = 'thread')",
        [],
        |row| row.get(0),
    )?;
    let meta_seq: i64 = conn
        .query_row(
            "SELECT last_sequence FROM orchestration_v2_projection_metadata
             WHERE projection_name = 'thread-projections' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    Ok(ev_seq.max(meta_seq))
}

fn query_pruned_through_sequence(conn: &Connection) -> Result<i64, ProjectionError> {
    Ok(conn
        .query_row(
            "SELECT last_sequence FROM orchestration_v2_projection_metadata
             WHERE projection_name = ?1 LIMIT 1",
            params![EVENT_LOG_PRUNED_THROUGH],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

fn should_skip_subagent_event(
    event_type: &str,
    stream_id: &str,
    payload_json: &str,
    subagent_ids: &mut HashSet<String>,
) -> bool {
    if event_type == "thread.created" {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload_json)
            && val
                .get("lineage")
                .and_then(|l| l.get("relationshipToParent"))
                .and_then(|v| v.as_str())
                == Some("subagent")
        {
            subagent_ids.insert(stream_id.to_owned());
        }
        return false;
    }
    if event_type == "thread.deleted" {
        return false;
    }
    if event_type == "turn-item.updated"
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(payload_json)
        && matches!(
            val.get("type").and_then(|v| v.as_str()),
            Some("approval_request" | "user_input_request" | "secret_request" | "link_request")
        )
    {
        return false;
    }
    subagent_ids.contains(stream_id)
        && matches!(
            event_type,
            "node.updated" | "turn-item.updated" | "provider-turn.updated" | "message.updated"
        )
}
