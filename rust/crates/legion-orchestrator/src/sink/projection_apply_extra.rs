//! Extended transactional projection upserts for nodes, turn items, checkpoints, plans,
//! and context handoffs/transfers.

use legion_contracts::orchestration::{DomainEvent, DomainEventPayload};
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::sink::projection_apply_runtime::{
    upsert_provider_session, upsert_provider_thread, upsert_provider_turn, upsert_runtime_request,
    upsert_subagent,
};
use crate::sink::types::SinkError;

pub(crate) fn apply_extended_event(
    conn: &Connection,
    event: &DomainEvent,
) -> Result<(), SinkError> {
    let tid = event.thread_id.as_str();
    match &event.payload {
        DomainEventPayload::RunAttemptUpdated { attempt, .. } => {
            upsert_run_attempt(conn, tid, attempt)
        }
        DomainEventPayload::NodeUpdated(node) => upsert_node(conn, node),
        DomainEventPayload::SubagentUpdated(subagent) => upsert_subagent(conn, subagent),
        DomainEventPayload::ProviderSessionUpdated { session, .. } => {
            upsert_provider_session(conn, tid, session)
        }
        DomainEventPayload::ProviderSessionDetached(detached) => {
            conn.execute(
                "DELETE FROM orchestration_v2_projection_provider_session_bindings
                 WHERE provider_session_id = ?1 AND thread_id = ?2",
                params![detached.provider_session_id.as_str(), tid],
            )?;
            Ok(())
        }
        DomainEventPayload::ProviderThreadUpdated(pth) => upsert_provider_thread(conn, tid, pth),
        DomainEventPayload::ProviderTurnUpdated(pturn) => upsert_provider_turn(conn, tid, pturn),
        DomainEventPayload::RuntimeRequestUpdated(req) => upsert_runtime_request(conn, event, req),
        DomainEventPayload::TurnItemUpdated(item) => upsert_turn_item(conn, item),
        DomainEventPayload::PlanUpdated(plan) => upsert_plan(conn, plan),
        DomainEventPayload::CheckpointScopeCreated(scope) => upsert_checkpoint_scope(conn, scope),
        DomainEventPayload::CheckpointCaptured(cp) => upsert_checkpoint(conn, cp),
        DomainEventPayload::ContextHandoffUpdated(h) => upsert_context_handoff(conn, h),
        DomainEventPayload::ContextTransferUpdated { transfer, .. } => {
            upsert_context_transfer(conn, transfer)
        }
        _ => Ok(()),
    }
}

fn to_json(val: &impl serde::Serialize) -> Result<(Value, String), SinkError> {
    let v = serde_json::to_value(val).map_err(|e| SinkError::Codec(e.to_string()))?;
    let s = serde_json::to_string(&v).map_err(|e| SinkError::Codec(e.to_string()))?;
    Ok((v, s))
}

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn upsert_run_attempt(
    conn: &Connection,
    tid: &str,
    attempt: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(attempt)?;
    let inst = str_at(&v, "providerInstanceId").unwrap_or("codex");
    conn.execute(
        "INSERT INTO orchestration_v2_projection_run_attempts (
           attempt_id, thread_id, run_id, attempt_ordinal, root_node_id,
           provider, provider_thread_id, provider_turn_id, status, payload_json, provider_instance_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(attempt_id) DO UPDATE SET
           provider_thread_id = excluded.provider_thread_id,
           provider_turn_id = excluded.provider_turn_id,
           status = excluded.status,
           payload_json = excluded.payload_json,
           provider_instance_id = excluded.provider_instance_id",
        params![
            str_at(&v, "id").unwrap_or(""),
            tid,
            str_at(&v, "runId").unwrap_or(""),
            v.get("attemptOrdinal").and_then(Value::as_i64).unwrap_or(1),
            str_at(&v, "rootNodeId").unwrap_or(""),
            inst,
            str_at(&v, "providerThreadId").unwrap_or(""),
            str_at(&v, "providerTurnId"),
            str_at(&v, "status").unwrap_or("preparing"),
            json,
            inst,
        ],
    )?;
    Ok(())
}

fn upsert_node(conn: &Connection, node: &impl serde::Serialize) -> Result<(), SinkError> {
    let (v, json) = to_json(node)?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_nodes (
           node_id, thread_id, run_id, parent_node_id, root_node_id, kind, status,
           provider_thread_id, provider_turn_id, runtime_request_id, checkpoint_scope_id,
           started_at, completed_at, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(node_id) DO UPDATE SET
           status = excluded.status,
           provider_thread_id = excluded.provider_thread_id,
           provider_turn_id = excluded.provider_turn_id,
           runtime_request_id = excluded.runtime_request_id,
           checkpoint_scope_id = excluded.checkpoint_scope_id,
           started_at = excluded.started_at,
           completed_at = excluded.completed_at,
           payload_json = excluded.payload_json",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "runId"),
            str_at(&v, "parentNodeId"),
            str_at(&v, "rootNodeId").unwrap_or(""),
            str_at(&v, "kind").unwrap_or("turn"),
            str_at(&v, "status").unwrap_or("pending"),
            str_at(&v, "providerThreadId"),
            str_at(&v, "providerTurnId"),
            str_at(&v, "runtimeRequestId"),
            str_at(&v, "checkpointScopeId"),
            str_at(&v, "startedAt"),
            str_at(&v, "completedAt"),
            json,
        ],
    )?;
    Ok(())
}

