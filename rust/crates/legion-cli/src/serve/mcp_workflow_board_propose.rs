//! Scoped MCP board proposal tool (`workflow_board_propose`) without approval bypasses.

use std::sync::Arc;

use legion_contracts::ids::WorkflowCardId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardChallengeStepInput,
    WorkflowBoardExploreStepInput, WorkflowBoardIntakeGoal, WorkflowBoardUpdatePlanInput,
    WorkflowBoardUpdateSpecInput,
};
use serde_json::{Map, Value};

use super::AppState;
use super::mcp_milestones::McpCallerContext;
use super::mcp_workflow_board::{BoundStageContext, resolve_bound_stage_context};
use super::mcp_workflow_board_exec::handle_execute_or_review_proposal;

const FORBIDDEN_TOP_LEVEL_KEYS: &[&str] = &[
    "approve",
    "approved",
    "approvespec",
    "approve_spec",
    "approveplan",
    "approve_plan",
    "approvedelivery",
    "approve_delivery",
    "returnforrevision",
    "return_for_revision",
    "resolvefinding",
    "resolve_finding",
    "resolvecontractviolation",
    "resolve_contract_violation",
    "reconcileinterruptedaction",
    "reconcile_interrupted_action",
    "proposeamendment",
    "propose_amendment",
    "recordmanualacceptance",
    "record_manual_acceptance",
    "deliveryapproved",
    "delivery_approved",
    "resolvedby",
    "resolved_by",
    "resolvedat",
    "resolved_at",
    "resolutionnote",
    "resolution_note",
];

pub(super) async fn handle_propose(
    state: &Arc<AppState>,
    caller: &McpCallerContext,
    args: &Value,
) -> Result<Value, String> {
    let ctx = resolve_bound_stage_context(state, caller, args)?;
    let obj = args
        .as_object()
        .ok_or_else(|| "MCP workflow_board_propose arguments must be a JSON object.".to_owned())?;

    guard_no_human_gate_bypass(obj)?;

    let bound_stage = ctx.stage_run.stage.as_str();
    let requested_stage = obj
        .get("stage")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(bound_stage);

    if requested_stage == "ship" {
        return Err(
            "Stage `ship` is not writable via workflow_board_propose; final delivery requires human authorization."
                .to_owned(),
        );
    }
    if !matches!(requested_stage, "spec" | "plan" | "execute" | "review") {
        return Err(format!(
            "Unsupported proposal stage `{requested_stage}`; expected `spec`, `plan`, `execute`, or `review`."
        ));
    }
    if requested_stage != bound_stage {
        return Err(format!(
            "Stage mismatch: active stage run `{}` is in stage `{bound_stage}`, not `{requested_stage}`.",
            ctx.stage_run.id
        ));
    }

    let expected_revision = obj
        .get("expectedRevision")
        .and_then(Value::as_u64)
        .filter(|rev| *rev > 0)
        .unwrap_or_else(|| u64::try_from(ctx.planning.card_detail.card.revision).unwrap_or(1));
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(requested_stage);

    match (requested_stage, kind) {
        ("spec", "spec") => handle_spec_proposal(state, &ctx, obj, expected_revision).await,
        ("spec", "explore" | "challenge") => {
            handle_explore_or_challenge_proposal(state, &ctx, obj, expected_revision, kind).await
        }
        ("spec", other) => Err(format!(
            "Kind `{other}` is not valid for `spec` stage proposals; expected `spec`, `explore`, or `challenge`."
        )),
        ("plan", "plan") => handle_plan_proposal(state, &ctx, obj, expected_revision).await,
        ("plan", "explore" | "challenge") => Err(format!(
            "Kind `{kind}` is only valid during `spec` stage; `plan` stage proposals only accept `plan`."
        )),
        ("plan", other) => Err(format!(
            "Unsupported proposal kind `{other}` for `plan` stage; expected `plan`."
        )),
        ("execute" | "review", _) => {
            handle_execute_or_review_proposal(
                state,
                &ctx,
                requested_stage,
                kind,
                obj,
                expected_revision,
            )
            .await
        }
        _ => Err("Unsupported stage.".to_owned()),
    }
}

fn guard_no_human_gate_bypass(obj: &Map<String, Value>) -> Result<(), String> {
    for key in obj.keys() {
        let lower = key.trim().to_ascii_lowercase();
        if FORBIDDEN_TOP_LEVEL_KEYS.contains(&lower.as_str()) {
            return Err(format!(
                "Forbidden human-approval or stage-return parameter `{key}` in workflow_board_propose."
            ));
        }
    }
    if let Some(disp) = obj
        .get("disposition")
        .and_then(Value::as_str)
        .map(str::trim)
        && disp != "unresolved"
    {
        return Err(
            "Agents cannot resolve findings via MCP; `disposition` must be `unresolved`."
                .to_owned(),
        );
    }
    if let Some(action) = obj.get("action").and_then(Value::as_str).map(str::trim) {
        let lower = action.to_ascii_lowercase();
        if lower == "complete"
            || lower.starts_with("approve")
            || lower.starts_with("return")
            || lower.starts_with("resolve")
            || lower.starts_with("manual")
            || lower.starts_with("ship")
            || lower.starts_with("reconcile")
            || lower.starts_with("amend")
        {
            return Err(format!(
                "Action `{action}` requires direct human authorization and cannot be called via MCP."
            ));
        }
    }
    Ok(())
}

