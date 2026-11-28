#![forbid(unsafe_code)]
//! Full M2 planning detail hydration (`CardPlanningDetail` -> `WorkflowCardDetail`).

use std::collections::HashSet;

use legion_contracts::ids::ProjectId;
use legion_contracts::workflow_board::{
    WorkflowBoardChallengeConcern, WorkflowBoardChallengeManualFallback,
    WorkflowBoardChallengeReceipt, WorkflowBoardExplorationDecision,
    WorkflowBoardExplorationDecisionStatus, WorkflowBoardExplorationState,
    WorkflowBoardSpecDocument, WorkflowCardDetail,
};
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;
use legion_workflows::sha256_prefixed;
use serde_json::Value;

use crate::workflow_board_hydration::{
    build_card_summary, to_approval_receipt, to_revision_entry, to_stage_run_link,
};
use crate::workflow_board_hydration_plan::hydrate_plan_document;
use crate::workflow_board_render::parse_stored_spec_fields;
use crate::workflow_board_types::{
    internal, parse_enum, parse_opt_enum, parse_opt_model_selection, parse_workspace_strategy,
};

pub(crate) fn to_card_detail_from_planning(
    planning: &CardPlanningDetail,
    artifact_keys: Vec<String>,
) -> Result<WorkflowCardDetail, legion_contracts::workflow_board::WorkflowBoardRequestError> {
    let detail = &planning.card_detail;
    let card = &detail.card;
    let project_id = ProjectId::new(&card.project_id).map_err(|e| internal(e.to_string()))?;
    let spec_fields = parse_stored_spec_fields(&card.spec_json);

    let spec_approval_row = detail
        .approvals
        .iter()
        .find(|a| a.gate == "spec" && a.status == "valid")
        .or_else(|| detail.approvals.iter().find(|a| a.gate == "spec"));
    let spec_approval = spec_approval_row.map(to_approval_receipt).transpose()?;

    let plan_approval_row = detail
        .approvals
        .iter()
        .find(|a| a.gate == "plan" && a.status == "valid")
        .or_else(|| detail.approvals.iter().find(|a| a.gate == "plan"));
    let plan_approval = plan_approval_row.map(to_approval_receipt).transpose()?;

    let summary = build_card_summary(
        detail,
        &project_id,
        spec_approval.as_ref(),
        plan_approval.as_ref(),
    )?;
    let stage_runs = detail
        .stage_runs
        .iter()
        .map(|row| to_stage_run_link(row, &project_id))
        .collect::<Result<Vec<_>, _>>()?;
    let revisions = detail
        .revisions
        .iter()
        .map(to_revision_entry)
        .collect::<Result<Vec<_>, _>>()?;

    let exploration = hydrate_exploration(planning, card.status.as_str())?;
    let challenge_receipt = hydrate_challenge_receipt(planning)?;
    let plan = hydrate_plan_document(planning)?;
    let unpacked = unpack_plan_fields(plan.as_ref());

    Ok(WorkflowCardDetail {
        summary,
        workspace_strategy: parse_workspace_strategy(&card.workspace_strategy_json)?,
        model_selection: parse_opt_model_selection(card.model_selection_json.as_deref())?,
        runtime_mode: parse_opt_enum(card.runtime_mode.as_deref(), "runtime_mode")?,
        interaction_mode: parse_opt_enum(card.interaction_mode.as_deref(), "interaction_mode")?,
        spec: WorkflowBoardSpecDocument {
            title: card.title.clone(),
            brief: card.brief.clone(),
            source_type: parse_enum(&card.source_type, "source_type")?,
            assistance_mode: parse_enum(&card.assistance_mode, "assistance_mode")?,
            acceptance_criteria: spec_fields.acceptance_criteria,
            constraints: spec_fields.constraints,
            source_refs: spec_fields.source_refs,
            wireframes_index: spec_fields.wireframes_index.clone(),
            spec_digest: card.spec_digest.clone(),
            updated_at: card.updated_at.clone(),
            intake_goal: parse_opt_enum(card.intake_goal.as_deref(), "intake_goal")?,
            brief_path: card.brief_path.clone(),
            brief_digest: card.brief_digest.clone(),
            source_digest: spec_fields.source_digest,
            source_input_digests: spec_fields.source_input_digests,
            intake_analysis: spec_fields.intake_analysis,
            wireframes_digest: card.wireframes_digest.clone(),
        },
        spec_approval,
        stage_runs,
        revisions: revisions.clone(),
        artifact_keys,
        wireframes_index: spec_fields.wireframes_index,
        wireframes_digest: card.wireframes_digest.clone(),
        exploration,
        challenge_receipt,
        max_challenge_concerns: Some(3),
        plan,
        plan_approval,
        lenses: unpacked.0,
        alternatives: unpacked.1,
        alternatives_revision_count: unpacked.2,
        selected_alternative_id: unpacked.3,
        selection_rationale: unpacked.4,
        strategy: unpacked.5,
        strategy_rationale: unpacked.6,
        strategy_metadata: unpacked.7,
        granularity: unpacked.8,
        tasks: unpacked.9,
        checks: unpacked.10,
        ac_mappings: unpacked.11,
        history: revisions,
        task_executions: Vec::new(),
        file_contract_reports: Vec::new(),
        check_receipts: Vec::new(),
        review_report: None,
        ac_observations: Vec::new(),
        ship_readiness: None,
        delivery_receipt: None,
        recovery_state: None,
        amendments: Vec::new(),
        cli_import: card
            .import_provenance_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
    })
}

