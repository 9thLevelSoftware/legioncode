#![forbid(unsafe_code)]
//! M3 Execute stage tests: worktree backfill & reuse, single-mutating task DAG enforcement,
//! post-turn `filesAllowed`/`filesForbidden` diff validation preserving unexpected edits,
//! and `proposeAmendment` / `retryStep` unblocking.

#[path = "workflow_board_m3_support/execute_remediation.rs"]
mod execute_remediation;
#[path = "workflow_board_m3_support/mod.rs"]
mod m3_support;
#[path = "workflow_board_m2_support/mod.rs"]
mod workflow_board_m2_support;

use std::fs;
use std::path::Path;

use legion_contracts::orchestration::WorkspaceStrategy;
use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAmendmentTargetStage,
    WorkflowBoardApprovePlanInput, WorkflowBoardEvidenceMethod, WorkflowBoardFileContractStatus,
    WorkflowBoardGetArtifactInput, WorkflowBoardGetCardInput, WorkflowBoardProposeAmendmentInput,
    WorkflowBoardRequestErrorCode, WorkflowBoardStage, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput, WorkflowBoardTaskStatus,
};
use m3_support::{m3_task, setup_approved_m3_card};
use workflow_board_m2_support::{TestResult, setup_m2_board};

#[tokio::test]
async fn execute_worktree_backfill_dag_single_mutating_task_and_contract_violation_preservation()
-> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let _repo_dir = Path::new(&repo_str);
    let tasks = vec![
        m3_task("TSK-1", vec!["src/a.txt"], vec!["secrets/**"], &[]),
        m3_task("TSK-2", vec!["src/b.txt"], vec!["secrets/**"], &["TSK-1"]),
        m3_task("TSK-3", vec!["src/c.txt"], vec![], &[]),
    ];
    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Manual,
        check_ids: None,
        command: None,
        procedure: Some("Verify tasks".to_owned()),
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Worktree {
            base_ref: "HEAD".to_owned(),
            branch: None,
            start_from_origin: None,
            repositories: None,
        }),
        tasks,
        Vec::new(),
        acs,
    ))
    .await?;

    // Start TSK-1 (with WorkspaceStrategy::Worktree -> backfills worktree_path on card).
    let started_t1 = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: rev,
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
    assert_eq!(started_t1.task_id.as_deref(), Some("TSK-1"));
    assert!(!started_t1.resumed);

    let detail_after_t1 = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let card_wt = detail_after_t1
        .stage_runs
        .iter()
        .find(|sr| sr.id == started_t1.stage_run_id)
        .and_then(|sr| sr.worktree_path.clone())
        .ok_or("missing worktree_path on execute stage run")?;
    let wt_dir = Path::new(&card_wt);
    assert!(wt_dir.is_dir());

    // Starting dependent TSK-2 or concurrent TSK-3 while TSK-1 is in_progress must fail.
    for other_task in ["TSK-2", "TSK-3"] {
        let err = board
            .start_stage(&WorkflowBoardStartStageInput {
                card_id: cid.clone(),
                expected_revision: started_t1.revision,
                stage: WorkflowBoardStageRunStage::Execute,
                task_id: Some(other_task.to_owned()),
                stable_launch_key: None,
                model_selection: None,
                runtime_mode: None,
                interaction_mode: None,
                workspace_strategy: None,
                acknowledge_unsupported_policies: None,
                acknowledged_policies: None,
            })
            .await
            .expect_err("concurrent or blocked task must be rejected");
        assert_eq!(err.code, WorkflowBoardRequestErrorCode::ContractConflict);
    }

    // Complete TSK-1 cleanly (only allowed file touched).
    fs::create_dir_all(wt_dir.join("src"))?;
    fs::write(wt_dir.join("src/a.txt"), "clean edit\n")?;
    let t1_done = board
        .complete_task_from_stage(
            cid.as_str(),
            started_t1.stage_run_id.as_str(),
            "TSK-1",
            "completed",
            None,
        )
        .await?;
    assert_eq!(
        t1_done
            .task_executions
            .iter()
            .find(|t| t.task_id == "TSK-1")
            .map(|t| t.status),
        Some(WorkflowBoardTaskStatus::Completed)
    );
    assert_eq!(
        t1_done
            .file_contract_reports
            .iter()
            .find(|r| r.task_id == "TSK-1")
            .map(|r| r.status),
        Some(WorkflowBoardFileContractStatus::Clean)
    );

    // Start TSK-2 and verify it reuses the EXACT same card worktree.
    let started_t2 = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: t1_done.summary.revision,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: Some("TSK-2".to_owned()),
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    let detail_t2 = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let t2_wt = detail_t2
        .stage_runs
        .iter()
        .find(|sr| sr.id == started_t2.stage_run_id)
        .and_then(|sr| sr.worktree_path.clone())
        .ok_or("missing worktree_path on TSK-2")?;
    assert_eq!(t2_wt, card_wt);

    // Modify both allowed `src/b.txt` and unexpected `src/unexpected.txt` -> blocked & preserved.
    fs::write(wt_dir.join("src/b.txt"), "b edit\n")?;
    fs::write(wt_dir.join("src/unexpected.txt"), "preserve-me\n")?;
    let viol_detail = board
        .complete_task_from_stage(
            cid.as_str(),
            started_t2.stage_run_id.as_str(),
            "TSK-2",
            "completed",
            None,
        )
        .await?;
    assert_eq!(
        viol_detail.summary.next_action.as_deref(),
        Some("resolve_contract_violation")
    );
    let report = viol_detail
        .file_contract_reports
        .iter()
        .find(|r| r.task_id == "TSK-2")
        .ok_or("missing TSK-2 violation report")?;
    assert_eq!(
        report.status,
        WorkflowBoardFileContractStatus::ViolationPending
    );
    assert!(report.preserved_in_worktree);
    assert!(
        report
            .unexpected_files
            .iter()
            .any(|f| f == "src/unexpected.txt")
    );
    assert_eq!(
        fs::read_to_string(wt_dir.join("src/unexpected.txt"))?,
        "preserve-me\n"
    );

    // Propose a plan amendment expanding TSK-2's allowed files -> routes to Plan with stale plan approval.
    let amended = board
        .propose_amendment(
            &WorkflowBoardProposeAmendmentInput {
                card_id: cid.clone(),
                expected_revision: viol_detail.summary.revision,
                target_stage: Some(WorkflowBoardAmendmentTargetStage::Plan),
                reason: "Include src/unexpected.txt in TSK-2".to_owned(),
                summary: Some("Expand TSK-2 scope".to_owned()),
                source_task_id: Some("TSK-2".to_owned()),
                unexpected_files: Some(vec!["src/unexpected.txt".to_owned()]),
                tasks: Some(vec![
                    m3_task("TSK-1", vec!["src/a.txt"], vec!["secrets/**"], &[]),
                    m3_task(
                        "TSK-2",
                        vec!["src/b.txt", "src/unexpected.txt"],
                        vec!["secrets/**"],
                        &["TSK-1"],
                    ),
                ]),
                planned_checks: None,
                ac_mappings: None,
                deliverables: None,
            },
            "client",
        )
        .await?;
    assert_eq!(amended.stage, WorkflowBoardStage::Plan);
    assert_eq!(
        amended.plan_approval.as_ref().map(|a| a.status),
        Some(legion_contracts::workflow_board::WorkflowBoardApprovalStatus::Stale)
    );

    // Approve amended plan and start TSK-2: TSK-1 must remain Completed, TSK-2 starts InProgress.
    let reapproved = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: amended.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        reapproved
            .card
            .tasks
            .iter()
            .find(|t| t.task_id == "TSK-1")
            .and_then(|t| t.status),
        Some(WorkflowBoardTaskStatus::Completed)
    );
    let resumed_t2 = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: reapproved.revision,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: Some("TSK-2".to_owned()),
            stable_launch_key: None,
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    assert_eq!(resumed_t2.task_id.as_deref(), Some("TSK-2"));
    let after_t2_start = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid,
        project_id: None,
    })?;
    assert_eq!(
        after_t2_start
            .task_executions
            .iter()
            .find(|t| t.task_id == "TSK-1")
            .map(|t| t.status),
        Some(WorkflowBoardTaskStatus::Completed)
    );
    assert_eq!(
        after_t2_start
            .task_executions
            .iter()
            .find(|t| t.task_id == "TSK-2")
            .map(|t| t.status),
        Some(WorkflowBoardTaskStatus::InProgress)
    );
    Ok(())
}