async fn handle_spec_proposal(
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
    let card_id = WorkflowCardId::new(&ctx.stage_run.card_id)
        .map_err(|err| format!("Invalid cardId: {err}"))?;
    let markdown = obj
        .get("markdown")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let brief = obj
        .get("brief")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| markdown.clone());

    let acceptance_criteria = if let Some(ac_val) = obj.get("acceptanceCriteria") {
        Some(
            serde_json::from_value::<Vec<WorkflowBoardAcceptanceCriterion>>(ac_val.clone())
                .map_err(|err| format!("Invalid acceptanceCriteria: {err}"))?,
        )
    } else if let Some(ref md) = markdown {
        let extracted = extract_ac_bullets_from_markdown(md);
        (!extracted.is_empty()).then_some(extracted)
    } else {
        None
    };

    let intake_goal = match obj.get("intakeGoal") {
        Some(val) if !val.is_null() => Some(
            serde_json::from_value::<WorkflowBoardIntakeGoal>(val.clone())
                .map_err(|err| format!("Invalid intakeGoal: {err}"))?,
        ),
        _ => None,
    };
    let input_paths = match obj.get("inputPaths") {
        Some(val) if !val.is_null() => Some(
            serde_json::from_value::<Vec<String>>(val.clone())
                .map_err(|err| format!("Invalid inputPaths: {err}"))?,
        ),
        _ => None,
    };
    let constraints = match obj.get("constraints") {
        Some(val) if !val.is_null() => Some(
            serde_json::from_value::<Vec<String>>(val.clone())
                .map_err(|err| format!("Invalid constraints: {err}"))?,
        ),
        _ => None,
    };
    let source_refs = match obj.get("sourceRefs") {
        Some(val) if !val.is_null() => Some(
            serde_json::from_value::<Vec<String>>(val.clone())
                .map_err(|err| format!("Invalid sourceRefs: {err}"))?,
        ),
        _ => None,
    };

    let input = WorkflowBoardUpdateSpecInput {
        card_id,
        expected_revision,
        title: obj.get("title").and_then(Value::as_str).map(str::to_owned),
        brief,
        intake_goal,
        input_paths,
        acceptance_criteria,
        constraints,
        source_refs,
        wireframes_index: PatchValue::Absent,
    };
    let result = board.update_spec(&input).await.map_err(|err| err.message)?;
    serde_json::to_value(result).map_err(|err| format!("Failed serializing spec result: {err}"))
}

fn build_bound_payload(
    ctx: &BoundStageContext,
    obj: &Map<String, Value>,
    expected_revision: u64,
) -> Value {
    let mut payload = obj.clone();
    payload.remove("stage");
    payload.remove("kind");
    payload.insert(
        "cardId".to_owned(),
        Value::String(ctx.stage_run.card_id.clone()),
    );
    payload.insert(
        "expectedRevision".to_owned(),
        Value::from(expected_revision),
    );
    Value::Object(payload)
}

async fn handle_explore_or_challenge_proposal(
    state: &Arc<AppState>,
    ctx: &BoundStageContext,
    obj: &Map<String, Value>,
    expected_revision: u64,
    kind: &str,
) -> Result<Value, String> {
    let board = state
        .runtime_http
        .workflow_board
        .as_ref()
        .ok_or_else(|| "Workflow board service is unavailable.".to_owned())?;
    let payload_val = build_bound_payload(ctx, obj, expected_revision);

    match kind {
        "explore" => {
            let input: WorkflowBoardExploreStepInput = serde_json::from_value(payload_val)
                .map_err(|err| format!("Invalid explore payload: {err}"))?;
            let result = board
                .explore_step(&input)
                .await
                .map_err(|err| err.message)?;
            serde_json::to_value(result)
                .map_err(|err| format!("Failed serializing explore result: {err}"))
        }
        "challenge" => {
            let input: WorkflowBoardChallengeStepInput = serde_json::from_value(payload_val)
                .map_err(|err| format!("Invalid challenge payload: {err}"))?;
            let result = board
                .challenge_step(&input)
                .await
                .map_err(|err| err.message)?;
            serde_json::to_value(result)
                .map_err(|err| format!("Failed serializing challenge result: {err}"))
        }
        other => Err(format!("Unsupported proposal kind `{other}`.")),
    }
}

async fn handle_plan_proposal(
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
    let payload_val = build_bound_payload(ctx, obj, expected_revision);
    let input: WorkflowBoardUpdatePlanInput = serde_json::from_value(payload_val)
        .map_err(|err| format!("Invalid updatePlan payload: {err}"))?;
    let result = board.update_plan(&input).await.map_err(|err| err.message)?;
    serde_json::to_value(result).map_err(|err| format!("Failed serializing plan result: {err}"))
}

fn extract_ac_bullets_from_markdown(md: &str) -> Vec<WorkflowBoardAcceptanceCriterion> {
    let mut out = Vec::new();
    for line in md.lines() {
        let trimmed = line.trim();
        let body = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .map(str::trim);
        let Some(body) = body else {
            continue;
        };
        let Some((id_part, text_part)) = body.split_once(':') else {
            continue;
        };
        let id = id_part.trim().trim_matches('`');
        let text = text_part.trim();
        if id.starts_with("AC-") && !text.is_empty() {
            out.push(WorkflowBoardAcceptanceCriterion {
                id: id.to_owned(),
                text: text.to_owned(),
            });
        }
    }
    out
}
