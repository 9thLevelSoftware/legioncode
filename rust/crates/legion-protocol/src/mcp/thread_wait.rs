//! `legion_thread_wait` / `t3_thread_wait` parent wake and live-request gating
//! (`#40604` & `#40633`, `apps/server/src/mcp/OrchestratorMcpService.ts`).

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use legion_contracts::ids::{NodeId, RunId, RuntimeRequestId, ThreadId};
use legion_contracts::orchestration::{RunStatus, Subagent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical wake hint returned to parent agents polling a still-running delegated child.
pub const ORCHESTRATOR_MCP_RUNNING_TASK_NEXT: &str =
    "Keep polling with wait or let task finish asynchronously; reply cleanly once finished.";

/// Delivery channel supported by a pending runtime request (`RuntimeRequest.responseCapability`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseCapabilityKind {
    /// Interactive prompt delivered live to an active provider turn; parent wait stops polling.
    Live,
    /// Deferred prompt answerable only via a follow-up user message.
    Message,
    /// Historical request that cannot be resumed.
    NotResumable,
    /// Fallback when wire shape is missing or unrecognized.
    Unknown,
}

impl ResponseCapabilityKind {
    /// Parses either `"live"` or `{ "type": "live" }` wire encodings.
    pub fn from_wire_value(value: &Value) -> Self {
        let raw = value
            .as_str()
            .or_else(|| value.get("type").and_then(Value::as_str));
        match raw {
            Some("live") => Self::Live,
            Some("message") => Self::Message,
            Some("not_resumable") => Self::NotResumable,
            _ => Self::Unknown,
        }
    }
}

/// Minimal runtime request projection evaluated during `thread_wait` / `delegate_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingChildRequest {
    /// Request identifier.
    pub id: RuntimeRequestId,
    /// Owning execution node identifier, if known.
    pub node_id: Option<NodeId>,
    /// Wire kind (`"user_input"`, `"command"`, `"file-change"`, `"permission"`, etc.).
    pub kind: String,
    /// Request lifecycle status (`"pending"`, `"resolved"`, `"expired"`, `"cancelled"`).
    pub status: String,
    /// Interactive capability of the request.
    pub response_capability: ResponseCapabilityKind,
}

/// Parent-side subagent record checked to decide whether a waited thread still owes work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentSubagentRecord {
    /// Subagent task identifier.
    pub id: NodeId,
    /// Parent thread identifier.
    pub thread_id: ThreadId,
    /// Origin string (`"app_owned"` vs `"provider_native"`).
    pub origin: String,
    /// Child thread ID assigned to this task.
    pub child_thread_id: Option<ThreadId>,
    /// Completion delivery state (`"pending"`, `"delivered"`, `"acknowledged"`, `"disposed"`).
    pub completion_delivery_state: Option<String>,
    /// Final result string once populated; untagged first runs belong to the task only while `None`.
    pub result: Option<String>,
}

impl ParentSubagentRecord {
    /// Extracts a [`ParentSubagentRecord`] from a [`Subagent`] domain model.
    pub fn from_subagent(subagent: &Subagent) -> Self {
        let completion_delivery_state = subagent
            .completion_delivery
            .as_ref()
            .and_then(|val| val.get("state"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Self {
            id: subagent.id.clone(),
            thread_id: subagent.thread_id.clone(),
            origin: subagent.origin.clone(),
            child_thread_id: subagent.child_thread_id.clone(),
            completion_delivery_state,
            result: subagent.result.clone(),
        }
    }
}

/// Input evaluated by [`evaluate_thread_wait_wake`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadWaitWakeInput {
    /// Parent thread calling `thread_wait`, if scoped to a parent thread.
    pub parent_thread_id: Option<ThreadId>,
    /// Child thread being waited on.
    pub target_thread_id: ThreadId,
    /// Optional `thread.forkedFrom` wire JSON (`{ "type": "node", "nodeId": "..." }`).
    pub target_forked_from: Option<Value>,
    /// `run.delegatedTaskId` on the waited run, or `None` for first untagged runs.
    pub run_delegated_task_id: Option<NodeId>,
    /// Run status reported by `thread_wait`, or `None` when idle.
    pub run_status: Option<RunStatus>,
    /// Whether the bounded wait timed out before settling.
    pub timed_out: bool,
    /// Parent subagent tasks loaded for the resolved `task_id`.
    pub parent_subagents: Vec<ParentSubagentRecord>,
    /// Child thread runtime requests inspected for live questions or approvals.
    pub child_requests: Vec<PendingChildRequest>,
}

/// Outcome of evaluating whether `thread_wait` should ask the parent agent to keep polling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadWaitWakeDecision {
    /// Resolved task ID from `run.delegatedTaskId` or `target.forkedFrom.nodeId`.
    pub resolved_task_id: Option<NodeId>,
    /// `true` when the wait timed out on a non-waiting active app-owned task not yet disposed.
    pub owned_task: bool,
    /// `true` when the child currently has any pending `live` interactive request.
    pub has_live_pending_request: bool,
    /// `true` only when `owned_task && !has_live_pending_request`.
    pub wakes_caller: bool,
    /// Next-action prompt hint included when `wakes_caller` is `true`.
    pub next_hint: Option<&'static str>,
}

