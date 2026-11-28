#![forbid(unsafe_code)]
//! Integration tests for M1.3 `ProjectWorkflowBoardService`:
//! (a) card/spec/approval + `.legion/workflows/<card-id>/` without git auto-commit + Plan brief -> devReplay thread
//! (b) SQLite reopen / host restart preserves approved revision & digest
//! (c) sequential & concurrent double `startStage("plan")` idempotency (1 thread)
//! (d) `workflow_board_write_journal` crash recovery & orphaned `.tmp` sweep

mod workflow_board_support;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use legion_contracts::ids::{ProjectId, WorkflowCardId};
use legion_contracts::orchestration::{PatchValue, ProviderInteractionMode, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardApprovalStatus, WorkflowBoardApproveSpecInput,
    WorkflowBoardCreateCardInput, WorkflowBoardGetCardInput, WorkflowBoardRequestErrorCode,
    WorkflowBoardStageRunStage, WorkflowBoardStartStageInput, WorkflowBoardUpdateSpecInput,
};
use legion_orchestrator::outbox::OutboxWorkerOptions;
use legion_server_runtime::{RuntimeOutboxExecutor, reconcile_incomplete_write_journal};
use rusqlite::params;

use workflow_board_support::{
    TestResult, create_and_approve_card, dev_replay_selection, open_board_stack, run_git,
    setup_workspace, verify_journal_recovery_and_external_edit, verify_start_stage_idempotency,
};

fn make_create_input(
    project_id: ProjectId,
    title: &str,
    brief: Option<&str>,
    brief_path: Option<&str>,
) -> WorkflowBoardCreateCardInput {
    WorkflowBoardCreateCardInput {
        project_id,
        title: title.to_owned(),
        brief: brief.map(str::to_owned),
        brief_path: brief_path.map(str::to_owned),
        input_paths: None,
        intake_goal: None,
        wireframes_enabled: None,
        wireframes_index: PatchValue::Absent,
        source_type: None,
        assistance_mode: None,
        workspace_strategy: None,
        model_selection: None,
        runtime_mode: None,
        interaction_mode: None,
    }
}

fn spec_with_acceptance(card_id: WorkflowCardId, revision: u64) -> WorkflowBoardUpdateSpecInput {
    WorkflowBoardUpdateSpecInput {
        card_id,
        expected_revision: revision,
        title: None,
        brief: None,
        intake_goal: None,
        input_paths: None,
        acceptance_criteria: Some(vec![WorkflowBoardAcceptanceCriterion {
            id: "AC-01".to_owned(),
            text: "Verify card behavior follows the approved spec".to_owned(),
        }]),
        constraints: None,
        source_refs: None,
        wireframes_index: PatchValue::Absent,
    }
}

