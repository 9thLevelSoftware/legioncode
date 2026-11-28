#![forbid(unsafe_code)]
//! Shared M3 service setup helpers for `legion-server-runtime` integration tests.

use legion_contracts::ids::{ProjectId, WorkflowCardId};
use legion_contracts::orchestration::{PatchValue, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAcceptanceCriterion,
    WorkflowBoardApprovePlanInput, WorkflowBoardApproveSpecInput, WorkflowBoardAssistanceMode,
    WorkflowBoardCreateCardInput, WorkflowBoardPlanAlternative, WorkflowBoardPlanLens,
    WorkflowBoardPlanLensEntry, WorkflowBoardPlannedCheck, WorkflowBoardPlanningStrategy,
    WorkflowBoardSpecSourceType, WorkflowBoardTaskContract, WorkflowBoardTaskGranularity,
    WorkflowBoardTaskTestingMethod, WorkflowBoardUpdatePlanInput, WorkflowBoardUpdateSpecInput,
};
use legion_server_runtime::ProjectWorkflowBoardService;

use super::workflow_board_m2_support::replay_model;

pub fn m3_task(
    id: &str,
    allowed: Vec<&str>,
    forbidden: Vec<&str>,
    depends_on: &[&str],
) -> WorkflowBoardTaskContract {
    WorkflowBoardTaskContract {
        id: None,
        task_id: id.to_owned(),
        title: format!("Task {id}"),
        depends_on: (!depends_on.is_empty())
            .then(|| depends_on.iter().map(|s| (*s).to_owned()).collect()),
        blocked_by: None,
        blocks: None,
        files_allowed: allowed.into_iter().map(str::to_owned).collect(),
        files_forbidden: (!forbidden.is_empty())
            .then(|| forbidden.into_iter().map(str::to_owned).collect()),
        expected_artifacts: None,
        verification_commands: Some(vec![vec!["git".to_owned(), "status".to_owned()]]),
        testing_method: WorkflowBoardTaskTestingMethod::TestFirst,
        testing_limitations: None,
        status: None,
    }
}

fn default_lenses_and_alts() -> (
    Vec<WorkflowBoardPlanLensEntry>,
    Vec<WorkflowBoardPlanAlternative>,
) {
    let lenses = vec![
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Ux,
            applicable: Some(false),
            summary: None,
            reason: Some("Backend".to_owned()),
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Functional,
            applicable: Some(true),
            summary: Some("Core".to_owned()),
            reason: None,
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Architecture,
            applicable: Some(true),
            summary: Some("Clean".to_owned()),
            reason: None,
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Quality,
            applicable: Some(true),
            summary: Some("Tested".to_owned()),
            reason: None,
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Infrastructure,
            applicable: Some(false),
            summary: None,
            reason: Some("Local".to_owned()),
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Delivery,
            applicable: Some(true),
            summary: Some("PR".to_owned()),
            reason: None,
        },
    ];
    let alternatives = vec![
        WorkflowBoardPlanAlternative {
            id: "alt-a".to_owned(),
            title: "Direct".to_owned(),
            summary: "Direct".to_owned(),
            tradeoffs: "Simple".to_owned(),
        },
        WorkflowBoardPlanAlternative {
            id: "alt-b".to_owned(),
            title: "Alt".to_owned(),
            summary: "Alt".to_owned(),
            tradeoffs: "Complex".to_owned(),
        },
    ];
    (lenses, alternatives)
}

pub async fn setup_approved_m3_card(
    board: &ProjectWorkflowBoardService,
    strategy: Option<WorkspaceStrategy>,
    tasks: Vec<WorkflowBoardTaskContract>,
    checks: Vec<WorkflowBoardPlannedCheck>,
    ac_mappings: Vec<WorkflowBoardAcEvidenceMapping>,
) -> Result<(WorkflowCardId, u64), Box<dyn std::error::Error>> {
    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "M3 Feature Card".to_owned(),
        brief: Some("Deliver M3 Execute and Ship".to_owned()),
        brief_path: None,
        input_paths: None,
        intake_goal: None,
        wireframes_enabled: Some(false),
        wireframes_index: PatchValue::Absent,
        source_type: Some(WorkflowBoardSpecSourceType::Idea),
        assistance_mode: Some(WorkflowBoardAssistanceMode::Direct),
        workspace_strategy: strategy,
        model_selection: Some(replay_model()),
        runtime_mode: Some(RuntimeMode::FullAccess),
        interaction_mode: None,
    })?;
    let cid = created.summary.card_id.clone();

    let criteria: Vec<WorkflowBoardAcceptanceCriterion> = ac_mappings
        .iter()
        .map(|m| WorkflowBoardAcceptanceCriterion {
            id: m.criterion_id.clone(),
            text: format!("Criterion {}", m.criterion_id),
        })
        .collect();
    let upd_spec = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: created.summary.revision,
            title: None,
            brief: Some("Detailed M3 spec".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: Some(criteria),
            constraints: Some(vec!["Zero auto-commit".to_owned()]),
            source_refs: Some(vec!["README.md".to_owned()]),
            wireframes_index: PatchValue::Absent,
        })
        .await?;

    let app_spec = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: upd_spec.revision,
                expected_digest: upd_spec.spec_digest,
            },
            "client",
        )
        .await?;

    let (lenses, alternatives) = default_lenses_and_alts();
    let upd_plan = board
        .update_plan(&WorkflowBoardUpdatePlanInput {
            card_id: cid.clone(),
            expected_revision: app_spec.revision,
            lenses: Some(lenses),
            alternatives: Some(alternatives),
            selected_alternative_id: PatchValue::Set("alt-a".to_owned()),
            selection_rationale: Some("Smallest surface".to_owned()),
            revise_alternatives_feedback: None,
            strategy: Some(WorkflowBoardPlanningStrategy::Outcomes),
            strategy_rationale: None,
            strategy_metadata: None,
            granularity: Some(WorkflowBoardTaskGranularity::Balanced),
            tasks: Some(tasks),
            checks: (!checks.is_empty()).then_some(checks),
            ac_mappings: Some(ac_mappings),
        })
        .await?;

    let app_plan = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: upd_plan.revision,
                expected_spec_digest: None,
                expected_plan_digest: upd_plan.plan_digest,
            },
            "client",
        )
        .await?;

    Ok((cid, app_plan.revision))
}
