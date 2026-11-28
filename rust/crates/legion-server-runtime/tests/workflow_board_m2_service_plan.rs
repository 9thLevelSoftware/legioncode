#![forbid(unsafe_code)]
//! M2 `ProjectWorkflowBoardService` tests: `updatePlan`, `approvePlan`, `returnForRevision`
//! invalidation, journal recovery for `plan.md`, and external edit reconciliation for wireframes.

#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::ids::ProjectId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAcceptanceCriterion, WorkflowBoardApprovalStatus,
    WorkflowBoardApprovePlanInput, WorkflowBoardApproveSpecInput, WorkflowBoardArtifactKind,
    WorkflowBoardAssistanceMode, WorkflowBoardCreateCardInput, WorkflowBoardEvidenceMethod,
    WorkflowBoardGetArtifactInput, WorkflowBoardGetCardInput, WorkflowBoardIntakeGoal,
    WorkflowBoardPlanAlternative, WorkflowBoardPlanLens, WorkflowBoardPlanLensEntry,
    WorkflowBoardPlannedCheck, WorkflowBoardPlanningStrategy, WorkflowBoardReturnForRevisionInput,
    WorkflowBoardReturnTargetStage, WorkflowBoardSpecSourceType, WorkflowBoardStage,
    WorkflowBoardTaskContract, WorkflowBoardTaskGranularity, WorkflowBoardTaskTestingMethod,
    WorkflowBoardUpdatePlanInput, WorkflowBoardUpdateSpecInput,
};
use legion_persistence::workflow_board_repo::stage_write;
use legion_persistence::workflow_board_repo::types::StageWriteInput;
use legion_server_runtime::{ProjectWorkflowBoardService, reconcile_incomplete_write_journal};
use workflow_board_m2_support::{TestResult, replay_model, setup_m2_board};

fn all_lenses() -> Vec<WorkflowBoardPlanLensEntry> {
    vec![
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Ux,
            applicable: Some(false),
            summary: None,
            reason: Some("Backend flow".to_owned()),
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Functional,
            applicable: Some(true),
            summary: Some("AC-01/02".to_owned()),
            reason: None,
        },
        WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Architecture,
            applicable: Some(true),
            summary: Some("Modular".to_owned()),
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
            summary: Some("Single PR".to_owned()),
            reason: None,
        },
    ]
}

fn two_alts() -> Vec<WorkflowBoardPlanAlternative> {
    vec![
        WorkflowBoardPlanAlternative {
            id: "alt-a".to_owned(),
            title: "Direct typed service".to_owned(),
            summary: "In-process".to_owned(),
            tradeoffs: "Clean parity".to_owned(),
        },
        WorkflowBoardPlanAlternative {
            id: "alt-b".to_owned(),
            title: "External daemon".to_owned(),
            summary: "Sidecar".to_owned(),
            tradeoffs: "IPC overhead".to_owned(),
        },
    ]
}

fn sample_update_plan_input(
    cid: legion_contracts::ids::WorkflowCardId,
    rev: u64,
) -> WorkflowBoardUpdatePlanInput {
    WorkflowBoardUpdatePlanInput {
        card_id: cid,
        expected_revision: rev,
        lenses: Some(all_lenses()),
        alternatives: Some(two_alts()),
        selected_alternative_id: PatchValue::Set("alt-a".to_owned()),
        selection_rationale: Some("Smallest clean boundary".to_owned()),
        revise_alternatives_feedback: None,
        strategy: Some(WorkflowBoardPlanningStrategy::Outcomes),
        strategy_rationale: Some("Deliver AC-01 & AC-02".to_owned()),
        strategy_metadata: None,
        granularity: Some(WorkflowBoardTaskGranularity::Balanced),
        tasks: Some(vec![
            WorkflowBoardTaskContract {
                id: None,
                task_id: "TSK-01".to_owned(),
                title: "Brief intake task".to_owned(),
                depends_on: None,
                blocked_by: None,
                blocks: None,
                files_allowed: vec!["docs/input.md".to_owned()],
                files_forbidden: Some(vec!["README.md".to_owned()]),
                expected_artifacts: Some(vec!["docs/input.md".to_owned()]),
                verification_commands: Some(vec![vec!["git".to_owned(), "status".to_owned()]]),
                testing_method: WorkflowBoardTaskTestingMethod::TestFirst,
                testing_limitations: None,
                status: None,
            },
            WorkflowBoardTaskContract {
                id: None,
                task_id: "TSK-02".to_owned(),
                title: "Board README check".to_owned(),
                depends_on: Some(vec!["TSK-01".to_owned()]),
                blocked_by: Some(vec!["TSK-01".to_owned()]),
                blocks: None,
                files_allowed: vec!["README.md".to_owned()],
                files_forbidden: Some(vec!["docs/input.md".to_owned()]),
                expected_artifacts: Some(vec!["README.md".to_owned()]),
                verification_commands: Some(vec![vec!["git".to_owned(), "status".to_owned()]]),
                testing_method: WorkflowBoardTaskTestingMethod::RegressionFirst,
                testing_limitations: Some("No network".to_owned()),
                status: None,
            },
        ]),
        checks: Some(vec![WorkflowBoardPlannedCheck {
            id: "CHK-01".to_owned(),
            criterion_id: Some("TSK-01".to_owned()),
            argv: vec!["git".to_owned(), "status".to_owned()],
            description: Some("Verify clean git".to_owned()),
        }]),
        ac_mappings: Some(vec![
            WorkflowBoardAcEvidenceMapping {
                criterion_id: "AC-01".to_owned(),
                method: WorkflowBoardEvidenceMethod::Automated,
                check_ids: Some(vec!["CHK-01".to_owned()]),
                command: None,
                procedure: None,
                external_ref: None,
                external_source: None,
                limitations: None,
            },
            WorkflowBoardAcEvidenceMapping {
                criterion_id: "AC-02".to_owned(),
                method: WorkflowBoardEvidenceMethod::Automated,
                check_ids: None,
                command: Some(vec!["git".to_owned(), "status".to_owned()]),
                procedure: None,
                external_ref: None,
                external_source: None,
                limitations: None,
            },
        ]),
    }
}

