//! Wake condition evaluation when `legion_thread_wait` times out (`#40604`, `#40633`).

use legion_contracts::ids::{NodeId, RuntimeRequestId, ThreadId};
use legion_contracts::orchestration::{RunStatus, Subagent};
use legion_orchestrator::projections::query_wire_thread_detail;
use legion_protocol::mcp::{
    ParentSubagentRecord, PendingChildRequest, ResponseCapabilityKind, ThreadWaitWakeDecision,
    ThreadWaitWakeInput, evaluate_thread_wait_wake,
};
use rusqlite::Connection;
use serde_json::Value;

pub(in crate::serve) fn evaluate_wait_wake_for_thread(
    conn: &Connection,
    parent_thread_id: Option<&str>,
    target_thread_id: &str,
    run_id: Option<&str>,
    run_status_str: &str,
) -> Option<ThreadWaitWakeDecision> {
    let target_tid = ThreadId::new(target_thread_id).ok()?;
    let target_detail = query_wire_thread_detail(conn, target_thread_id).ok();
    let target_forked_from = target_detail
        .as_ref()
        .and_then(|d| d.projection.thread.get("forkedFrom").cloned());
    let run_delegated_task_id = target_detail.as_ref().and_then(|d| {
        run_id
            .and_then(|rid| {
                d.projection
                    .runs
                    .iter()
                    .find(|r| r.get("id").and_then(Value::as_str) == Some(rid))
            })
            .or_else(|| d.projection.runs.last())
            .and_then(|r| r.get("delegatedTaskId").and_then(Value::as_str))
            .and_then(|raw| NodeId::new(raw).ok())
    });
    let parent_tid = parent_thread_id.and_then(|raw| ThreadId::new(raw).ok());
    let parent_subagents = parent_tid
        .as_ref()
        .and_then(|pid| query_wire_thread_detail(conn, pid.as_str()).ok())
        .map(|d| {
            d.projection
                .subagents
                .into_iter()
                .filter_map(|v| serde_json::from_value::<Subagent>(v).ok())
                .map(|s| ParentSubagentRecord::from_subagent(&s))
                .collect()
        })
        .unwrap_or_default();
    let child_requests = target_detail
        .map(|d| {
            d.projection
                .runtime_requests
                .into_iter()
                .filter_map(|req| {
                    let id = req
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|s| RuntimeRequestId::new(s).ok())?;
                    let kind = req
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned();
                    let status = req
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending")
                        .to_owned();
                    let node_id = req
                        .get("nodeId")
                        .and_then(Value::as_str)
                        .and_then(|s| NodeId::new(s).ok());
                    let response_capability = req.get("responseCapability").map_or(
                        ResponseCapabilityKind::Unknown,
                        ResponseCapabilityKind::from_wire_value,
                    );
                    Some(PendingChildRequest {
                        id,
                        node_id,
                        kind,
                        status,
                        response_capability,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let run_status =
        serde_json::from_value::<RunStatus>(Value::String(run_status_str.to_owned())).ok();
    Some(evaluate_thread_wait_wake(&ThreadWaitWakeInput {
        parent_thread_id: parent_tid,
        target_thread_id: target_tid,
        target_forked_from,
        run_delegated_task_id,
        run_status,
        timed_out: true,
        parent_subagents,
        child_requests,
    }))
}

pub(super) fn clamp_limit(args: &Value, key: &str, default_val: u64, max_val: u64) -> usize {
    usize::try_from(
        args.get(key)
            .and_then(Value::as_u64)
            .unwrap_or(default_val)
            .clamp(1, max_val),
    )
    .unwrap_or(usize::try_from(default_val).unwrap_or(100))
}

pub(super) fn insert_optional_json_field(target: &mut Value, key: &str, raw: Option<&str>) {
    if let Some(obj) = target.as_object_mut()
        && let Some(val) = raw.and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        obj.insert(key.to_owned(), val);
    }
}
