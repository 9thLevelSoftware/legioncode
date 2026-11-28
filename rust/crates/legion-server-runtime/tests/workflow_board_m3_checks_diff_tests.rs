#![forbid(unsafe_code)]
//! M3 planned `argv` check runner, credential scrubbing, snapshot digest binding & auto-staleness,
//! `.legion/workflows/` exclusion, `getCardDiff`, and interrupted-action recovery tests.

#[path = "workflow_board_m3_support/mod.rs"]
mod m3_support;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::orchestration::WorkspaceStrategy;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAmendmentTargetStage, WorkflowBoardCheckStatus,
    WorkflowBoardEvidenceMethod, WorkflowBoardGetCardDiffInput, WorkflowBoardGetCardInput,
    WorkflowBoardPauseCardInput, WorkflowBoardPlannedCheck, WorkflowBoardProposeAmendmentInput,
    WorkflowBoardRequestErrorCode, WorkflowBoardResumeCardInput, WorkflowBoardRetryStepInput,
    WorkflowBoardRetryStepKind, WorkflowBoardStageRunStage, WorkflowBoardStartStageInput,
};
use legion_persistence::workflow_board_repo::record_external_action;
use legion_persistence::workflow_board_repo::types_m3::RecordExternalActionInput;
use m3_support::{m3_task, setup_approved_m3_card};
use workflow_board_m2_support::{TestResult, setup_m2_board};

#[tokio::test]
async fn planned_argv_checks_scrub_env_bind_snapshots_and_exclude_board_metadata() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    let script_path = repo_dir.join("check_env.mjs");
    fs::write(
        &script_path,
        "const leaked = Object.keys(process.env).filter(k => k.startsWith('OPENAI_') || k.startsWith('ANTHROPIC_') || k.startsWith('LEGION_') || k === 'AWS_SECRET_ACCESS_KEY');\n\
         if (leaked.length) { console.error('LEAKED:' + leaked.join(',')); process.exit(2); }\n\
         console.log('SCRUB_OK');\n",
    )?;

    let checks = vec![WorkflowBoardPlannedCheck {
        id: "chk-scrub".to_owned(),
        argv: vec!["node".to_owned(), "check_env.mjs".to_owned()],
        criterion_id: Some("AC-01".to_owned()),
        description: Some("Verify credential scrubbing".to_owned()),
    }];
    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Automated,
        check_ids: Some(vec!["chk-scrub".to_owned()]),
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

    // Note: unsafe `set_var` avoided; verify `PATH` / safe base env works and check output is scrubbed + receipt bound.
    let after_check = board
        .run_planned_check(cid.as_str(), "chk-scrub", Some(rev))
        .await?;
    let receipt = after_check
        .check_receipts
        .iter()
        .find(|r| r.check_id == "chk-scrub")
        .ok_or("missing check receipt")?;
    assert_eq!(receipt.status, WorkflowBoardCheckStatus::Passed);
    assert_eq!(receipt.exit_code, Some(0));
    assert!(
        receipt
            .log_ref
            .as_deref()
            .is_some_and(|p| p.starts_with("logs/"))
    );
    let snap1 = receipt
        .product_snapshot_digest
        .clone()
        .ok_or("missing snapshot digest")?;
    assert!(snap1.starts_with("sha256:"));

    // Writing inside `.legion/workflows/<card-id>/` must NOT stale the check receipt.
    let card_dir = repo_dir.join(".legion/workflows").join(cid.as_str());
    fs::write(card_dir.join("notes.tmp.md"), "board metadata note\n")?;
    let after_meta = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(
        after_meta
            .check_receipts
            .iter()
            .find(|r| r.check_id == "chk-scrub")
            .map(|r| r.status),
        Some(WorkflowBoardCheckStatus::Passed)
    );

    // Modifying a product file in the worktree changes `product_snapshot_digest`, marks receipt `stale`,
    // and appears in `getCardDiff`.
    fs::create_dir_all(repo_dir.join("src"))?;
    fs::write(repo_dir.join("src/product.ts"), "export const x = 1;\n")?;
    let diff = board.get_card_diff(&WorkflowBoardGetCardDiffInput {
        card_id: cid.clone(),
        task_id: None,
        include_patches: Some(true),
    })?;
    assert!(diff.files.iter().any(|f| f.path == "src/product.ts"));
    assert_ne!(
        diff.product_snapshot_digest.as_deref(),
        Some(snap1.as_str())
    );

    let after_prod = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(
        after_prod
            .check_receipts
            .iter()
            .find(|r| r.check_id == "chk-scrub")
            .map(|r| r.status),
        Some(WorkflowBoardCheckStatus::Stale)
    );

    // Interrupted external action blocks Execute until `resumeCard(reconcileInterrupted: true)` reconciles it.
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        record_external_action(
            &conn,
            &RecordExternalActionInput {
                card_id: cid.as_str(),
                id: "ext-int-1",
                action_kind: "git_push",
                target_id: None,
                stage_run_id: None,
                idempotency_key: "idem-chk-1",
                status: "interrupted",
                expected_snapshot_digest: Some(&snap1),
                observed_snapshot_digest: None,
                details_json: "{\"summary\":\"Interrupted action\"}",
            },
            "2026-11-24T12:00:00Z",
        )?;
    }
    let blocked_start = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: after_prod.summary.revision,
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
        .expect_err("interrupted external action must block startStage");
    assert_eq!(
        blocked_start.code,
        WorkflowBoardRequestErrorCode::ExternalEditDetected
    );

    let blocked_retry = board
        .retry_step(&WorkflowBoardRetryStepInput {
            card_id: cid.clone(),
            expected_revision: after_prod.summary.revision,
            step_kind: WorkflowBoardRetryStepKind::Check,
            target_id: Some("chk-scrub".to_owned()),
            reason: Some("Retry while interrupted".to_owned()),
        })
        .await
        .expect_err("interrupted external action must block retryStep");
    assert_eq!(
        blocked_retry.code,
        WorkflowBoardRequestErrorCode::ExternalEditDetected
    );

    let blocked_amend = board
        .propose_amendment(
            &WorkflowBoardProposeAmendmentInput {
                card_id: cid.clone(),
                expected_revision: after_prod.summary.revision,
                target_stage: Some(WorkflowBoardAmendmentTargetStage::Plan),
                reason: "Amend while interrupted".to_owned(),
                summary: None,
                source_task_id: None,
                unexpected_files: None,
                tasks: None,
                planned_checks: None,
                ac_mappings: None,
                deliverables: None,
            },
            "client",
        )
        .await
        .expect_err("interrupted external action must block proposeAmendment");
    assert_eq!(
        blocked_amend.code,
        WorkflowBoardRequestErrorCode::ExternalEditDetected
    );

    let paused = board
        .pause_card(&WorkflowBoardPauseCardInput {
            card_id: cid.clone(),
            expected_revision: after_prod.summary.revision,
            reason: Some("Reconcile".to_owned()),
        })
        .await?;
    let blocked_resume = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused.revision,
            reconcile_interrupted: Some(false),
            note: None,
        })
        .await
        .expect_err("unreconciled interrupted action must block resumeCard");
    assert_eq!(
        blocked_resume.code,
        WorkflowBoardRequestErrorCode::ExternalEditDetected
    );

    let resumed = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused.revision,
            reconcile_interrupted: Some(true),
            note: Some("Reconciled external action".to_owned()),
        })
        .await?;
    assert_eq!(resumed.reconciled_count, 1);

    verify_journal_resume_and_missing_retry_errors(&orch, &board, &cid, resumed.revision).await
}