fn upsert_turn_item(conn: &Connection, item: &impl serde::Serialize) -> Result<(), SinkError> {
    let (v, json) = to_json(item)?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_turn_items (
           turn_item_id, thread_id, run_id, node_id, provider_thread_id, provider_turn_id,
           parent_item_id, ordinal, type, status, updated_at, payload_json, inline_work_output
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)
         ON CONFLICT(turn_item_id) DO UPDATE SET
           run_id = excluded.run_id,
           node_id = excluded.node_id,
           provider_thread_id = excluded.provider_thread_id,
           provider_turn_id = excluded.provider_turn_id,
           parent_item_id = excluded.parent_item_id,
           ordinal = excluded.ordinal,
           type = excluded.type,
           status = excluded.status,
           updated_at = excluded.updated_at,
           payload_json = excluded.payload_json",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "runId"),
            str_at(&v, "nodeId"),
            str_at(&v, "providerThreadId"),
            str_at(&v, "providerTurnId"),
            str_at(&v, "parentItemId"),
            v.get("ordinal").and_then(Value::as_i64).unwrap_or(0),
            str_at(&v, "type").unwrap_or("error"),
            str_at(&v, "status").unwrap_or("completed"),
            str_at(&v, "updatedAt").unwrap_or(""),
            json,
        ],
    )?;
    Ok(())
}

fn upsert_plan(conn: &Connection, plan: &impl serde::Serialize) -> Result<(), SinkError> {
    let (v, json) = to_json(plan)?;
    conn.execute(
        "INSERT INTO orchestration_v2_projection_plans (
           plan_id, thread_id, run_id, node_id, kind, status, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(plan_id) DO UPDATE SET
           status = excluded.status,
           payload_json = excluded.payload_json",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "runId"),
            str_at(&v, "nodeId").unwrap_or(""),
            str_at(&v, "kind").unwrap_or("proposed_plan"),
            str_at(&v, "status").unwrap_or("active"),
            json,
        ],
    )?;
    Ok(())
}

fn upsert_checkpoint_scope(
    conn: &Connection,
    scope: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(scope)?;
    conn.execute(
        "INSERT OR REPLACE INTO orchestration_v2_projection_checkpoint_scopes (
           scope_id, thread_id, run_id, node_id, parent_scope_id, provider_thread_id,
           kind, ordinal_within_parent, advances_app_run_count, created_at, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "runId"),
            str_at(&v, "nodeId").unwrap_or(""),
            str_at(&v, "parentScopeId"),
            str_at(&v, "providerThreadId"),
            str_at(&v, "kind").unwrap_or("root_run"),
            v.get("ordinalWithinParent")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            i64::from(
                v.get("advancesAppRunCount")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            ),
            str_at(&v, "createdAt").unwrap_or(""),
            json,
        ],
    )?;
    Ok(())
}

fn upsert_checkpoint(conn: &Connection, cp: &impl serde::Serialize) -> Result<(), SinkError> {
    let (v, json) = to_json(cp)?;
    conn.execute(
        "INSERT OR REPLACE INTO orchestration_v2_projection_checkpoints (
           checkpoint_id, thread_id, scope_id, run_id, node_id, parent_checkpoint_id,
           ordinal_within_scope, app_run_ordinal, status, captured_at, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "scopeId").unwrap_or(""),
            str_at(&v, "runId"),
            str_at(&v, "nodeId").unwrap_or(""),
            str_at(&v, "parentCheckpointId"),
            v.get("ordinalWithinScope")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            v.get("appRunOrdinal").and_then(Value::as_i64),
            str_at(&v, "status").unwrap_or("ready"),
            str_at(&v, "capturedAt").unwrap_or(""),
            json,
        ],
    )?;
    Ok(())
}

fn upsert_context_handoff(
    conn: &Connection,
    handoff: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(handoff)?;
    conn.execute(
        "INSERT OR REPLACE INTO orchestration_v2_projection_context_handoffs (
           context_handoff_id, thread_id, target_run_id, to_provider_thread_id,
           strategy, status, updated_at, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "threadId").unwrap_or(""),
            str_at(&v, "targetRunId").unwrap_or(""),
            str_at(&v, "toProviderThreadId").unwrap_or(""),
            str_at(&v, "strategy").unwrap_or("handoff"),
            str_at(&v, "status").unwrap_or("pending"),
            str_at(&v, "updatedAt").unwrap_or(""),
            json,
        ],
    )?;
    Ok(())
}

fn upsert_context_transfer(
    conn: &Connection,
    transfer: &impl serde::Serialize,
) -> Result<(), SinkError> {
    let (v, json) = to_json(transfer)?;
    conn.execute(
        "INSERT OR REPLACE INTO orchestration_v2_projection_context_transfers (
           context_transfer_id, source_thread_id, target_thread_id, target_run_id,
           type, status, source_provider, target_provider, updated_at, payload_json,
           source_provider_instance_id, target_provider_instance_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            str_at(&v, "id").unwrap_or(""),
            str_at(&v, "sourceThreadId").unwrap_or(""),
            str_at(&v, "targetThreadId").unwrap_or(""),
            str_at(&v, "targetRunId"),
            str_at(&v, "type").unwrap_or("fork"),
            str_at(&v, "status").unwrap_or("pending"),
            str_at(&v, "sourceProviderInstanceId"),
            str_at(&v, "targetProviderInstanceId"),
            str_at(&v, "updatedAt").unwrap_or(""),
            json,
            str_at(&v, "sourceProviderInstanceId"),
            str_at(&v, "targetProviderInstanceId"),
        ],
    )?;
    Ok(())
}
