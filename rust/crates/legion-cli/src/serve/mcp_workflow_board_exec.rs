//! Scoped MCP handlers for M3 `execute`, `check`, and `review` proposals.
//! Strictly prevents human-gate bypasses and forces finding `disposition = "unresolved"`.

use std::sync::Arc;

use legion_contracts::workflow_board::normalize_review_finding_severity;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::AppState;
use super::mcp_workflow_board::BoundStageContext;

pub(super) async fn handle_execute_or_review_proposal(
    state: &Arc<AppState>,
    ctx: &BoundStageContext,
    requested_stage: &str,
    kind: &str,
    obj: &Map<String, Value>,
    expected_revision: u64,
) -> Result<Value, String> {
    match (requested_stage, kind) {
        ("execute", "execute") => handle_execute_task_proposal(state, ctx, obj).await,
        ("execute", "check") => {
            handle_execute_check_proposal(state, ctx, obj, expected_revision).await
        }
        ("execute", other) => Err(format!(
            "Kind `{other}` is not valid for `execute` stage proposals; expected `execute` or `check`."
        )),
        ("review", "review") => handle_review_proposal(state, ctx, obj).await,
        ("review", other) => Err(format!(
            "Kind `{other}` is not valid for `review` stage proposals; expected `review`."
        )),
        _ => Err(format!(
            "Unsupported M3 proposal `(stage={requested_stage}, kind={kind})`"
        )),
    }
}

async fn handle_execute_task_proposal(
    state: &Arc<AppState>,
    ctx: &BoundStageContext,
    obj: &Map<String, Value>,
) -> Result<Value, String> {
    let board = state
        .runtime_http
        .workflow_board
        .as_ref()
        .ok_or_else(|| "Workflow board service is unavailable.".to_owned())?;
    let bound_task = ctx.stage_run.task_id.as_deref();
    let requested_task = obj
        .get("taskId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let task_id = match (bound_task, requested_task) {
        (Some(b), Some(r)) if b != r => {
            return Err(format!(
                "Task mismatch: stage run `{}` is bound to task `{b}`, not `{r}`.",
                ctx.stage_run.id
            ));
        }
        (Some(b), _) => b,
        (None, Some(r)) => r,
        (None, None) => {
            return Err("Missing `taskId` for execute task completion proposal.".to_owned());
        }
    };
    let outcome_status = obj
        .get("outcomeStatus")
        .or_else(|| obj.get("status"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("completed");
    if !matches!(outcome_status, "completed" | "failed" | "blocked") {
        return Err(format!(
            "Invalid execute `outcomeStatus` `{outcome_status}`; expected `completed`, `failed`, or `blocked`."
        ));
    }
    let error_message = obj
        .get("errorMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let detail = board
        .complete_task_from_stage(
            &ctx.stage_run.card_id,
            &ctx.stage_run.id,
            task_id,
            outcome_status,
            error_message,
        )
        .await
        .map_err(|err| err.message)?;

    Ok(json!({
        "cardId": detail.summary.card_id,
        "revision": detail.summary.revision,
        "stage": detail.summary.stage,
        "status": detail.summary.status,
        "taskId": task_id,
        "card": detail,
    }))
}

async fn handle_execute_check_proposal(
    state: &Arc<AppState>,
    ctx: &BoundStageContext,
    obj: &Map<String, Value>,
    expected_revision: u64,
) -> Result<Value, String> {
    let board = state
        .runtime_http
        .workflow_board
        .as_ref()
        .ok_or_else(|| "Workflow board service is unavailable.".to_owned())?;
    let check_id = obj
        .get("checkId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Missing `checkId` for execute check proposal.".to_owned())?;
    let detail = board
        .run_planned_check(&ctx.stage_run.card_id, check_id, Some(expected_revision))
        .await
        .map_err(|err| err.message)?;
    let receipt = detail
        .check_receipts
        .iter()
        .find(|r| r.check_id == check_id)
        .cloned();
    Ok(json!({
        "cardId": detail.summary.card_id,
        "revision": detail.summary.revision,
        "checkId": check_id,
        "receipt": receipt,
        "card": detail,
    }))
}

async fn handle_review_proposal(
    state: &Arc<AppState>,
    ctx: &BoundStageContext,
    obj: &Map<String, Value>,
) -> Result<Value, String> {
    let board = state
        .runtime_http
        .workflow_board
        .as_ref()
        .ok_or_else(|| "Workflow board service is unavailable.".to_owned())?;
    let mut verdict = obj
        .get("verdict")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "Missing `verdict` for review proposal (`pass`, `findings`, or `failed`).".to_owned()
        })?
        .to_owned();
    if !matches!(verdict.as_str(), "pass" | "findings" | "failed") {
        return Err(format!(
            "Invalid review verdict `{verdict}`; expected `pass`, `findings`, or `failed`."
        ));
    }
    let summary = obj
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let parsed_findings = parse_mcp_review_findings(obj)?;
    if verdict == "pass" && !parsed_findings.is_empty() {
        "findings".clone_into(&mut verdict);
    }

    let detail = board
        .record_review_from_stage(
            &ctx.stage_run.card_id,
            &ctx.stage_run.id,
            &verdict,
            summary,
            &parsed_findings,
        )
        .await
        .map_err(|err| err.message)?;

    Ok(json!({
        "cardId": detail.summary.card_id,
        "revision": detail.summary.revision,
        "stage": detail.summary.stage,
        "verdict": verdict,
        "findingsCount": parsed_findings.len(),
        "card": detail,
    }))
}

type McpFindingTuple = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
);

fn parse_mcp_review_findings(obj: &Map<String, Value>) -> Result<Vec<McpFindingTuple>, String> {
    let mut parsed_findings = Vec::new();
    let Some(arr) = obj.get("findings").and_then(Value::as_array) else {
        return Ok(parsed_findings);
    };
    for (idx, item) in arr.iter().enumerate() {
        let f_obj = item
            .as_object()
            .ok_or_else(|| format!("Review finding at index {idx} must be a JSON object."))?;
        if let Some(disp) = f_obj.get("disposition").and_then(Value::as_str)
            && disp.trim() != "unresolved"
        {
            return Err(
                "Agents cannot set finding `disposition` to resolved/accepted/deferred; only `unresolved` is permitted via MCP."
                    .to_owned(),
            );
        }
        for forbidden in [
            "resolvedBy",
            "resolved_by",
            "resolvedAt",
            "resolved_at",
            "resolutionNote",
            "resolution_note",
        ] {
            if f_obj.contains_key(forbidden) {
                return Err(format!(
                    "Forbidden resolution field `{forbidden}` inside review finding; only humans can resolve findings."
                ));
            }
        }
        let id = f_obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(|| format!("fnd-{}", Uuid::new_v4().simple()), str::to_owned);
        let severity =
            normalize_review_finding_severity(f_obj.get("severity").and_then(Value::as_str))
                .to_owned();
        let criterion_id = f_obj
            .get("criterionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let file = f_obj
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let line = f_obj.get("line").and_then(Value::as_i64);
        let message = f_obj
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("Review finding `{id}` requires a non-empty `message`."))?
            .to_owned();
        parsed_findings.push((id, severity, criterion_id, file, line, message));
    }
    Ok(parsed_findings)
}
