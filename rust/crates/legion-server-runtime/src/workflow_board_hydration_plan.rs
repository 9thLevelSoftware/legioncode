#![forbid(unsafe_code)]
//! Plan document, tasks, checks, and AC-evidence hydration from `CardPlanningDetail`.

use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardPlanAlternative, WorkflowBoardPlanDocument,
    WorkflowBoardPlanLensEntry, WorkflowBoardPlannedCheck, WorkflowBoardRequestError,
    WorkflowBoardStrategyMetadata, WorkflowBoardTaskContract,
};
use legion_persistence::workflow_board_repo::types_m2::CardPlanningDetail;

use crate::workflow_board_types::parse_enum;

pub(crate) fn hydrate_plan_document(
    planning: &CardPlanningDetail,
) -> Result<Option<WorkflowBoardPlanDocument>, WorkflowBoardRequestError> {
    let has_rows = planning.plan_design.is_some()
        || !planning.tasks.is_empty()
        || !planning.planned_checks.is_empty()
        || !planning.ac_evidence.is_empty();
    if !has_rows {
        return Ok(None);
    }
    let stored_doc: Option<WorkflowBoardPlanDocument> = planning
        .card_detail
        .card
        .plan_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<WorkflowBoardPlanDocument>(raw).ok());

    let (
        lenses,
        alternatives,
        alternatives_revision_count,
        selected_alternative_id,
        selection_rationale,
        strategy,
        strategy_rationale,
        strategy_metadata,
        granularity,
        updated_at,
    ) = match &planning.plan_design {
        Some(pd) => {
            let lenses: Vec<WorkflowBoardPlanLensEntry> =
                serde_json::from_str(&pd.lenses_json).unwrap_or_default();
            let alternatives: Vec<WorkflowBoardPlanAlternative> =
                serde_json::from_str(&pd.alternatives_json).unwrap_or_default();
            let meta: Option<WorkflowBoardStrategyMetadata> =
                serde_json::from_str(&pd.strategy_metadata_json).ok();
            (
                lenses,
                alternatives,
                u64::try_from(pd.comparison_revision_count.max(0)).unwrap_or(0),
                pd.selected_alternative_id.clone(),
                pd.selection_rationale.clone(),
                Some(parse_enum(&pd.strategy, "plan_strategy")?),
                pd.strategy_rationale.clone(),
                meta,
                Some(parse_enum(&pd.granularity, "task_granularity")?),
                pd.updated_at.clone(),
            )
        }
        None => (
            Vec::new(),
            Vec::new(),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            planning.card_detail.card.updated_at.clone(),
        ),
    };

    let tasks = hydrate_tasks(planning, stored_doc.as_ref())?;
    let checks = hydrate_checks(planning, stored_doc.as_ref());
    let ac_mappings = hydrate_ac_mappings(planning, stored_doc.as_ref())?;

    Ok(Some(WorkflowBoardPlanDocument {
        lenses,
        alternatives,
        alternatives_revision_count,
        selected_alternative_id,
        selection_rationale,
        strategy,
        strategy_rationale,
        strategy_metadata,
        granularity,
        tasks,
        checks,
        ac_mappings,
        plan_digest: planning.card_detail.card.plan_digest.clone(),
        tasks_digest: planning.card_detail.card.tasks_digest.clone(),
        updated_at,
    }))
}

