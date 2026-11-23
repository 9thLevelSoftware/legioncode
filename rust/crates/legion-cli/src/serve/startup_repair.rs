//! Idempotent startup repair for stale V2 SQLite projection rows.
//!
//! Heals malformed `titleRegeneration` / `lastTitleRegeneration` objects,
//! legacy `settledOverride: "unsettled"` values, and active/running runs whose
//! `provider-turn.start` outbox effect permanently failed prior to exit.

use legion_contracts::ids::{RunId, ThreadId, TurnItemId};
use legion_contracts::orchestration::{
    ProviderFailure, ProviderFailureClass, TurnItem, TurnItemBase, TurnItemKind, TurnItemStatus,
};
use legion_orchestrator::Orchestrator;
use legion_orchestrator::projections::wire_codec::{
    sanitize_last_title_regeneration_value, sanitize_title_regeneration_value,
};
use legion_orchestrator::time::now_iso_utc;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

pub(crate) fn repair_startup_state(orchestrator: &Orchestrator) {
    let now_iso = now_iso_utc();
    let Ok(conn) = orchestrator.sink().db().lock() else {
        eprintln!("[startup-repair] warning: failed to acquire SQLite connection lock");
        return;
    };
    if let Err(err) = repair_in_tx(&conn, &now_iso) {
        let _ = conn.execute_batch("ROLLBACK;");
        eprintln!("[startup-repair] warning: startup repair failed: {err}");
    }
}

fn repair_in_tx(conn: &Connection, now_iso: &str) -> Result<(), rusqlite::Error> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    repair_thread_projections(conn)?;
    repair_orphaned_runs(conn, now_iso)?;
    conn.execute_batch("COMMIT;")?;
    Ok(())
}

fn repair_thread_projections(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut stmt =
        conn.prepare("SELECT thread_id, payload_json FROM orchestration_v2_projection_threads")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    for (thread_id, payload_json) in rows {
        let Ok(mut doc) = serde_json::from_str::<Value>(&payload_json) else {
            continue;
        };
        let Some(obj) = doc.as_object_mut() else {
            continue;
        };
        let mut changed = false;
        if let Some(tr) = obj.get_mut("titleRegeneration") {
            let next_tr = if tr.is_null() {
                Value::Null
            } else {
                sanitize_title_regeneration_value(tr).unwrap_or(Value::Null)
            };
            if next_tr != *tr {
                *tr = next_tr;
                changed = true;
            }
        }
        if let Some(ltr) = obj.get_mut("lastTitleRegeneration") {
            let next_ltr = if ltr.is_null() {
                Value::Null
            } else {
                sanitize_last_title_regeneration_value(ltr).unwrap_or(Value::Null)
            };
            if next_ltr != *ltr {
                *ltr = next_ltr;
                changed = true;
            }
        }
        if obj.get("settledOverride").and_then(Value::as_str) == Some("unsettled") {
            obj.insert("settledOverride".to_owned(), json!("active"));
            changed = true;
        }
        if obj.get("settled_override").and_then(Value::as_str) == Some("unsettled") {
            obj.remove("settled_override");
            obj.insert("settledOverride".to_owned(), json!("active"));
            changed = true;
        }
        if changed && let Ok(updated_json) = serde_json::to_string(&doc) {
            conn.execute(
                "UPDATE orchestration_v2_projection_threads SET payload_json = ?1 WHERE thread_id = ?2",
                params![updated_json, thread_id],
            )?;
        }
    }
    Ok(())
}

struct OrphanRunRow {
    run_id: String,
    thread_id: String,
    payload_json: String,
    error_message: String,
}

