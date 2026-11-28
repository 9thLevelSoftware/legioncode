#![forbid(unsafe_code)]
//! M3/M5.1 `resumeCard(reconcileInterrupted: true)` reconciliation and `deliverable_stale` tests.

#[path = "workflow_board_m3_support/mod.rs"]
mod m3_support;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::orchestration::WorkspaceStrategy;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardApprovalStatus, WorkflowBoardApprovePlanInput,
    WorkflowBoardEvidenceMethod, WorkflowBoardGetCardInput, WorkflowBoardPauseCardInput,
    WorkflowBoardPlannedCheck, WorkflowBoardRequestErrorCode, WorkflowBoardResumeCardInput,
    WorkflowBoardReturnForRevisionInput, WorkflowBoardReturnTargetStage,
    WorkflowBoardShipBlockerKind, WorkflowBoardStageRunStage, WorkflowBoardStartStageInput,
};
use legion_persistence::workflow_board_repo::record_external_action;
use legion_persistence::workflow_board_repo::types_m3::RecordExternalActionInput;
use m3_support::{m3_task, setup_approved_m3_card};
use workflow_board_m2_support::{TestResult, setup_m2_board};

#[tokio::test]
async fn resume_card_reconcile_interrupted_preserves_deliverable_stale_and_rejects_active_card()
-> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let checks = vec![WorkflowBoardPlannedCheck {
        id: "chk-1".to_owned(),
        argv: vec!["git".to_owned(), "status".to_owned()],
        criterion_id: Some("AC-01".to_owned()),
        description: Some("Check status".to_owned()),
    }];
    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Automated,
        check_ids: Some(vec!["chk-1".to_owned()]),
        command: None,
        procedure: None,
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Root { branch: None }),
        vec![m3_task("TSK-1", vec!["src/**"], vec![], &[])],
        checks,
        acs,
    ))
    .await?;

    // Recording an interrupted action and calling `resumeCard` while still active (not paused)
    // must reject with `InvalidStageTransition` without reconciling the interrupted action.
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        record_external_action(
            &conn,
            &RecordExternalActionInput {
                card_id: cid.as_str(),
                id: "ext-active-1",
                action_kind: "git_push",
                target_id: None,
                stage_run_id: None,
                idempotency_key: "idem-active-1",
                status: "interrupted",
                expected_snapshot_digest: None,
                observed_snapshot_digest: None,
                details_json: "{\"summary\":\"Interrupted before pause\"}",
            },
            "2026-11-27T09:00:00Z",
        )?;
    }
    let active_err = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: rev,
            reconcile_interrupted: Some(true),
            note: Some("Should fail on active card".to_owned()),
        })
        .await
        .expect_err("resumeCard on non-paused card must fail");
    assert_eq!(
        active_err.code,
        WorkflowBoardRequestErrorCode::InvalidStageTransition
    );
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let status: String = conn.query_row(
            "SELECT status FROM workflow_board_external_actions WHERE id = 'ext-active-1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(status, "interrupted");
    }

    // Pause card and tamper with `plan.md` on disk, then resume with `reconcile_interrupted: true`.
    let paused = board
        .pause_card(&WorkflowBoardPauseCardInput {
            card_id: cid.clone(),
            expected_revision: rev,
            reason: Some("Pause before external tamper".to_owned()),
        })
        .await?;
    let plan_path = repo_dir
        .join(".legion/workflows")
        .join(cid.as_str())
        .join("plan.md");
    let original_plan = fs::read_to_string(&plan_path)?;
    fs::write(&plan_path, format!("{original_plan}\n<!-- tampered -->\n"))?;

    let resumed = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused.revision,
            reconcile_interrupted: Some(true),
            note: Some("Reconcile while plan.md tampered".to_owned()),
        })
        .await?;
    assert_eq!(resumed.reconciled_count, 1);
    assert_eq!(resumed.card.summary.has_unreconciled_edits, Some(true));
    assert!(
        resumed
            .card
            .recovery_state
            .as_ref()
            .is_some_and(|r| r.requires_reconciliation)
    );
    assert!(
        resumed
            .card
            .ship_readiness
            .as_ref()
            .is_some_and(|sr| sr.blockers.iter().any(|b| {
                b.id == "blocker-external-edit"
                    && b.kind == WorkflowBoardShipBlockerKind::ExternalEdit
            }))
    );
    assert_eq!(
        resumed.card.summary.plan_approval_status,
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let ext_state: Option<String> = conn.query_row(
            "SELECT external_edit_state FROM workflow_board_cards WHERE card_id = ?1",
            [cid.as_str()],
            |r| r.get(0),
        )?;
        assert_eq!(ext_state.as_deref(), Some("deliverable_stale"));
    }

    // Subsequent `startStage(Execute)` must still fail while deliverables remain stale.
    let blocked_exec = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: resumed.revision,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: Some("TSK-1".to_owned()),
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await
        .expect_err("stale deliverables after resume must block startStage(Execute)");
    assert!(matches!(
        blocked_exec.code,
        WorkflowBoardRequestErrorCode::PlanNotApproved
            | WorkflowBoardRequestErrorCode::ExternalEditDetected
    ));

    // Restoring `plan.md` to persisted contents and pausing + resuming clears `deliverable_stale`.
    fs::write(&plan_path, &original_plan)?;
    let paused2 = board
        .pause_card(&WorkflowBoardPauseCardInput {
            card_id: cid.clone(),
            expected_revision: resumed.revision,
            reason: Some("Pause after restore".to_owned()),
        })
        .await?;
    let resumed2 = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused2.revision,
            reconcile_interrupted: Some(true),
            note: Some("Restored plan.md".to_owned()),
        })
        .await?;
    assert_eq!(resumed2.card.summary.has_unreconciled_edits, Some(false));
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let ext_state: Option<String> = conn.query_row(
            "SELECT external_edit_state FROM workflow_board_cards WHERE card_id = ?1",
            [cid.as_str()],
            |r| r.get(0),
        )?;
        assert_eq!(ext_state.as_deref(), Some("clean"));
    }

    // Return card to `plan` and verify `approvePlan` catches both `expected_plan_digest`
    // and disk artifact `DigestMismatch` before succeeding when clean.
    let returned = board
        .return_for_revision(&WorkflowBoardReturnForRevisionInput {
            card_id: cid.clone(),
            expected_revision: resumed2.revision,
            target_stage: WorkflowBoardReturnTargetStage::Plan,
            reason: "Verify plan re-approval after restore".to_owned(),
        })
        .await?;
    let mismatch_err = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: returned.revision,
                expected_spec_digest: None,
                expected_plan_digest: Some("sha256:wrong".to_owned()),
            },
            "client",
        )
        .await
        .expect_err("wrong expected_plan_digest must fail");
    assert_eq!(
        mismatch_err.code,
        WorkflowBoardRequestErrorCode::DigestMismatch
    );

    fs::write(
        &plan_path,
        format!("{original_plan}\n<!-- tampered again -->\n"),
    )?;
    let disk_mismatch_err = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: returned.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await
        .expect_err("tampered plan.md on disk must fail approvePlan with DigestMismatch");
    assert_eq!(
        disk_mismatch_err.code,
        WorkflowBoardRequestErrorCode::DigestMismatch
    );

    fs::write(&plan_path, &original_plan)?;
    let reapproved = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: returned.revision,
                expected_spec_digest: None,
                expected_plan_digest: returned.card.plan.and_then(|p| p.plan_digest),
            },
            "client",
        )
        .await?;
    assert_eq!(
        reapproved.card.summary.plan_approval_status,
        Some(WorkflowBoardApprovalStatus::Valid)
    );
    assert_eq!(reapproved.card.summary.has_unreconciled_edits, Some(false));
    let _ = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid,
        project_id: None,
    })?;
    Ok(())
}