fn hydrate_tasks(
    planning: &CardPlanningDetail,
    stored_doc: Option<&WorkflowBoardPlanDocument>,
) -> Result<Vec<WorkflowBoardTaskContract>, WorkflowBoardRequestError> {
    let mut out = Vec::with_capacity(planning.tasks.len());
    for t in &planning.tasks {
        let doc_match = stored_doc.and_then(|d| d.tasks.iter().find(|dt| dt.task_id == t.task_id));
        let blocked_by: Vec<String> = serde_json::from_str(&t.blocked_by_json).unwrap_or_default();
        let blocks: Vec<String> = serde_json::from_str(&t.blocks_json).unwrap_or_default();
        let files_allowed: Vec<String> =
            serde_json::from_str(&t.files_allowed_json).unwrap_or_default();
        let files_forbidden: Vec<String> =
            serde_json::from_str(&t.files_forbidden_json).unwrap_or_default();
        let expected_artifacts: Vec<String> =
            serde_json::from_str(&t.expected_artifacts_json).unwrap_or_default();
        let verification_commands: Vec<Vec<String>> =
            serde_json::from_str(&t.verification_commands_json).unwrap_or_default();

        out.push(WorkflowBoardTaskContract {
            id: doc_match.and_then(|m| m.id.clone()).or_else(|| {
                planning
                    .card_detail
                    .card
                    .import_provenance_json
                    .as_ref()
                    .map(|_| t.task_id.clone())
            }),
            task_id: t.task_id.clone(),
            title: t.title.clone(),
            depends_on: doc_match
                .and_then(|m| m.depends_on.clone())
                .or_else(|| (!blocked_by.is_empty()).then_some(blocked_by.clone())),
            blocked_by: doc_match
                .and_then(|m| m.blocked_by.clone())
                .or_else(|| (!blocked_by.is_empty()).then_some(blocked_by)),
            blocks: (!blocks.is_empty()).then_some(blocks),
            files_allowed,
            files_forbidden: (!files_forbidden.is_empty()).then_some(files_forbidden),
            expected_artifacts: (!expected_artifacts.is_empty()).then_some(expected_artifacts),
            verification_commands: (!verification_commands.is_empty())
                .then_some(verification_commands),
            testing_method: parse_enum(&t.testing_method, "testing_method")?,
            testing_limitations: t
                .testing_boundary
                .clone()
                .or_else(|| doc_match.and_then(|m| m.testing_limitations.clone())),
            status: Some(parse_enum(&t.status, "task_status")?),
        });
    }
    Ok(out)
}

fn hydrate_checks(
    planning: &CardPlanningDetail,
    stored_doc: Option<&WorkflowBoardPlanDocument>,
) -> Vec<WorkflowBoardPlannedCheck> {
    let mut out = Vec::with_capacity(planning.planned_checks.len());
    for c in &planning.planned_checks {
        let doc_match = stored_doc.and_then(|d| d.checks.iter().find(|dc| dc.id == c.check_id));
        let argv: Vec<String> = serde_json::from_str(&c.argv_json).unwrap_or_default();
        out.push(WorkflowBoardPlannedCheck {
            id: c.check_id.clone(),
            criterion_id: doc_match.and_then(|m| m.criterion_id.clone()),
            argv,
            description: c
                .label
                .clone()
                .or_else(|| doc_match.and_then(|m| m.description.clone())),
        });
    }
    out
}

fn hydrate_ac_mappings(
    planning: &CardPlanningDetail,
    stored_doc: Option<&WorkflowBoardPlanDocument>,
) -> Result<Vec<WorkflowBoardAcEvidenceMapping>, WorkflowBoardRequestError> {
    let mut out = Vec::with_capacity(planning.ac_evidence.len());
    for m in &planning.ac_evidence {
        let doc_match = stored_doc.and_then(|d| {
            d.ac_mappings
                .iter()
                .find(|dm| dm.criterion_id == m.criterion_id)
        });
        let check_ids: Vec<String> = serde_json::from_str(&m.check_ids_json).unwrap_or_default();
        let command: Option<Vec<String>> = m
            .command_argv_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok());
        out.push(WorkflowBoardAcEvidenceMapping {
            criterion_id: m.criterion_id.clone(),
            method: parse_enum(&m.method, "evidence_method")?,
            check_ids: (!check_ids.is_empty()).then_some(check_ids),
            command,
            procedure: m.procedure.clone(),
            external_ref: m.evidence_ref.clone(),
            external_source: doc_match.and_then(|dm| dm.external_source.clone()),
            limitations: doc_match.and_then(|dm| dm.limitations.clone()),
        });
    }
    Ok(out)
}
