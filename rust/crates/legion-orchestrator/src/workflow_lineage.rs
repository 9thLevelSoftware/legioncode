//! Workflow-step thread lineage models, fork stripping, and project thread list filtering
//! (`#38882`, `Orchestrator.ts`, `ProjectionStore.ts`, `ThreadManagementService.ts`).

use legion_contracts::ids::EnvironmentId;
use legion_contracts::orchestration::AppThread;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::projections::ProjectionError;

/// Provenance linking a thread to the durable workflow run step it executes (`#38882`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowOrigin {
    /// Environment running the workflow coordinator.
    pub environment_id: EnvironmentId,
    /// Workflow run ID.
    pub run_id: String,
    /// Workflow definition/template name.
    pub workflow: String,
    /// Agent step name within `workflow`.
    pub step: String,
}

/// Project-thread listing options mirroring `ThreadManagementService.listProjectThreads` (`#38882`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectThreadListOptions {
    /// Filter to a single project ID when `Some`; list all projects when `None`.
    pub project_id: Option<String>,
    /// Include delegated subagents, remote delegated threads, and workflow step threads; side
    /// chats remain hidden either way.
    pub include_subagents: bool,
}

/// Active thread list entry returned by [`list_project_threads`] (`#38882`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedThreadSummary {
    /// Thread ID.
    pub id: String,
    /// Owning project ID.
    pub project_id: String,
    /// Thread display title.
    pub title: String,
    /// Last updated ISO-8601 timestamp.
    pub updated_at: String,
    /// Workflow step provenance when attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<WorkflowOrigin>,
    /// Full wire thread JSON for callers needing extra metadata fields.
    pub thread: Value,
}

/// Returns `true` when `thread_json` has `lineage.sideChat == true`.
#[must_use]
pub fn is_side_chat_json(thread_json: &Value) -> bool {
    thread_json
        .get("lineage")
        .and_then(|lineage| lineage.get("sideChat"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Returns `true` when `lineage` / `workflowRun` belongs to a workflow step (`#38882`).
#[must_use]
pub fn is_workflow_step_lineage(lineage: Option<&Value>, workflow_run: Option<&Value>) -> bool {
    lineage
        .and_then(|l| l.get("workflowStep"))
        .and_then(Value::as_bool)
        == Some(true)
        || workflow_run.is_some_and(|val| !val.is_null())
}

/// Returns `true` when `thread_json` is a regular listed thread (`#38882`): excludes subagents,
/// side chats, delegated remote children, AND workflow-step threads.
#[must_use]
pub fn is_listed_thread_json(thread_json: &Value) -> bool {
    let lineage = thread_json.get("lineage");
    let relationship = lineage
        .and_then(|l| l.get("relationshipToParent"))
        .and_then(Value::as_str);
    let delegated_from = thread_json
        .get("delegatedFrom")
        .is_some_and(|v| !v.is_null());
    let workflow_run = thread_json.get("workflowRun");

    relationship != Some("subagent")
        && !is_side_chat_json(thread_json)
        && !is_workflow_step_lineage(lineage, workflow_run)
        && !delegated_from
}

/// Typed `AppThread` helper matching `isListedThreadLineage` (`#38882`).
#[must_use]
pub fn is_listed_thread_lineage(
    thread: &AppThread,
    workflow_step_flag: bool,
    workflow_run: Option<&WorkflowOrigin>,
) -> bool {
    thread.lineage.relationship_to_parent.as_deref() != Some("subagent")
        && thread.lineage.side_chat != Some(true)
        && !workflow_step_flag
        && workflow_run.is_none()
        && thread.delegated_from.is_none()
}

/// Strips `delegatedFrom`, `workflowRun`, and `lineage.workflowStep` when forking a thread (`#38882`:
/// a fork of a workflow step or delegated child is an independent conversation).
pub fn strip_provenance_for_fork(thread_json: &mut Value) {
    if let Some(obj) = thread_json.as_object_mut() {
        obj.remove("delegatedFrom");
        obj.remove("workflowRun");
        if let Some(Value::Object(lineage)) = obj.get_mut("lineage") {
            lineage.remove("workflowStep");
        }
    }
}

/// Queries active non-deleted, non-archived threads honoring `#38882` workflow-step visibility.
pub fn list_project_threads(
    conn: &Connection,
    options: &ProjectThreadListOptions,
) -> Result<Vec<ListedThreadSummary>, ProjectionError> {
    let raw_rows: Vec<String> = if let Some(pid) = options.project_id.as_deref() {
        let mut stmt = conn.prepare(
            "SELECT payload_json
             FROM orchestration_v2_projection_threads
             WHERE deleted_at IS NULL
               AND archived_at IS NULL
               AND project_id = ?1
             ORDER BY updated_at DESC, thread_id DESC",
        )?;
        stmt.query_map(params![pid], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let mut stmt = conn.prepare(
            "SELECT payload_json
             FROM orchestration_v2_projection_threads
             WHERE deleted_at IS NULL
               AND archived_at IS NULL
             ORDER BY updated_at DESC, thread_id DESC",
        )?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut out = Vec::new();
    for raw in raw_rows {
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|err| ProjectionError::Codec(format!("thread json decode: {err}")))?;
        let keep = if options.include_subagents {
            !is_side_chat_json(&parsed)
        } else {
            is_listed_thread_json(&parsed)
        };
        if !keep {
            continue;
        }
        let workflow_run = parsed
            .get("workflowRun")
            .filter(|v| !v.is_null())
            .cloned()
            .map(serde_json::from_value::<WorkflowOrigin>)
            .transpose()
            .map_err(|err| ProjectionError::Codec(format!("workflowRun decode: {err}")))?;
        out.push(ListedThreadSummary {
            id: parsed
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            project_id: parsed
                .get("projectId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            title: parsed
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            updated_at: parsed
                .get("updatedAt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            workflow_run,
            thread: parsed,
        });
    }
    Ok(out)
}
