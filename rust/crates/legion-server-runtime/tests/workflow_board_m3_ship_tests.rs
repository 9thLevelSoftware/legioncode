#![forbid(unsafe_code)]
//! M3 Pause/Resume, fresh-thread independent Review, human `resolveFinding`,
//! `recordManualAcceptance`, and client-only zero-auto-commit `approveDelivery` tests.

#[path = "workflow_board_m3_support/mod.rs"]
mod m3_support;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use legion_contracts::orchestration::WorkspaceStrategy;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardApproveDeliveryInput, WorkflowBoardCardStatus,
    WorkflowBoardDeliveryStatus, WorkflowBoardEvidenceMethod,
    WorkflowBoardFindingResolveDisposition, WorkflowBoardManualAcceptanceVerdict,
    WorkflowBoardPauseCardInput, WorkflowBoardRecordManualAcceptanceInput,
    WorkflowBoardRequestErrorCode, WorkflowBoardResolveFindingInput, WorkflowBoardResumeCardInput,
    WorkflowBoardStage, WorkflowBoardStageRunStage, WorkflowBoardStartStageInput,
};
use m3_support::{m3_task, setup_approved_m3_card};
use workflow_board_m2_support::{TestResult, setup_m2_board};

fn git_head(dir: &Path) -> String {
    let out = StdCommand::new("git")
        .current_dir(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
async fn pause_resume_fresh_thread_review_and_client_only_ship_without_autocommit() -> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let head_before = git_head(repo_dir);

    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Manual,
        check_ids: None,
        command: None,
        procedure: Some("Inspect output".to_owned()),
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Root { branch: None }),
        vec![m3_task("TSK-1", vec!["src/a.txt"], vec![], &[])],
        Vec::new(),
        acs,
    ))
    .await?;

    // Pause and resume during Execute.
    let paused = board
        .pause_card(&WorkflowBoardPauseCardInput {
            card_id: cid.clone(),
            expected_revision: rev,
            reason: Some("Operator pause".to_owned()),
        })
        .await?;
    assert_eq!(paused.status, WorkflowBoardCardStatus::Paused);
    let resumed = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused.revision,
            reconcile_interrupted: None,
            note: Some("Continue".to_owned()),
        })
        .await?;
    assert_eq!(resumed.status, WorkflowBoardCardStatus::Active);

    // Execute TSK-1 on thread A.
    let exec_start = board
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
        .await?;
    fs::create_dir_all(repo_dir.join("src"))?;
    fs::write(repo_dir.join("src/a.txt"), "feature implemented\n")?;
    let exec_done = board
        .complete_task_from_stage(
            cid.as_str(),
            exec_start.stage_run_id.as_str(),
            "TSK-1",
            "completed",
            None,
        )
        .await?;

    // Start independent Review -> MUST launch a fresh thread != execute thread.
    let rev_start = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: exec_done.summary.revision,
            stage: WorkflowBoardStageRunStage::Review,
            task_id: None,
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    assert_ne!(rev_start.thread_id, exec_start.thread_id);

    let rev_recorded = board
        .record_review_from_stage(
            cid.as_str(),
            rev_start.stage_run_id.as_str(),
            "findings",
            Some("One finding"),
            &[(
                "f-1".to_owned(),
                "minor".to_owned(),
                Some("AC-01".to_owned()),
                Some("src/a.txt".to_owned()),
                Some(1),
                "Verify manually".to_owned(),
            )],
        )
        .await?;
    verify_ship_gates_and_approve_delivery(&board, &cid, &rev_recorded, repo_dir, &head_before)
        .await
}

async fn verify_ship_gates_and_approve_delivery(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    rev_recorded: &legion_contracts::workflow_board::WorkflowCardDetail,
    repo_dir: &Path,
    head_before: &str,
) -> TestResult {
    let readiness_blocked = rev_recorded
        .ship_readiness
        .as_ref()
        .ok_or("missing ship_readiness")?;
    assert!(!readiness_blocked.ready);
    assert!(
        readiness_blocked
            .blockers
            .iter()
            .any(|b| b.pointer.tab == "review")
    );

    // Non-client caller is refused on resolveFinding, recordManualAcceptance, and approveDelivery.
    let agent_res_err = board
        .resolve_finding(
            &WorkflowBoardResolveFindingInput {
                card_id: cid.clone(),
                expected_revision: rev_recorded.summary.revision,
                review_id: None,
                finding_id: "f-1".to_owned(),
                disposition: WorkflowBoardFindingResolveDisposition::AcceptRisk,
                reason: Some("Agent trying to self-approve".to_owned()),
            },
            "thread:thr-agent",
        )
        .await
        .expect_err("agent must not resolve finding");
    assert_eq!(
        agent_res_err.code,
        WorkflowBoardRequestErrorCode::InvalidStageTransition
    );

    assert_invalid_finding_and_review_id(board, cid, rev_recorded.summary.revision).await?;

    let review_id = rev_recorded
        .review_report
        .as_ref()
        .map(|r| r.review_id.clone());
    let resolved = board
        .resolve_finding(
            &WorkflowBoardResolveFindingInput {
                card_id: cid.clone(),
                expected_revision: rev_recorded.summary.revision,
                review_id,
                finding_id: "f-1".to_owned(),
                disposition: WorkflowBoardFindingResolveDisposition::AcceptRisk,
                reason: Some("Accepted by operator".to_owned()),
            },
            "client",
        )
        .await?;

    assert_unknown_ac_rejected(board, cid, resolved.revision).await?;

    let accepted = board
        .record_manual_acceptance(
            &WorkflowBoardRecordManualAcceptanceInput {
                card_id: cid.clone(),
                expected_revision: resolved.revision,
                criterion_id: "AC-01".to_owned(),
                verdict: WorkflowBoardManualAcceptanceVerdict::Pass,
                note: Some("Verified output".to_owned()),
                evidence_ref: None,
                evidence_digest: None,
                expected_product_snapshot_digest: None,
            },
            "client",
        )
        .await?;
    assert!(accepted.ship_readiness.ready);

    let delivered = board
        .approve_delivery(
            &WorkflowBoardApproveDeliveryInput {
                card_id: cid.clone(),
                expected_revision: accepted.revision,
                expected_product_snapshot_digest: None,
                note: Some("Ready to ship".to_owned()),
            },
            "client",
        )
        .await?;
    assert_eq!(delivered.status, WorkflowBoardCardStatus::Completed);
    assert_eq!(
        delivered.delivery_status,
        WorkflowBoardDeliveryStatus::Shipped
    );
    assert_eq!(delivered.card.summary.stage, WorkflowBoardStage::Ship);
    assert_eq!(
        git_head(repo_dir),
        head_before,
        "approveDelivery must NEVER auto-commit"
    );
    Ok(())
}

