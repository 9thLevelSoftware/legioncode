#![forbid(unsafe_code)]
//! M2 `ProjectWorkflowBoardService` tests: brownfield brief intake, `.git` rejection, wireframes,
//! resumable guided exploration, bounded challenge synthesis, and non-UI fallback / `startStage("spec")`.

#[path = "workflow_board_m2_support/spec_helpers.rs"]
mod spec_helpers;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::ids::ProjectId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardApprovalStatus, WorkflowBoardApproveSpecInput,
    WorkflowBoardAssistanceMode, WorkflowBoardChallengeConcernResponse,
    WorkflowBoardChallengeDisposition, WorkflowBoardChallengeStatus,
    WorkflowBoardChallengeStepAction, WorkflowBoardChallengeStepAnswers,
    WorkflowBoardChallengeStepInput, WorkflowBoardCreateCardInput, WorkflowBoardIntakeGoal,
    WorkflowBoardRequestErrorCode, WorkflowBoardSpecSourceType, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput, WorkflowBoardUpdateSpecInput,
};
use spec_helpers::{ch_step_req, concern, run_challenge_synthesis, run_explore_pause_resume};
use workflow_board_m2_support::{TestResult, replay_model, setup_m2_board};

#[tokio::test]
async fn m2_intake_explore_wireframes_and_challenge_flow_work() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    fs::create_dir_all(repo_dir.join("docs"))?;
    fs::write(
        repo_dir.join("docs/input.md"),
        "# Imported Brief\n\n- AC-01: Brief import tracks file digest\n- AC-02: Plan tasks stay disjoint\n",
    )?;

    let git_err = board
        .create_card(&WorkflowBoardCreateCardInput {
            project_id: ProjectId::new("proj-board-1")?,
            title: "Bad Git Path".to_owned(),
            brief: Some("Should fail".to_owned()),
            brief_path: Some(".git/config".to_owned()),
            input_paths: None,
            intake_goal: None,
            wireframes_enabled: None,
            wireframes_index: PatchValue::Absent,
            source_type: Some(WorkflowBoardSpecSourceType::File),
            assistance_mode: Some(WorkflowBoardAssistanceMode::Guided),
            workspace_strategy: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
        })
        .expect_err("must reject .git briefPath");
    assert_eq!(
        git_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "M2 Feature Card".to_owned(),
        brief: None,
        brief_path: Some("docs/input.md".to_owned()),
        input_paths: Some(vec!["README.md".to_owned()]),
        intake_goal: Some(WorkflowBoardIntakeGoal::Change),
        wireframes_enabled: Some(true),
        wireframes_index: PatchValue::Absent,
        source_type: Some(WorkflowBoardSpecSourceType::Codebase),
        assistance_mode: Some(WorkflowBoardAssistanceMode::Guided),
        workspace_strategy: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
    })?;
    let cid = created.summary.card_id.clone();
    assert!(created.spec.brief_digest.is_some());
    assert_eq!(created.spec.source_input_digests.len(), 2);
    assert_eq!(
        created.wireframes_index.as_deref(),
        Some("wireframes/index.html")
    );
    assert!(created.wireframes_digest.is_some());

    let updated_spec = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: created.summary.revision,
            title: None,
            brief: Some("Complete brownfield specification".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: Some(vec![
                WorkflowBoardAcceptanceCriterion {
                    id: "AC-01".to_owned(),
                    text: "Digest".to_owned(),
                },
                WorkflowBoardAcceptanceCriterion {
                    id: "AC-02".to_owned(),
                    text: "Disjoint".to_owned(),
                },
            ]),
            constraints: Some(vec!["Zero auto-commits".to_owned()]),
            source_refs: Some(vec!["README.md".to_owned()]),
            wireframes_index: PatchValue::Absent,
        })
        .await?;

    let exp_rev = run_explore_pause_resume(&board, &cid, updated_spec.revision).await?;
    let ch_rev = run_challenge_synthesis(&board, &cid, exp_rev).await?;
    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid,
                expected_revision: ch_rev,
                expected_digest: updated_spec.spec_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(
        approved.spec_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );
    Ok(())
}

#[tokio::test]
async fn m2_non_ui_wireframes_skip_manual_fallback_and_start_spec_stage() -> TestResult {
    let (_tmp, _orch, board, _repo_str) = setup_m2_board()?;
    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "Non-UI Direct Card".to_owned(),
        brief: Some("Pure backend refactor".to_owned()),
        brief_path: None,
        input_paths: None,
        intake_goal: Some(WorkflowBoardIntakeGoal::Audit),
        wireframes_enabled: Some(false),
        wireframes_index: PatchValue::Null,
        source_type: Some(WorkflowBoardSpecSourceType::Idea),
        assistance_mode: Some(WorkflowBoardAssistanceMode::Direct),
        workspace_strategy: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
    })?;
    let cid = created.summary.card_id.clone();
    assert_eq!(created.wireframes_index, None);

    let fallback = board
        .challenge_step(&WorkflowBoardChallengeStepInput {
            card_id: cid.clone(),
            expected_revision: created.summary.revision,
            action: WorkflowBoardChallengeStepAction::ManualFallback,
            concerns: None,
            acknowledgement_token: Some("I acknowledge".to_owned()),
            unresolved_concern_ids: Some(Vec::new()),
            answers: Some(WorkflowBoardChallengeStepAnswers::Strings(vec![
                "Risk 1 accepted".to_owned(),
                "Verification by test suite".to_owned(),
                "Rollback via git revert".to_owned(),
            ])),
            approved: None,
        })
        .await?;
    assert_eq!(
        fallback.challenge_receipt.status,
        WorkflowBoardChallengeStatus::ManualFallback
    );

    let spec_started = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid,
            expected_revision: fallback.revision,
            stage: WorkflowBoardStageRunStage::Spec,
            task_id: None,
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: Some(RuntimeMode::ApprovalRequired),
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    assert_eq!(spec_started.stage, WorkflowBoardStageRunStage::Spec);
    assert!(!spec_started.resumed);
    Ok(())
}

