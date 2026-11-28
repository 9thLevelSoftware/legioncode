#![forbid(unsafe_code)]
//! Iteration 2 remediation regression tests (Findings 5, 6, 7, 8, 10).

#[path = "workflow_board_m2_support/plan_helpers.rs"]
mod plan_helpers;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::ids::ProjectId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::workflow_board::{
    WorkflowBoardApprovalStatus, WorkflowBoardApprovePlanInput, WorkflowBoardApproveSpecInput,
    WorkflowBoardAssistanceMode, WorkflowBoardChallengeConcernResponse,
    WorkflowBoardChallengeDisposition, WorkflowBoardChallengeStepAction,
    WorkflowBoardChallengeStepAnswers, WorkflowBoardChallengeStepInput,
    WorkflowBoardCreateCardInput, WorkflowBoardExploreAction, WorkflowBoardExploreAnswerInput,
    WorkflowBoardExploreStepAction, WorkflowBoardExploreStepInput, WorkflowBoardGetCardInput,
    WorkflowBoardIntakeGoal, WorkflowBoardRequestErrorCode, WorkflowBoardReturnForRevisionInput,
    WorkflowBoardReturnTargetStage, WorkflowBoardSpecSourceType, WorkflowBoardStage,
    WorkflowBoardUpdateSpecInput,
};
use legion_persistence::workflow_board_repo::stage_write;
use legion_persistence::workflow_board_repo::types::StageWriteInput;
use legion_server_runtime::reconcile_incomplete_write_journal;
use legion_workflows::sha256_prefixed;
use plan_helpers::{make_plan_input, make_task, setup_spec_approved_card};
use serde_json::Value;
use workflow_board_m2_support::{TestResult, replay_model, setup_m2_board};

#[tokio::test]
async fn custom_wireframe_recovery_orphan_tasks_and_external_task_edits() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, rev) =
        setup_spec_approved_card(&board, PatchValue::Set("mockups/main.html".to_owned())).await?;
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    let custom_wf = card_dir.join("mockups/main.html");
    let wf_bytes = fs::read(&custom_wf)?;
    let wf_digest = sha256_prefixed(&wf_bytes);
    fs::remove_file(&custom_wf)?;
    let staged = card_dir.join("mockups_main.html.wj-custom.tmp");
    fs::write(&staged, &wf_bytes)?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        stage_write(
            &conn,
            &StageWriteInput {
                id: "wj-custom",
                card_id: cid.as_str(),
                target_path: &custom_wf.to_string_lossy(),
                staging_path: Some(&staged.to_string_lossy()),
                operation: "write",
                expected_digest: Some(&wf_digest),
                previous_digest: None,
                revision: Some(1),
            },
            "2026-11-24T12:00:00Z",
        )?;
        let report = reconcile_incomplete_write_journal(&conn, "2026-11-24T12:00:01Z")?;
        assert_eq!(report.committed, 1);
        assert_eq!(report.rolled_back, 0);
    }
    assert!(custom_wf.exists());

    let p1 = board
        .update_plan(&make_plan_input(
            cid.clone(),
            rev,
            vec![
                make_task("TSK-1", "src/a.rs"),
                make_task("TSK-2", "src/b.rs"),
            ],
        ))
        .await?;
    assert!(card_dir.join("tasks/TSK-2.md").exists());
    let p2 = board
        .update_plan(&make_plan_input(
            cid.clone(),
            p1.revision,
            vec![make_task("TSK-1", "src/a.rs")],
        ))
        .await?;
    assert!(!card_dir.join("tasks/TSK-2.md").exists());
    assert!(!p2.card.artifact_keys.iter().any(|k| k == "tasks/TSK-2.md"));

    let app_plan = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: p2.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        app_plan.plan_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );
    fs::write(card_dir.join("tasks/TSK-1.md"), "# TAMPERED\n")?;
    let reconciled = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid,
        project_id: None,
    })?;
    assert_eq!(
        reconciled.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Valid)
    );
    assert_eq!(
        reconciled.plan_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    Ok(())
}