async fn verify_journal_resume_and_missing_retry_errors(
    orch: &legion_orchestrator::Orchestrator,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    active_rev: u64,
) -> TestResult {
    let paused2 = board
        .pause_card(&WorkflowBoardPauseCardInput {
            card_id: cid.clone(),
            expected_revision: active_rev,
            reason: Some("Test incomplete write journal".to_owned()),
        })
        .await?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "INSERT INTO workflow_board_write_journal (id, card_id, target_path, operation, status, created_at, updated_at)
             VALUES ('j-inc-1', ?1, 'notes.md', 'write', 'pending', '2026-11-24T12:00:00Z', '2026-11-24T12:00:00Z')",
            [cid.as_str()],
        )?;
    }
    let blocked_journal_resume = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused2.revision,
            reconcile_interrupted: Some(false),
            note: None,
        })
        .await
        .expect_err("incomplete write journal must block resumeCard");
    assert_eq!(
        blocked_journal_resume.code,
        WorkflowBoardRequestErrorCode::ExternalEditDetected
    );
    let resumed2 = board
        .resume_card(&WorkflowBoardResumeCardInput {
            card_id: cid.clone(),
            expected_revision: paused2.revision,
            reconcile_interrupted: Some(true),
            note: Some("Reconcile journal".to_owned()),
        })
        .await?;
    assert_eq!(resumed2.reconciled_count, 1);

    for (kind, target) in [
        (WorkflowBoardRetryStepKind::Check, "missing-chk"),
        (WorkflowBoardRetryStepKind::Task, "missing-task"),
    ] {
        let missing_err = board
            .retry_step(&WorkflowBoardRetryStepInput {
                card_id: cid.clone(),
                expected_revision: resumed2.revision,
                step_kind: kind,
                target_id: Some(target.to_owned()),
                reason: Some("Missing target".to_owned()),
            })
            .await
            .expect_err("missing retry target must fail with InvalidInput");
        assert_eq!(
            missing_err.code,
            WorkflowBoardRequestErrorCode::InvalidInput
        );
    }
    Ok(())
}