fn assert_partial_metadata(
    partial: &legion_contracts::workflow_board::WorkflowBoardChallengeStepResult,
) {
    assert_eq!(partial.concerns.len(), 2);
    assert_eq!(partial.concerns[0].summary, "Original summary 1");
    assert_eq!(
        partial.concerns[0].why_matters.as_deref(),
        Some("Matters 1")
    );
    assert_eq!(partial.concerns[0].evidence.as_deref(), Some("README.md"));
    assert_eq!(partial.concerns[0].assumption.as_deref(), Some("Assume 1"));
    assert_eq!(
        partial.concerns[0].answer.as_deref(),
        Some("Human answer 1")
    );
    assert_eq!(partial.concerns[1].id, "CON-2");
    assert_eq!(partial.concerns[1].summary, "Original summary 2");
    assert_eq!(
        partial.concerns[1].disposition,
        WorkflowBoardChallengeDisposition::Unresolved
    );
    assert_eq!(
        partial.challenge_receipt.status,
        WorkflowBoardChallengeStatus::InProgress
    );
}

async fn assert_partial_gates(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    rev: u64,
) {
    let unknown_err = ch_step_req(
        board,
        cid,
        rev,
        WorkflowBoardChallengeStepAction::Resolve,
        None,
        Some(WorkflowBoardChallengeStepAnswers::Responses(vec![
            WorkflowBoardChallengeConcernResponse {
                concern_id: "CON-999".to_owned(),
                disposition: WorkflowBoardChallengeDisposition::Accept,
                response: None,
                reason: None,
            },
        ])),
    )
    .await
    .expect_err("unknown concernId must fail");
    assert_eq!(
        unknown_err.code,
        WorkflowBoardRequestErrorCode::InvalidInput
    );

    let complete_err = ch_step_req(
        board,
        cid,
        rev,
        WorkflowBoardChallengeStepAction::Complete,
        None,
        None,
    )
    .await
    .expect_err("omitted CON-2 must block complete");
    assert_eq!(
        complete_err.code,
        WorkflowBoardRequestErrorCode::ChallengeUnresolved
    );

    let approve_err = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: rev,
                expected_digest: None,
            },
            "client",
        )
        .await
        .expect_err("omitted CON-2 must block approveSpec");
    assert_eq!(
        approve_err.code,
        WorkflowBoardRequestErrorCode::ChallengeUnresolved
    );
}

#[tokio::test]
async fn m2_challenge_partial_responses_preserve_metadata_and_unresolved_concerns() -> TestResult {
    let (_tmp, _orch, board, _repo_str) = setup_m2_board()?;
    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "Partial Challenge Responses".to_owned(),
        brief: Some("Verify challenge responses merge cleanly".to_owned()),
        brief_path: None,
        input_paths: None,
        intake_goal: None,
        wireframes_enabled: Some(false),
        wireframes_index: PatchValue::Null,
        source_type: Some(WorkflowBoardSpecSourceType::Idea),
        assistance_mode: Some(WorkflowBoardAssistanceMode::Direct),
        workspace_strategy: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
    })?;
    let cid = created.summary.card_id.clone();
    let started = ch_step_req(
        &board,
        &cid,
        created.summary.revision,
        WorkflowBoardChallengeStepAction::Start,
        Some(vec![
            concern(
                "CON-1",
                "Original summary 1",
                Some("Matters 1"),
                Some("README.md"),
                Some("Assume 1"),
                WorkflowBoardChallengeDisposition::Unresolved,
                None,
            ),
            concern(
                "CON-2",
                "Original summary 2",
                Some("Matters 2"),
                None,
                None,
                WorkflowBoardChallengeDisposition::Unresolved,
                None,
            ),
        ]),
        None,
    )
    .await?;

    let partial = ch_step_req(
        &board,
        &cid,
        started.revision,
        WorkflowBoardChallengeStepAction::Resolve,
        None,
        Some(WorkflowBoardChallengeStepAnswers::Responses(vec![
            WorkflowBoardChallengeConcernResponse {
                concern_id: "CON-1".to_owned(),
                disposition: WorkflowBoardChallengeDisposition::Answer,
                response: Some("Human answer 1".to_owned()),
                reason: None,
            },
        ])),
    )
    .await?;
    assert_partial_metadata(&partial);
    assert_partial_gates(&board, &cid, partial.revision).await;
    Ok(())
}