type UnpackedPlanFields = (
    Vec<legion_contracts::workflow_board::WorkflowBoardPlanLensEntry>,
    Vec<legion_contracts::workflow_board::WorkflowBoardPlanAlternative>,
    Option<u64>,
    Option<String>,
    Option<String>,
    Option<legion_contracts::workflow_board::WorkflowBoardPlanningStrategy>,
    Option<String>,
    Option<legion_contracts::workflow_board::WorkflowBoardStrategyMetadata>,
    Option<legion_contracts::workflow_board::WorkflowBoardTaskGranularity>,
    Vec<legion_contracts::workflow_board::WorkflowBoardTaskContract>,
    Vec<legion_contracts::workflow_board::WorkflowBoardPlannedCheck>,
    Vec<legion_contracts::workflow_board::WorkflowBoardAcEvidenceMapping>,
);

fn unpack_plan_fields(
    plan: Option<&legion_contracts::workflow_board::WorkflowBoardPlanDocument>,
) -> UnpackedPlanFields {
    match plan {
        Some(p) => (
            p.lenses.clone(),
            p.alternatives.clone(),
            Some(p.alternatives_revision_count),
            p.selected_alternative_id.clone(),
            p.selection_rationale.clone(),
            p.strategy,
            p.strategy_rationale.clone(),
            p.strategy_metadata.clone(),
            p.granularity,
            p.tasks.clone(),
            p.checks.clone(),
            p.ac_mappings.clone(),
        ),
        None => Default::default(),
    }
}

fn hydrate_exploration(
    planning: &CardPlanningDetail,
    card_status: &str,
) -> Result<
    Option<WorkflowBoardExplorationState>,
    legion_contracts::workflow_board::WorkflowBoardRequestError,
> {
    if planning.exploration_rounds.is_empty() && planning.decisions.is_empty() {
        return Ok(None);
    }
    let latest_round = planning.exploration_rounds.last();
    let round_index = latest_round
        .and_then(|r| u64::try_from(r.round_index).ok())
        .unwrap_or(0);
    let pending_round_prompt = latest_round.is_some_and(|r| r.pending_continue_prompt);
    let paused = card_status == "paused" || latest_round.is_some_and(|r| r.status == "paused");

    let is_resolved = |status: &str| matches!(status, "resolved" | "accepted");
    let resolved_ids: HashSet<&str> = planning
        .decisions
        .iter()
        .filter(|d| is_resolved(d.status.as_str()))
        .map(|d| d.decision_id.as_str())
        .collect();

    let mut decisions = Vec::new();
    let mut unresolved_ids = Vec::new();
    for d in &planning.decisions {
        let status: WorkflowBoardExplorationDecisionStatus = if is_resolved(d.status.as_str()) {
            WorkflowBoardExplorationDecisionStatus::Resolved
        } else {
            WorkflowBoardExplorationDecisionStatus::Unresolved
        };
        if status == WorkflowBoardExplorationDecisionStatus::Unresolved {
            unresolved_ids.push(d.decision_id.clone());
        }
        let deps: Vec<String> = serde_json::from_str(&d.depends_on_json).unwrap_or_default();
        let dep_ready = deps.iter().all(|dep| resolved_ids.contains(dep.as_str()));
        if d.round_index.is_some()
            || is_resolved(d.status.as_str())
            || (dep_ready && decisions.len() < 3)
        {
            decisions.push(WorkflowBoardExplorationDecision {
                id: d.decision_id.clone(),
                prompt: d.prompt.clone(),
                status,
                last_action: parse_opt_enum(d.last_action.as_deref(), "explore_action")?,
                answer: d.answer.clone(),
                explanation: d.rationale.clone(),
                recommendation: d.recommendation.clone(),
                updated_at: d.updated_at.clone(),
            });
        }
    }

    let interview_payload = decisions
        .iter()
        .filter(|d| d.status == WorkflowBoardExplorationDecisionStatus::Resolved)
        .map(|d| format!("{}:{}", d.id, d.answer.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");
    let interview_digest =
        (!interview_payload.is_empty()).then(|| sha256_prefixed(interview_payload.as_bytes()));

    Ok(Some(WorkflowBoardExplorationState {
        round_index,
        pending_round_prompt,
        paused,
        unresolved_ids,
        decisions,
        interview_digest,
    }))
}

fn hydrate_challenge_receipt(
    planning: &CardPlanningDetail,
) -> Result<
    Option<WorkflowBoardChallengeReceipt>,
    legion_contracts::workflow_board::WorkflowBoardRequestError,
> {
    let Some(ch) = planning.challenges.first() else {
        return Ok(None);
    };
    let concerns: Vec<WorkflowBoardChallengeConcern> =
        serde_json::from_str(&ch.concerns_json).map_err(|e| internal(e.to_string()))?;
    let manual_fallback: Option<WorkflowBoardChallengeManualFallback> = ch
        .manual_fallback_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| internal(e.to_string()))?;
    let diff_summary = ch.synthesis_diff_json.as_deref().and_then(|raw| {
        serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|v| {
                v.get("diffSummary")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| Some(raw.to_owned()))
    });
    let opt_trim = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_owned())
    };
    Ok(Some(WorkflowBoardChallengeReceipt {
        status: parse_enum(&ch.status, "challenge_status")?,
        concerns,
        synthesis_count: u64::try_from(ch.synthesis_count.max(0)).unwrap_or(0),
        spec_digest: opt_trim(&ch.spec_digest),
        interview_digest: opt_trim(&ch.interview_digest),
        source_refs_digest: opt_trim(&ch.source_refs_digest),
        manual_fallback,
        diff_summary,
        updated_at: ch.updated_at.clone(),
    }))
}