fn repair_orphaned_runs(conn: &Connection, now_iso: &str) -> Result<(), rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT r.run_id, r.thread_id, r.payload_json,
                COALESCE(
                    (SELECT o.last_error FROM orchestration_v2_effect_outbox o
                      WHERE o.thread_id = r.thread_id
                        AND o.effect_type = 'provider-turn.start'
                        AND o.status = 'failed'
                        AND instr(o.payload_json, r.run_id) > 0
                      ORDER BY o.updated_at DESC LIMIT 1),
                    (SELECT o.last_error FROM orchestration_v2_effect_outbox o
                      WHERE o.thread_id = r.thread_id
                        AND o.effect_type = 'provider-turn.start'
                        AND o.status = 'failed'
                      ORDER BY o.updated_at DESC LIMIT 1),
                    ''
                ) AS error_msg
           FROM orchestration_v2_projection_runs r
          WHERE r.status IN ('running', 'starting', 'preparing')
            AND EXISTS (
                SELECT 1 FROM orchestration_v2_effect_outbox o
                 WHERE o.thread_id = r.thread_id
                   AND o.effect_type = 'provider-turn.start'
                   AND o.status = 'failed'
                   AND (
                       instr(o.payload_json, r.run_id) > 0
                       OR NOT EXISTS (
                           SELECT 1 FROM orchestration_v2_effect_outbox a
                            WHERE a.thread_id = r.thread_id
                              AND a.effect_type = 'provider-turn.start'
                              AND a.status IN ('pending', 'running')
                       )
                   )
            )",
    )?;
    let runs = stmt
        .query_map([], |row| {
            Ok(OrphanRunRow {
                run_id: row.get(0)?,
                thread_id: row.get(1)?,
                payload_json: row.get(2)?,
                error_message: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    for orphan in runs {
        mark_run_and_children_failed(conn, &orphan, now_iso)?;
        insert_missing_error_turn_item(conn, &orphan, now_iso)?;
    }
    Ok(())
}

fn mark_run_and_children_failed(
    conn: &Connection,
    orphan: &OrphanRunRow,
    now_iso: &str,
) -> Result<(), rusqlite::Error> {
    if let Ok(mut doc) = serde_json::from_str::<Value>(&orphan.payload_json)
        && let Some(obj) = doc.as_object_mut()
    {
        obj.insert("status".to_owned(), json!("failed"));
        if obj.get("completedAt").is_none_or(Value::is_null) {
            obj.insert("completedAt".to_owned(), json!(now_iso));
        }
        if let Ok(updated) = serde_json::to_string(&doc) {
            conn.execute(
                "UPDATE orchestration_v2_projection_runs
                    SET status = 'failed',
                        completed_at = COALESCE(completed_at, ?1),
                        payload_json = ?2
                  WHERE run_id = ?3",
                params![now_iso, updated, orphan.run_id],
            )?;
        }
    }

    let mut attempt_stmt = conn.prepare(
        "SELECT attempt_id, payload_json FROM orchestration_v2_projection_run_attempts
          WHERE run_id = ?1 AND status IN ('running', 'preparing', 'starting')",
    )?;
    let attempts = attempt_stmt
        .query_map(params![orphan.run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(attempt_stmt);

    for (attempt_id, attempt_json) in attempts {
        if let Ok(mut doc) = serde_json::from_str::<Value>(&attempt_json)
            && let Some(obj) = doc.as_object_mut()
        {
            obj.insert("status".to_owned(), json!("failed"));
            if obj.get("completedAt").is_none_or(Value::is_null) {
                obj.insert("completedAt".to_owned(), json!(now_iso));
            }
            if let Ok(updated) = serde_json::to_string(&doc) {
                conn.execute(
                    "UPDATE orchestration_v2_projection_run_attempts
                        SET status = 'failed', payload_json = ?1
                      WHERE attempt_id = ?2",
                    params![updated, attempt_id],
                )?;
            }
        }
    }

    let mut node_stmt = conn.prepare(
        "SELECT node_id, payload_json FROM orchestration_v2_projection_nodes
          WHERE run_id = ?1 AND status IN ('running', 'pending', 'starting')",
    )?;
    let nodes = node_stmt
        .query_map(params![orphan.run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(node_stmt);

    for (node_id, node_json) in nodes {
        if let Ok(mut doc) = serde_json::from_str::<Value>(&node_json)
            && let Some(obj) = doc.as_object_mut()
        {
            obj.insert("status".to_owned(), json!("failed"));
            if obj.get("completedAt").is_none_or(Value::is_null) {
                obj.insert("completedAt".to_owned(), json!(now_iso));
            }
            if let Ok(updated) = serde_json::to_string(&doc) {
                conn.execute(
                    "UPDATE orchestration_v2_projection_nodes
                        SET status = 'failed',
                            completed_at = COALESCE(completed_at, ?1),
                            payload_json = ?2
                      WHERE node_id = ?3",
                    params![now_iso, updated, node_id],
                )?;
            }
        }
    }
    Ok(())
}

fn insert_missing_error_turn_item(
    conn: &Connection,
    orphan: &OrphanRunRow,
    now_iso: &str,
) -> Result<(), rusqlite::Error> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM orchestration_v2_projection_turn_items
                        WHERE run_id = ?1 AND type = 'error')",
        params![orphan.run_id],
        |row| row.get(0),
    )?;
    if exists != 0 {
        return Ok(());
    }
    let next_ord: i64 = conn.query_row(
        "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM orchestration_v2_projection_turn_items
          WHERE thread_id = ?1",
        params![orphan.thread_id],
        |row| row.get(0),
    )?;
    let Ok(item_id) = TurnItemId::new(format!("item-repair-error-{}", orphan.run_id)) else {
        return Ok(());
    };
    let Ok(thread_id) = ThreadId::new(orphan.thread_id.clone()) else {
        return Ok(());
    };
    let Ok(run_id) = RunId::new(orphan.run_id.clone()) else {
        return Ok(());
    };
    let msg = if orphan.error_message.trim().is_empty() {
        "Provider turn failed prior to server restart".to_owned()
    } else {
        orphan.error_message.trim().chars().take(4_000).collect()
    };
    let ordinal = u64::try_from(next_ord.max(1)).unwrap_or(1);
    let item = TurnItem {
        base: TurnItemBase {
            deferred_work: None,
            tool_non_execution_kind: None,
            tool_surface: None,
            tool_icon: None,
            tool_source: None,
            id: item_id.clone(),
            thread_id,
            run_id: Some(run_id),
            node_id: None,
            provider_thread_id: None,
            provider_turn_id: None,
            native_item_ref: None,
            parent_item_id: None,
            ordinal,
            status: TurnItemStatus::Failed,
            title: None,
            started_at: None,
            completed_at: Some(now_iso.to_owned()),
            updated_at: now_iso.to_owned(),
        },
        kind: TurnItemKind::Error {
            failure: ProviderFailure {
                class: ProviderFailureClass::ProviderError,
                message: msg,
                code: None,
                retryable: Some(false),
                reset_at: None,
            },
            retry: None,
        },
    };
    let Ok(payload_json) = serde_json::to_string(&item) else {
        return Ok(());
    };
    conn.execute(
        "INSERT OR IGNORE INTO orchestration_v2_projection_turn_items (
           turn_item_id, thread_id, run_id, node_id, provider_thread_id, provider_turn_id,
           parent_item_id, ordinal, type, status, updated_at, payload_json, inline_work_output
         ) VALUES (?1, ?2, ?3, NULL, NULL, NULL, NULL, ?4, 'error', 'failed', ?5, ?6, 0)",
        params![
            item_id.as_str(),
            orphan.thread_id,
            orphan.run_id,
            next_ord.max(1),
            now_iso,
            payload_json,
        ],
    )?;
    Ok(())
}
