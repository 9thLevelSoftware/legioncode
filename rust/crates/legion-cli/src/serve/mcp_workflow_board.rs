//! Scoped MCP board context & brief reader (`workflow_board_read_brief`).

use std::sync::Arc;

use legion_contracts::ids::WorkflowCardId;
use legion_contracts::workflow_board::{WorkflowBoardGetArtifactInput, WorkflowBoardGetCardInput};
use legion_persistence::workflow_board_repo::{
    CardPlanningDetail, StageRunRow, get_card_planning_detail, get_latest_stage_run_for_thread,
    get_stage_run_by_workflow_run_id,
};
use serde_json::{Value, json};

use super::AppState;
use super::mcp_milestones::McpCallerContext;

pub(super) struct BoundStageContext {
    pub(super) stage_run: StageRunRow,
    pub(super) planning: CardPlanningDetail,
}

pub(super) fn list_board_tools(name: &dyn Fn(&str) -> String) -> Vec<Value> {
    vec![
        json!({
            "name": name("workflow_board_read_brief"),
            "description": "Read the active workflow-board stage brief, spec, exploration decisions, challenge receipt, and plan context for this stage thread's bound card.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cardId": { "type": "string", "description": "Optional card ID check; must match the card bound to this stage thread." }
                }
            }
        }),
        json!({
            "name": name("workflow_board_propose"),
            "description": "Propose a bounded spec update, exploration decision/round, challenge concern/synthesis, or plan update for this stage thread's bound card without bypassing human approval gates.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cardId": { "type": "string", "description": "Optional card ID check; must match the card bound to this stage thread." },
                    "stage": { "type": "string", "enum": ["spec", "plan", "execute", "review"] },
                    "kind": { "type": "string", "enum": ["spec", "explore", "challenge", "plan", "execute", "check", "review"] },
                    "expectedRevision": { "type": "integer", "minimum": 1 }
                }
            }
        }),
    ]
}

pub(super) fn resolve_bound_stage_context(
    state: &AppState,
    caller: &McpCallerContext,
    args: &Value,
) -> Result<BoundStageContext, String> {
    if !caller.has_thread_scope {
        return Err("Workflow board MCP tools require an active thread-scoped session.".to_owned());
    }
    let thread_id = caller
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Workflow board MCP tools require a bound thread id in session or headers.".to_owned()
        })?;
    if let Some(sub) = caller.subject.as_deref()
        && let Some(sub_tid) = sub.strip_prefix("thread:").map(str::trim)
        && !sub_tid.is_empty()
        && sub_tid != thread_id
    {
        return Err(format!(
            "Thread scope mismatch: session `{sub}` cannot access thread `{thread_id}`."
        ));
    }

    let conn = state
        .orchestrator
        .sink()
        .db()
        .lock()
        .map_err(|_| "Database lock poisoned.".to_owned())?;

    let mut stage_run = get_latest_stage_run_for_thread(&conn, thread_id)
        .map_err(|err| format!("Failed loading workflow board stage run: {err}"))?;

    if stage_run.is_none() {
        let proj_json: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id = ?1 LIMIT 1",
                rusqlite::params![thread_id],
                |row| row.get(0),
            )
            .ok();
        if let Some(raw) = proj_json
            && let Ok(parsed) = serde_json::from_str::<Value>(&raw)
            && let Some(wf_run_id) = parsed
                .get("workflowRun")
                .and_then(|wf| wf.get("runId"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        {
            stage_run = get_stage_run_by_workflow_run_id(&conn, wf_run_id)
                .map_err(|err| format!("Failed loading stage run for workflow run: {err}"))?;
        }
    }

    let stage_run = stage_run.ok_or_else(|| {
        format!(
            "Thread `{thread_id}` is not an active workflow-board stage thread; board tools are restricted to bound stage runs."
        )
    })?;

    if !matches!(
        stage_run.status.as_str(),
        "pending" | "launching" | "running" | "paused"
    ) {
        return Err(format!(
            "Bound workflow-board stage run `{}` is `{}` and no longer active.",
            stage_run.id, stage_run.status
        ));
    }

    if let Some(requested_card) = args
        .get("cardId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && requested_card != stage_run.card_id
    {
        return Err(format!(
            "Cross-card access refused: thread `{thread_id}` is bound to card `{}`, not `{requested_card}`.",
            stage_run.card_id
        ));
    }

    let planning = get_card_planning_detail(&conn, &stage_run.card_id)
        .map_err(|err| format!("Failed loading card `{}`: {err}", stage_run.card_id))?;

    Ok(BoundStageContext {
        stage_run,
        planning,
    })
}

pub(super) fn handle_read_brief(
    state: &Arc<AppState>,
    caller: &McpCallerContext,
    args: &Value,
) -> Result<Value, String> {
    let ctx = resolve_bound_stage_context(state, caller, args)?;
    let board = state
        .runtime_http
        .workflow_board
        .as_ref()
        .ok_or_else(|| "Workflow board service is unavailable.".to_owned())?;
    let card_id = WorkflowCardId::new(&ctx.stage_run.card_id)
        .map_err(|err| format!("Invalid bound cardId: {err}"))?;

    let detail = board
        .get_card_detail(&WorkflowBoardGetCardInput {
            card_id: card_id.clone(),
            project_id: None,
        })
        .map_err(|err| err.message)?;

    let brief_key = match ctx.stage_run.stage.as_str() {
        "plan" => "plan-brief.md",
        "execute" => "execute-brief.md",
        "review" => "review-brief.md",
        "ship" => "ship-brief.md",
        _ => "spec-brief.md",
    };

    let brief_artifact = board
        .get_artifact(&WorkflowBoardGetArtifactInput {
            card_id,
            key: Some(brief_key.to_owned()),
            kind: None,
        })
        .ok();

    Ok(json!({
        "cardId": ctx.stage_run.card_id,
        "stage": ctx.stage_run.stage,
        "stageRunId": ctx.stage_run.id,
        "revision": detail.summary.revision,
        "briefArtifactKey": brief_artifact.as_ref().map(|a| a.key.as_str()),
        "briefMarkdown": brief_artifact.as_ref().map(|a| a.content.as_str()),
        "card": detail,
    }))
}