/// Extracts `nodeId` when `forked_from` has `{ "type": "node", "nodeId": "<id>" }`.
pub fn extract_forked_node_id(forked_from: Option<&Value>) -> Option<NodeId> {
    let val = forked_from?;
    if val.get("type").and_then(Value::as_str) != Some("node") {
        return None;
    }
    let raw = val.get("nodeId").and_then(Value::as_str)?;
    NodeId::new(raw).ok()
}

/// Returns `true` when `req` is both `"pending"` and capable of receiving a `"live"` answer.
pub fn is_live_pending_request(req: &PendingChildRequest) -> bool {
    req.status == "pending" && req.response_capability == ResponseCapabilityKind::Live
}

/// Returns `true` when the child has a pending `"live"` `user_input` prompt (`#40604`).
pub fn has_live_pending_user_input(requests: &[PendingChildRequest]) -> bool {
    requests
        .iter()
        .any(|req| req.kind == "user_input" && is_live_pending_request(req))
}

/// Returns the oldest pending live approval request ID among `task_run_ids` (`#40604`).
pub fn oldest_live_pending_approval<'a, S1: BuildHasher, S2: BuildHasher>(
    requests: &'a [PendingChildRequest],
    node_to_run: &HashMap<NodeId, RunId, S1>,
    task_run_ids: &HashSet<RunId, S2>,
) -> Option<&'a PendingChildRequest> {
    requests.iter().find(|req| {
        is_live_pending_request(req)
            && is_approval_request_kind(&req.kind)
            && req
                .node_id
                .as_ref()
                .and_then(|nid| node_to_run.get(nid))
                .is_some_and(|rid| task_run_ids.contains(rid))
    })
}

fn is_approval_request_kind(kind: &str) -> bool {
    matches!(kind, "command" | "file-read" | "file-change" | "permission")
}

/// Evaluates whether a bounded `legion_thread_wait` / `t3_thread_wait` timeout should keep a
/// parent agent polling (`#40604` & `#40633`).
pub fn evaluate_thread_wait_wake(input: &ThreadWaitWakeInput) -> ThreadWaitWakeDecision {
    let resolved_task_id = input
        .run_delegated_task_id
        .clone()
        .or_else(|| extract_forked_node_id(input.target_forked_from.as_ref()));
    let is_waiting = matches!(input.run_status, Some(RunStatus::Waiting));

    let owned_task = input.timed_out
        && !is_waiting
        && input.parent_thread_id.is_some()
        && resolved_task_id.is_some()
        && input.parent_subagents.iter().any(|task| {
            let Some(parent_tid) = input.parent_thread_id.as_ref() else {
                return false;
            };
            let Some(task_id) = resolved_task_id.as_ref() else {
                return false;
            };
            &task.id == task_id
                && task.origin == "app_owned"
                && &task.thread_id == parent_tid
                && task.child_thread_id.as_ref() == Some(&input.target_thread_id)
                && task.completion_delivery_state.as_deref() != Some("disposed")
                && (input.run_delegated_task_id.is_some() || task.result.is_none())
        });

    let has_live_pending_request = input.child_requests.iter().any(is_live_pending_request);
    let wakes_caller = owned_task && !has_live_pending_request;

    ThreadWaitWakeDecision {
        resolved_task_id,
        owned_task,
        has_live_pending_request,
        wakes_caller,
        next_hint: wakes_caller.then_some(ORCHESTRATOR_MCP_RUNNING_TASK_NEXT),
    }
}