#[tokio::test]
async fn dirty_main_checkout_does_not_cause_false_contract_conflict_on_new_worktree() -> TestResult
{
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    let repo_dir = Path::new(&repo_str);
    // Dirty and untracked files in main project checkout before card launches Worktree.
    fs::write(repo_dir.join("README.md"), "# Dirty main checkout\n")?;
    fs::write(repo_dir.join("untracked_main.tmp"), "ignore me\n")?;

    let acs = vec![WorkflowBoardAcEvidenceMapping {
        criterion_id: "AC-01".to_owned(),
        method: WorkflowBoardEvidenceMethod::Manual,
        check_ids: None,
        command: None,
        procedure: Some("Verify clean worktree".to_owned()),
        external_ref: None,
        external_source: None,
        limitations: None,
    }];
    let (cid, rev) = Box::pin(setup_approved_m3_card(
        &board,
        Some(WorkspaceStrategy::Worktree {
            base_ref: "HEAD".to_owned(),
            branch: None,
            start_from_origin: None,
            repositories: None,
        }),
        vec![m3_task("TSK-1", vec!["src/a.txt"], vec![], &[])],
        Vec::new(),
        acs,
    ))
    .await?;

    let started = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: rev,
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
    let detail = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let wt_path = detail
        .stage_runs
        .iter()
        .find(|sr| sr.id == started.stage_run_id)
        .and_then(|sr| sr.worktree_path.clone())
        .ok_or("missing worktree_path")?;
    let brief = board.get_artifact(&WorkflowBoardGetArtifactInput {
        card_id: cid.clone(),
        key: Some("execute-brief.md".to_owned()),
        kind: None,
    })?;
    assert!(
        brief.content.contains(&wt_path),
        "execute-brief.md must record materialized worktree path `{wt_path}`"
    );

    let wt_dir = Path::new(&wt_path);
    fs::create_dir_all(wt_dir.join("src"))?;
    fs::write(wt_dir.join("src/a.txt"), "clean worktree edit\n")?;
    let completed = board
        .complete_task_from_stage(
            cid.as_str(),
            started.stage_run_id.as_str(),
            "TSK-1",
            "completed",
            None,
        )
        .await?;
    assert_eq!(
        completed
            .file_contract_reports
            .iter()
            .find(|r| r.task_id == "TSK-1")
            .map(|r| r.status),
        Some(WorkflowBoardFileContractStatus::Clean)
    );
    Ok(())
}