#[tokio::test]
async fn m1_handoff_restart_preservation_and_idempotent_start_stage() -> TestResult {
    let (_tmp, paths, repo_str) = setup_workspace()?;
    let repo_path = Path::new(&repo_str);
    let head_before = run_git(repo_path, &["rev-parse", "HEAD"]);
    let (card_id, approved_rev, approved_digest) =
        create_and_approve_card(&paths, repo_path, &head_before).await?;

    let (orch2, wf_mgr2, board2) = open_board_stack(&paths, true)?;
    let outbox_exec = RuntimeOutboxExecutor::new(Arc::clone(&orch2));
    outbox_exec.set_settings_path(paths.settings_path.clone());
    let worker = orch2.spawn_outbox_worker(outbox_exec, OutboxWorkerOptions::default());

    let reloaded = board2.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card_id.clone(),
        project_id: None,
    })?;
    assert_eq!(reloaded.summary.revision, approved_rev);
    assert_eq!(
        reloaded
            .spec_approval
            .as_ref()
            .and_then(|a| a.spec_digest.clone()),
        approved_digest
    );

    Box::pin(verify_start_stage_idempotency(
        &orch2,
        &board2,
        &wf_mgr2,
        &card_id,
        approved_rev,
        repo_path,
        &head_before,
    ))
    .await?;
    worker.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn write_journal_crash_recovery_external_edits_and_confinement() -> TestResult {
    let (_tmp, paths, repo_str) = setup_workspace()?;
    let repo_path = Path::new(&repo_str);
    fs::write(repo_path.join(".env"), "SECRET=123\n")?;

    let (orch, _wf_mgr, board) = open_board_stack(&paths, true)?;
    let pid = ProjectId::new("proj-board-1")?;
    let env_err = board
        .create_card(&make_create_input(
            pid.clone(),
            "Secret Attempt",
            None,
            Some(".env"),
        ))
        .expect_err("must reject .env");
    assert_eq!(
        env_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    let created = board.create_card(&make_create_input(
        pid,
        "Recovery Card",
        Some("Original spec content"),
        None,
    ))?;
    let cid = created.summary.card_id.clone();
    let card_dir = repo_path.join(".legion/workflows").join(cid.as_str());
    let spec_path = card_dir.join("spec.md");
    let orig_spec = fs::read_to_string(&spec_path)?;

    let trav_err = board
        .get_card_detail(&WorkflowBoardGetCardInput {
            card_id: WorkflowCardId::new("../escape")?,
            project_id: None,
        })
        .expect_err("must reject traversal cardId");
    assert_eq!(
        trav_err.code,
        WorkflowBoardRequestErrorCode::InvalidArtifactKey
    );

    Box::pin(verify_journal_recovery_and_external_edit(
        &orch,
        &board,
        &cid,
        &card_dir,
        &spec_path,
        &orig_spec,
        created.summary.revision,
    ))
    .await
}

#[test]
fn create_card_failure_keeps_connection_healthy_and_recovers() -> TestResult {
    let (_tmp, paths, _repo_str) = setup_workspace()?;
    let (orch, _wf_mgr, board) = open_board_stack(&paths, true)?;
    let project_id = ProjectId::new("proj-board-1")?;

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute_batch(
            "CREATE TEMP TRIGGER block_wj_file_written
             BEFORE UPDATE ON workflow_board_write_journal
             WHEN NEW.status = 'file_written'
             BEGIN
                 SELECT RAISE(ABORT, 'injected finalize_staged_files failure');
             END;",
        )?;
    }

    let finalize_err = board
        .create_card(&make_create_input(
            project_id.clone(),
            "Finalize Failure Card",
            Some("Should commit DB before failing file_written"),
            None,
        ))
        .expect_err("injected finalize failure must error");
    assert_eq!(
        finalize_err.code,
        WorkflowBoardRequestErrorCode::InternalError
    );

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        assert!(
            conn.is_autocommit(),
            "connection must not leak an open transaction after finalize failure"
        );
        conn.execute_batch("DROP TRIGGER temp.block_wj_file_written;")?;
    }

    let healthy = board.create_card(&make_create_input(
        project_id.clone(),
        "Subsequent Healthy Card",
        None,
        None,
    ))?;
    assert_eq!(healthy.summary.title, "Subsequent Healthy Card");

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let report = reconcile_incomplete_write_journal(&conn, "2026-11-24T09:00:00Z")?;
        assert_eq!(report.committed, 2);
        conn.execute_batch(
            "CREATE TEMP TRIGGER block_wj_stage_insert
             BEFORE INSERT ON workflow_board_write_journal
             BEGIN
                 SELECT RAISE(ABORT, 'injected pre-commit staging failure');
             END;",
        )?;
    }

    let stage_err = board
        .create_card(&make_create_input(
            project_id.clone(),
            "Precommit Rollback Card",
            None,
            None,
        ))
        .expect_err("injected pre-commit staging failure must error");
    assert_eq!(stage_err.code, WorkflowBoardRequestErrorCode::InternalError);

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        assert!(
            conn.is_autocommit(),
            "connection must stay in autocommit after pre-commit rollback"
        );
        conn.execute_batch("DROP TRIGGER temp.block_wj_stage_insert;")?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM workflow_board_cards WHERE title = 'Precommit Rollback Card'",
            params![],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0, "pre-commit rollback must revert inserted card");
    }

    let missing = ProjectId::new("unknown-board-project")?;
    let err = board
        .get_board_snapshot(&missing)
        .expect_err("unknown project");
    assert_eq!(err.code, WorkflowBoardRequestErrorCode::ProjectNotFound);
    let snap = board.get_board_snapshot(&project_id)?;
    assert_eq!(snap.cards.len(), 2);
    Ok(())
}