async fn setup_approved_spec_card(
    board: &ProjectWorkflowBoardService,
    repo_dir: &Path,
) -> Result<(legion_contracts::ids::WorkflowCardId, u64, Option<String>), Box<dyn std::error::Error>>
{
    fs::create_dir_all(repo_dir.join("docs"))?;
    fs::write(repo_dir.join("docs/input.md"), "# Plan Input\n")?;
    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "M2 Plan Card".to_owned(),
        brief: Some("Plan and recover".to_owned()),
        brief_path: Some("docs/input.md".to_owned()),
        input_paths: Some(vec!["README.md".to_owned()]),
        intake_goal: Some(WorkflowBoardIntakeGoal::Change),
        wireframes_enabled: Some(true),
        wireframes_index: PatchValue::Absent,
        source_type: Some(WorkflowBoardSpecSourceType::Codebase),
        assistance_mode: Some(WorkflowBoardAssistanceMode::Balanced),
        workspace_strategy: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
    })?;
    let cid = created.summary.card_id.clone();
    let updated_spec = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: created.summary.revision,
            title: None,
            brief: Some("Spec for plan".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: Some(vec![
                WorkflowBoardAcceptanceCriterion {
                    id: "AC-01".to_owned(),
                    text: "Check runs".to_owned(),
                },
                WorkflowBoardAcceptanceCriterion {
                    id: "AC-02".to_owned(),
                    text: "Disjoint tasks".to_owned(),
                },
            ]),
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await?;
    let approved_spec = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: updated_spec.revision,
                expected_digest: updated_spec.spec_digest.clone(),
            },
            "client",
        )
        .await?;
    Ok((cid, approved_spec.revision, updated_spec.spec_digest))
}

#[tokio::test]
async fn m2_plan_approve_invalidation_and_journal_recovery_work() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, spec_app_rev, spec_digest) = setup_approved_spec_card(&board, repo_dir).await?;

    let updated_plan = board
        .update_plan(&sample_update_plan_input(cid.clone(), spec_app_rev))
        .await?;
    let plan_art = board.get_artifact(&WorkflowBoardGetArtifactInput {
        card_id: cid.clone(),
        key: None,
        kind: Some(WorkflowBoardArtifactKind::Plan),
    })?;
    assert_eq!(plan_art.key, "plan.md");

    let approved_plan = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: updated_plan.revision,
                expected_spec_digest: spec_digest,
                expected_plan_digest: updated_plan.plan_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(
        approved_plan.plan_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );

    let returned = board
        .return_for_revision(&WorkflowBoardReturnForRevisionInput {
            card_id: cid.clone(),
            expected_revision: approved_plan.revision,
            target_stage: WorkflowBoardReturnTargetStage::Plan,
            reason: "Refine TSK-02".to_owned(),
        })
        .await?;
    assert_eq!(returned.stage, WorkflowBoardStage::Plan);
    assert_eq!(
        returned.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Valid)
    );
    assert_eq!(
        returned.plan_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );

    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    let torn = card_dir.join("plan.md.wj-torn.tmp");
    fs::write(&torn, "torn")?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        stage_write(
            &conn,
            &StageWriteInput {
                id: "wj-plan-torn",
                card_id: cid.as_str(),
                target_path: &card_dir.join("plan.md").to_string_lossy(),
                staging_path: Some(&torn.to_string_lossy()),
                operation: "write",
                expected_digest: Some("sha256:bad"),
                previous_digest: None,
                revision: Some(99),
            },
            "2026-11-24T12:00:00Z",
        )?;
        assert_eq!(
            reconcile_incomplete_write_journal(&conn, "2026-11-24T12:00:01Z")?.rolled_back,
            1
        );
    }
    assert!(!torn.exists());

    fs::write(
        card_dir.join("wireframes/index.html"),
        "<html>tampered</html>",
    )?;
    let after_wf_tamper = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid,
        project_id: None,
    })?;
    assert_eq!(
        after_wf_tamper.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    Ok(())
}

#[tokio::test]
async fn update_spec_in_execute_regresses_stage_and_card_json_and_recovers() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, spec_app_rev, _) = setup_approved_spec_card(&board, repo_dir).await?;
    let planned = board
        .update_plan(&sample_update_plan_input(cid.clone(), spec_app_rev))
        .await?;
    let in_exec = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: planned.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(in_exec.card.summary.stage, WorkflowBoardStage::Execute);

    let edited = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: in_exec.revision,
            title: None,
            brief: Some("Edited spec while in execute stage".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await?;
    assert_eq!(edited.card.summary.stage, WorkflowBoardStage::Spec);
    assert_eq!(
        edited.card.summary.next_action.as_deref(),
        Some("revise_spec")
    );
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    let disk_card: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(card_dir.join("card.json"))?)?;
    assert_eq!(disk_card["stage"], "spec");
    assert_eq!(disk_card["nextAction"], "revise_spec");

    let reapproved_spec = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: edited.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(reapproved_spec.card.summary.stage, WorkflowBoardStage::Plan);

    let reapproved_plan = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid,
                expected_revision: reapproved_spec.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        reapproved_plan.card.summary.stage,
        WorkflowBoardStage::Execute
    );
    Ok(())
}