#[tokio::test]
async fn noop_wireframes_card_json_sync_and_explore_post_approve_spec() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());

    let noop_null = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: rev,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Null,
        })
        .await?;
    assert_eq!(noop_null.revision, rev);
    assert!(noop_null.diff.is_none());
    assert_eq!(
        noop_null.spec_approval_status,
        Some(WorkflowBoardApprovalStatus::Valid)
    );

    let set_wf = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: rev,
            title: None,
            brief: None,
            intake_goal: Some(WorkflowBoardIntakeGoal::Audit),
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Set("wireframes/index.html".to_owned()),
        })
        .await?;
    let disk_card: Value = serde_json::from_str(&fs::read_to_string(card_dir.join("card.json"))?)?;
    assert_eq!(disk_card["intakeGoal"], "audit");
    assert_eq!(
        disk_card["wireframesDigest"],
        set_wf.card.wireframes_digest.as_deref().unwrap_or("")
    );

    let reapp_spec = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: set_wf.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;
    let noop_set = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: reapp_spec.revision,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Set("wireframes/index.html".to_owned()),
        })
        .await?;
    assert_eq!(noop_set.revision, reapp_spec.revision);
    assert_eq!(
        noop_set.spec_approval_status,
        Some(WorkflowBoardApprovalStatus::Valid)
    );

    let late_exp = board
        .explore_step(&WorkflowBoardExploreStepInput {
            card_id: cid,
            expected_revision: reapp_spec.revision,
            action: Some(WorkflowBoardExploreStepAction::Step),
            continue_round: None,
            answers: Some(vec![WorkflowBoardExploreAnswerInput {
                decision_id: "DEC-LATE".to_owned(),
                action: Some(WorkflowBoardExploreAction::Edit),
                choice: None,
                answer: Some("Late decision".to_owned()),
                note: None,
            }]),
        })
        .await?;
    assert_eq!(late_exp.card.summary.stage, WorkflowBoardStage::Spec);
    assert_eq!(
        late_exp.card.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    Ok(())
}

#[tokio::test]
async fn update_plan_in_execute_transitions_to_plan_and_return_notifies() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, spec_rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;
    let plan_up = board
        .update_plan(&make_plan_input(
            cid.clone(),
            spec_rev,
            vec![make_task("TSK-1", "src/a.rs")],
        ))
        .await?;
    let plan_ok = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: plan_up.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(plan_ok.card.summary.stage, WorkflowBoardStage::Execute);

    let in_exec_edit = board
        .update_plan(&make_plan_input(
            cid.clone(),
            plan_ok.revision,
            vec![make_task("TSK-1", "src/a.rs")],
        ))
        .await?;
    assert_eq!(in_exec_edit.card.summary.stage, WorkflowBoardStage::Plan);
    assert_eq!(
        in_exec_edit.plan_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    let reapproved = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: in_exec_edit.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        reapproved.plan_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );

    let mut rx = board.subscribe_changes();
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    fs::write(card_dir.join("spec.md"), "# externally edited\n")?;
    let _ = board
        .return_for_revision(&WorkflowBoardReturnForRevisionInput {
            card_id: cid,
            expected_revision: 999_999,
            target_stage: WorkflowBoardReturnTargetStage::Spec,
            reason: "Stale check".to_owned(),
        })
        .await
        .expect_err("stale revision must fail after reconciliation");
    assert_eq!(rx.try_recv().ok().as_deref(), Some("proj-board-1"));
    Ok(())
}

#[tokio::test]
async fn approve_plan_rejects_pre_approve_tampered_or_missing_task_contract_and_resolve_without_start()
-> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let (cid, spec_rev) = setup_spec_approved_card(&board, PatchValue::Null).await?;
    let planned = board
        .update_plan(&make_plan_input(
            cid.clone(),
            spec_rev,
            vec![make_task("TSK-1", "src/a.rs")],
        ))
        .await?;
    let task_path = repo_dir
        .join(".legion/workflows")
        .join(cid.as_str())
        .join("tasks/TSK-1.md");

    fs::write(&task_path, "# TAMPERED BEFORE APPROVE_PLAN\n")?;
    let tampered_err = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: planned.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await
        .expect_err("approvePlan must reject tampered tasks/*.md on disk");
    assert_eq!(
        tampered_err.code,
        WorkflowBoardRequestErrorCode::DigestMismatch
    );

    fs::remove_file(&task_path)?;
    let missing_err = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: planned.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await
        .expect_err("approvePlan must reject missing tasks/*.md on disk");
    assert_eq!(
        missing_err.code,
        WorkflowBoardRequestErrorCode::DigestMismatch
    );
    let card = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid,
        project_id: None,
    })?;
    assert_eq!(card.summary.stage, WorkflowBoardStage::Plan);
    assert!(card.plan_approval.is_none());

    let fresh = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "No Start Challenge Card".to_owned(),
        brief: Some("Challenge resolve without start must fail".to_owned()),
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
    let resolve_err = board
        .challenge_step(&WorkflowBoardChallengeStepInput {
            card_id: fresh.summary.card_id,
            expected_revision: fresh.summary.revision,
            action: WorkflowBoardChallengeStepAction::Resolve,
            concerns: None,
            acknowledgement_token: None,
            unresolved_concern_ids: None,
            answers: Some(WorkflowBoardChallengeStepAnswers::Responses(vec![
                WorkflowBoardChallengeConcernResponse {
                    concern_id: "CON-999".to_owned(),
                    disposition: WorkflowBoardChallengeDisposition::Accept,
                    response: None,
                    reason: None,
                },
            ])),
            approved: None,
        })
        .await
        .expect_err("challengeStep resolve without prior start must fail");
    assert_eq!(
        resolve_err.code,
        WorkflowBoardRequestErrorCode::InvalidInput
    );
    Ok(())
}