async fn assert_invalid_finding_and_review_id(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    expected_revision: u64,
) -> TestResult {
    let unknown_finding_err = board
        .resolve_finding(
            &WorkflowBoardResolveFindingInput {
                card_id: cid.clone(),
                expected_revision,
                review_id: None,
                finding_id: "f-unknown".to_owned(),
                disposition: WorkflowBoardFindingResolveDisposition::Dismiss,
                reason: Some("Missing finding".to_owned()),
            },
            "client",
        )
        .await
        .expect_err("unknown finding_id must return InvalidInput");
    assert_eq!(
        unknown_finding_err.code,
        WorkflowBoardRequestErrorCode::InvalidInput
    );

    let mismatched_review_err = board
        .resolve_finding(
            &WorkflowBoardResolveFindingInput {
                card_id: cid.clone(),
                expected_revision,
                review_id: Some("wrong-review-id".to_owned()),
                finding_id: "f-1".to_owned(),
                disposition: WorkflowBoardFindingResolveDisposition::AcceptRisk,
                reason: Some("Wrong review run".to_owned()),
            },
            "client",
        )
        .await
        .expect_err("mismatched review_id must return InvalidInput");
    assert_eq!(
        mismatched_review_err.code,
        WorkflowBoardRequestErrorCode::InvalidInput
    );
    Ok(())
}

async fn assert_unknown_ac_rejected(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    expected_revision: u64,
) -> TestResult {
    let unknown_ac_err = board
        .record_manual_acceptance(
            &WorkflowBoardRecordManualAcceptanceInput {
                card_id: cid.clone(),
                expected_revision,
                criterion_id: "AC-UNKNOWN".to_owned(),
                verdict: WorkflowBoardManualAcceptanceVerdict::Pass,
                note: Some("Unknown AC".to_owned()),
                evidence_ref: None,
                evidence_digest: None,
                expected_product_snapshot_digest: None,
            },
            "client",
        )
        .await
        .expect_err("unknown criterion_id must return InvalidInput");
    assert_eq!(
        unknown_ac_err.code,
        WorkflowBoardRequestErrorCode::InvalidInput
    );
    Ok(())
}

#[tokio::test]
async fn blocked_approve_delivery_does_not_return_plan_not_approved() -> TestResult {
    let (_tmp, _orch, board, _repo_str) = setup_m2_board()?;
    let checks = vec![
        legion_contracts::workflow_board::WorkflowBoardPlannedCheck {
            id: "CHK-01".to_owned(),
            argv: vec![
                "node".to_owned(),
                "-e".to_owned(),
                "process.exit(0)".to_owned(),
            ],
            criterion_id: Some("AC-01".to_owned()),
            description: Some("Planned check".to_owned()),
        },
    ];
    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Automated,
        check_ids: Some(vec!["CHK-01".to_owned()]),
        command: None,
        procedure: None,
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Root { branch: None }),
        vec![m3_task("TSK-1", vec!["src/a.txt"], vec![], &[])],
        checks,
        acs,
    ))
    .await?;

    let err = board
        .approve_delivery(
            &WorkflowBoardApproveDeliveryInput {
                card_id: cid,
                expected_revision: rev,
                expected_product_snapshot_digest: None,
                note: None,
            },
            "client",
        )
        .await
        .expect_err("delivery with missing Planned check CHK-01 must fail");
    assert_ne!(err.code, WorkflowBoardRequestErrorCode::PlanNotApproved);
    assert_ne!(err.code, WorkflowBoardRequestErrorCode::SpecNotApproved);
    assert_eq!(
        err.code,
        WorkflowBoardRequestErrorCode::InvalidStageTransition
    );
    Ok(())
}