#[tokio::test]
async fn noop_update_spec_after_external_edit_marks_approval_stale() -> TestResult {
    let (_tmp, paths, repo_str) = setup_workspace()?;
    let repo_path = Path::new(&repo_str);
    let (_orch, _wf_mgr, board) = open_board_stack(&paths, true)?;

    let created = board.create_card(&make_create_input(
        ProjectId::new("proj-board-1")?,
        "External Edit Noop Card",
        Some("Initial spec"),
        None,
    ))?;
    let cid = created.summary.card_id.clone();
    let error = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: created.summary.revision,
                expected_digest: None,
            },
            "client",
        )
        .await
        .expect_err("a new approval needs an acceptance criterion");
    assert_eq!(error.code, WorkflowBoardRequestErrorCode::SpecNotApproved);
    let unchanged = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(unchanged.summary.revision, created.summary.revision);
    assert!(unchanged.spec_approval.is_none());
    let specified = board
        .update_spec(&spec_with_acceptance(cid.clone(), created.summary.revision))
        .await?;
    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: specified.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        approved.spec_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );

    let spec_path = repo_path
        .join(".legion/workflows")
        .join(cid.as_str())
        .join("spec.md");
    fs::write(&spec_path, "# Externally modified spec\n")?;

    let updated = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: approved.revision,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: None,
            constraints: None,
            source_refs: None,
            wireframes_index: PatchValue::Absent,
        })
        .await?;
    assert_eq!(
        updated.spec_approval_status,
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    assert_eq!(
        updated.card.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    Ok(())
}

#[tokio::test]
async fn start_stage_resumes_from_persisted_stage_run_when_overrides_omitted() -> TestResult {
    let (tmp, paths, repo_str) = setup_workspace()?;
    let worktree_path = tmp.path().join("existing-plan-worktree");
    run_git(
        Path::new(&repo_str),
        &[
            "worktree",
            "add",
            "-b",
            "existing-plan",
            &worktree_path.to_string_lossy(),
            "HEAD",
        ],
    );
    let (orch, _wf_mgr, board) = open_board_stack(&paths, true)?;
    let outbox_exec = RuntimeOutboxExecutor::new(Arc::clone(&orch));
    outbox_exec.set_settings_path(paths.settings_path.clone());
    let worker = orch.spawn_outbox_worker(outbox_exec, OutboxWorkerOptions::default());

    let created = board.create_card(&make_create_input(
        ProjectId::new("proj-board-1")?,
        "Override Resume Card",
        Some("Card with no default model or worktree"),
        None,
    ))?;
    let cid = created.summary.card_id.clone();
    let specified = board
        .update_spec(&spec_with_acceptance(cid.clone(), created.summary.revision))
        .await?;
    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: specified.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;

    let first = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: approved.revision,
            stage: WorkflowBoardStageRunStage::Plan,
            task_id: None,
            stable_launch_key: None,
            model_selection: Some(dev_replay_selection()),
            runtime_mode: Some(RuntimeMode::FullAccess),
            interaction_mode: Some(ProviderInteractionMode::Default),
            workspace_strategy: Some(WorkspaceStrategy::ExistingWorktree {
                worktree_path: worktree_path.to_string_lossy().into_owned(),
                branch: None,
            }),
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    assert!(!first.resumed);

    let resumed = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: first.revision,
            stage: WorkflowBoardStageRunStage::Plan,
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
    assert!(resumed.resumed);
    assert_eq!(resumed.thread_id, first.thread_id);
    assert_eq!(resumed.run_id, first.run_id);

    Box::pin(workflow_board_support::verify_plan_workspace_binding(
        &board,
        &cid,
        first.revision,
        &repo_str,
        &worktree_path,
    ))
    .await?;

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "UPDATE workflow_board_cards SET workspace_strategy_json = '{bad' WHERE card_id = ?1",
            params![cid.as_str()],
        )?;
    }
    let bad_err = board
        .get_card_detail(&WorkflowBoardGetCardInput {
            card_id: cid,
            project_id: None,
        })
        .expect_err("malformed workspace_strategy_json must fail loudly");
    assert_eq!(bad_err.code, WorkflowBoardRequestErrorCode::InternalError);

    worker.shutdown().await?;
    Ok(())
}
