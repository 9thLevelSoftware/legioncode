#![forbid(unsafe_code)]
//! Shared plan setup helpers for M2 remediation and security integration tests.

use legion_contracts::ids::{ProjectId, WorkflowCardId};
use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAcceptanceCriterion,
    WorkflowBoardApproveSpecInput, WorkflowBoardAssistanceMode, WorkflowBoardCreateCardInput,
    WorkflowBoardEvidenceMethod, WorkflowBoardIntakeGoal, WorkflowBoardPlanAlternative,
    WorkflowBoardPlanLens, WorkflowBoardPlanLensEntry, WorkflowBoardPlanningStrategy,
    WorkflowBoardSpecSourceType, WorkflowBoardTaskContract, WorkflowBoardTaskGranularity,
    WorkflowBoardTaskTestingMethod, WorkflowBoardUpdatePlanInput, WorkflowBoardUpdateSpecInput,
};
use legion_server_runtime::ProjectWorkflowBoardService;

use super::workflow_board_m2_support::replay_model;

pub fn make_task(id: &str, allowed: &str) -> WorkflowBoardTaskContract {
    WorkflowBoardTaskContract {
        id: None,
        task_id: id.to_owned(),
        title: format!("Task {id}"),
        depends_on: None,
        blocked_by: None,
        blocks: None,
        files_allowed: vec![allowed.to_owned()],
        files_forbidden: None,
        expected_artifacts: None,
        verification_commands: Some(vec![vec!["git".to_owned(), "status".to_owned()]]),
        testing_method: WorkflowBoardTaskTestingMethod::TestFirst,
        testing_limitations: None,
        status: None,
    }
}

pub fn make_plan_input(
    cid: WorkflowCardId,
    rev: u64,
    tasks: Vec<WorkflowBoardTaskContract>,
) -> WorkflowBoardUpdatePlanInput {
    WorkflowBoardUpdatePlanInput {
        card_id: cid,
        expected_revision: rev,
        lenses: Some(vec![
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
        ]),
        alternatives: Some(vec![
            WorkflowBoardPlanAlternative {
                id: "alt-a".to_owned(),
                title: "Direct".to_owned(),
                summary: "In-proc".to_owned(),
                tradeoffs: "Simple".to_owned(),
            },
            WorkflowBoardPlanAlternative {
                id: "alt-b".to_owned(),
                title: "Sidecar".to_owned(),
                summary: "Out-proc".to_owned(),
                tradeoffs: "IPC".to_owned(),
            },
        ]),
        selected_alternative_id: PatchValue::Set("alt-a".to_owned()),
        selection_rationale: Some("Smallest surface".to_owned()),
        revise_alternatives_feedback: None,
        strategy: Some(WorkflowBoardPlanningStrategy::Outcomes),
        strategy_rationale: None,
        strategy_metadata: None,
        granularity: Some(WorkflowBoardTaskGranularity::Balanced),
        tasks: Some(tasks),
        checks: None,
        ac_mappings: Some(vec![WorkflowBoardAcEvidenceMapping {
            criterion_id: "AC-01".to_owned(),
            method: WorkflowBoardEvidenceMethod::Automated,
            check_ids: None,
            command: Some(vec!["git".to_owned(), "status".to_owned()]),
            procedure: None,
            external_ref: None,
            external_source: None,
            limitations: None,
        }]),
    }
}

pub async fn setup_spec_approved_card(
    board: &ProjectWorkflowBoardService,
    wireframes_index: PatchValue<String>,
) -> Result<(WorkflowCardId, u64), Box<dyn std::error::Error>> {
    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "Remediation Card".to_owned(),
        brief: Some("Initial brief".to_owned()),
        brief_path: None,
        input_paths: None,
        intake_goal: Some(WorkflowBoardIntakeGoal::Change),
        wireframes_enabled: Some(false),
        wireframes_index,
        source_type: Some(WorkflowBoardSpecSourceType::Idea),
        assistance_mode: Some(WorkflowBoardAssistanceMode::Direct),
        workspace_strategy: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
    })?;
    let cid = created.summary.card_id;
    let updated = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: created.summary.revision,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: Some(vec![WorkflowBoardAcceptanceCriterion {
                id: "AC-01".to_owned(),
                text: "Verified".to_owned(),
            }]),
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await?;
    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: updated.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;
    Ok((cid, approved.revision))
}
