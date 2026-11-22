//! Extended transactional projection upserts for provider sessions, threads, turns,
//! runtime requests, and subagents.

use legion_contracts::orchestration::DomainEvent;
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::sink::types::SinkError;

fn to_json(val: &impl serde::Serialize) -> Result<(Value, String), SinkError> {
    let v = serde_json::to_value(val).map_err(|e| SinkError::Codec(e.to_string()))?;
    let s = serde_json::to_string(&v).map_err(|e| SinkError::Codec(e.to_string()))?;
    Ok((v, s))
}

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

pub(crate) fn upsert_subagent(
    conn: &Connection,
    sub: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(sub)?;
    let inst = str_at(&v, "providerInstanceId").unwrap_or("codex");
    let driver = str_at(&v, "driver");
    let delivery_state = v
        .get("completionDelivery")
        .and_then(|d| d.get("state"))
        .and_then(Value::as_str);
    conn.execute(
        "INSERT INTO orchestration_v2_projection_subagents (
           subagent_id, thread_id, run_id, parent_node_id, provider,
           provider_thread_id, child_thread_id, origin, status,
           started_at, completed_at, updated_at, payload_json,
           driver, provider_instance_id, completion_delivery_state
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(subagent_id) DO UPDATE SET
           run_id = excluded.run_id,
           provider_thread_id = excluded.provider_thread_id,
           child_thread_id = excluded.child_thread_id,
           status = excluded.status,
           started_at = excluded.started_at,
           completed_at = excluded.completed_at,
           updated_at = excluded.updated_at,
           payload_json = excluded.payload_json,
           driver = excluded.driver,
           provider_instance_id = excluded.provider_instance_id,
           completion_delivery_state = excluded.completion_delivery_state",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "runId"),
            str_at(&v, "parentNodeId").unwrap_or(""),
            inst,
            str_at(&v, "providerThreadId"),
            str_at(&v, "childThreadId"),
            str_at(&v, "origin").unwrap_or("provider_native"),
            str_at(&v, "status").unwrap_or("pending"),
            str_at(&v, "startedAt"),
            str_at(&v, "completedAt"),
            str_at(&v, "updatedAt").unwrap_or(""),
            json,
            driver,
            inst,
            delivery_state,
        ],
    )?;
    Ok(())
}

pub(crate) fn upsert_provider_session(
    conn: &Connection,
    tid: &str,
    session: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(session)?;
    let sid = str_at(&v, "id").unwrap_or("");
    let inst = str_at(&v, "providerInstanceId").unwrap_or("codex");
    conn.execute(
        "INSERT INTO orchestration_v2_projection_provider_sessions (
           provider_session_id, thread_id, provider, status, model,
           updated_at, payload_json, driver, provider_instance_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(provider_session_id) DO UPDATE SET
           thread_id = excluded.thread_id,
           status = excluded.status,
           model = excluded.model,
           updated_at = excluded.updated_at,
           payload_json = excluded.payload_json,
           driver = excluded.driver,
           provider_instance_id = excluded.provider_instance_id",
        params![
            sid,
            tid,
            inst,
            str_at(&v, "status").unwrap_or("starting"),
            str_at(&v, "model"),
            str_at(&v, "updatedAt").unwrap_or(""),
            json,
            str_at(&v, "driver"),
            inst,
        ],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO orchestration_v2_projection_provider_session_bindings (
           provider_session_id, thread_id
         ) VALUES (?1, ?2)",
        params![sid, tid],
    )?;
    Ok(())
}

pub(crate) fn upsert_provider_thread(
    conn: &Connection,
    tid: &str,
    pth: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(pth)?;
    let inst = str_at(&v, "providerInstanceId").unwrap_or("codex");
    let eff_tid = str_at(&v, "appThreadId").unwrap_or(tid);
    conn.execute(
        "INSERT INTO orchestration_v2_projection_provider_threads (
           provider_thread_id, thread_id, owner_node_id, provider, provider_session_id,
           status, first_run_ordinal, last_run_ordinal, updated_at, payload_json,
           driver, provider_instance_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(provider_thread_id) DO UPDATE SET
           thread_id = excluded.thread_id,
           owner_node_id = excluded.owner_node_id,
           provider_session_id = excluded.provider_session_id,
           status = excluded.status,
           first_run_ordinal = excluded.first_run_ordinal,
           last_run_ordinal = excluded.last_run_ordinal,
           updated_at = excluded.updated_at,
           payload_json = excluded.payload_json,
           driver = excluded.driver,
           provider_instance_id = excluded.provider_instance_id",
        params![
            str_at(&v, "id").unwrap_or(""),
            eff_tid,
            str_at(&v, "ownerNodeId"),
            inst,
            str_at(&v, "providerSessionId"),
            str_at(&v, "status").unwrap_or("idle"),
            v.get("firstRunOrdinal").and_then(Value::as_i64),
            v.get("lastRunOrdinal").and_then(Value::as_i64),
            str_at(&v, "updatedAt").unwrap_or(""),
            json,
            str_at(&v, "driver"),
            inst,
        ],
    )?;
    Ok(())
}

pub(crate) fn upsert_provider_turn(
    conn: &Connection,
    tid: &str,
    pturn: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(pturn)?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_provider_turns (
           provider_turn_id, thread_id, provider_thread_id, node_id, run_attempt_id,
           ordinal, status, started_at, completed_at, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(provider_turn_id) DO UPDATE SET
           run_attempt_id = excluded.run_attempt_id,
           status = excluded.status,
           started_at = excluded.started_at,
           completed_at = excluded.completed_at,
           payload_json = excluded.payload_json",
        params![
            str_at(&v, "id").unwrap_or(""),
            tid,
            str_at(&v, "providerThreadId").unwrap_or(""),
            str_at(&v, "nodeId").unwrap_or(""),
            str_at(&v, "runAttemptId"),
            v.get("ordinal").and_then(Value::as_i64).unwrap_or(1),
            str_at(&v, "status").unwrap_or("pending"),
            str_at(&v, "startedAt"),
            str_at(&v, "completedAt"),
            json,
        ],
    )?;
    Ok(())
}

pub(crate) fn upsert_runtime_request(
    conn: &Connection,
    event: &DomainEvent,
    req: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(req)?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_runtime_requests (
           runtime_request_id, thread_id, node_id, provider_turn_id, kind, status,
           created_at, resolved_at, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(runtime_request_id) DO UPDATE SET
           status = excluded.status,
           resolved_at = excluded.resolved_at,
           payload_json = excluded.payload_json",
        params![
            str_at(&v, "id").unwrap_or(""),
            event.thread_id.as_str(),
            str_at(&v, "nodeId").unwrap_or(""),
            str_at(&v, "providerTurnId"),
            str_at(&v, "kind").unwrap_or("approval"),
            str_at(&v, "status").unwrap_or("pending"),
            str_at(&v, "createdAt").unwrap_or(""),
            str_at(&v, "resolvedAt"),
            json,
        ],
    )?;
    Ok(())
}
