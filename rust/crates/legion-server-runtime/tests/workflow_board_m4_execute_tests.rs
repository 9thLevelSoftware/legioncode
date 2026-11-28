#![forbid(unsafe_code)]
//! Regression tests for M4 CLI-imported `execute`-stage task claim & preflight alignment.

#[path = "workflow_board_m2_support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use legion_contracts::ids::ProjectId;
use legion_contracts::orchestration::PatchValue;
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApprovalStatus, WorkflowBoardApprovePlanInput,
    WorkflowBoardApproveSpecInput, WorkflowBoardGetCardInput, WorkflowBoardPlanAlternative,
    WorkflowBoardPlanLens, WorkflowBoardPlanLensEntry, WorkflowBoardRequestErrorCode,
    WorkflowBoardReturnForRevisionInput, WorkflowBoardReturnTargetStage, WorkflowBoardStage,
    WorkflowBoardStageRunStage, WorkflowBoardStartStageInput, WorkflowBoardTaskStatus,
    WorkflowBoardUpdatePlanInput,
};
use support::{TestResult, replay_model, setup_m2_board};

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn copy_ready_cli_fixture(repo_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.legion-cli");
    let dst = repo_path.join(".legion-cli");
    copy_dir_recursive(&src, &dst)?;
    let tsk2_path = dst.join("tasks/TSK-0002.md");
    let tsk2_raw = fs::read_to_string(&tsk2_path)?;
    fs::write(
        tsk2_path,
        tsk2_raw.replacen("status: blocked", "status: todo", 1),
    )?;
    let exec_path = dst.join("workflow/execution.yaml");
    let exec_raw = fs::read_to_string(&exec_path)?;
    let exec_ready = exec_raw
        .lines()
        .filter(|l| !l.starts_with("blocker:"))
        .map(|l| {
            if l == "status: blocked" {
                "status: ready"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(exec_path, exec_ready)?;
    Ok(dst)
}

fn execute_input(
    card_id: &legion_contracts::ids::WorkflowCardId,
    expected_revision: u64,
    ack: Option<bool>,
) -> WorkflowBoardStartStageInput {
    WorkflowBoardStartStageInput {
        card_id: card_id.clone(),
        expected_revision,
        stage: WorkflowBoardStageRunStage::Execute,
        task_id: None,
        stable_launch_key: None,
        model_selection: Some(replay_model()),
        runtime_mode: Some(RuntimeMode::FullAccess),
        interaction_mode: None,
        workspace_strategy: None,
        acknowledge_unsupported_policies: ack,
        acknowledged_policies: None,
    }
}

#[tokio::test]
async fn start_stage_execute_claims_cli_todo_task_and_guards_stale_or_native() -> TestResult {
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let repo_path = Path::new(&repo_str);
    copy_ready_cli_fixture(repo_path)?;
    let project_id = ProjectId::new("proj-board-1")?;
    let applied = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id: project_id.clone(),
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: Some(RuntimeMode::FullAccess),
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    let card_id = applied.card_id.clone();

    let started = board
        .start_stage(&execute_input(&card_id, 1, Some(true)))
        .await?;
    assert_eq!(started.task_id.as_deref(), Some("TSK-0002"));
    assert!(started.run_id.is_some());
    assert!(!started.resumed);

    let detail = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card_id.clone(),
        project_id: None,
    })?;
    let tsk2 = detail
        .tasks
        .iter()
        .find(|t| t.task_id == "TSK-0002")
        .ok_or("missing TSK-0002")?;
    assert_eq!(tsk2.status, Some(WorkflowBoardTaskStatus::InProgress));
    let (active_sr, sr_status): (Option<String>, String) = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let active_sr: Option<String> = conn.query_row(
            "SELECT active_stage_run_id FROM workflow_board_tasks WHERE card_id = ?1 AND task_id = 'TSK-0002'",
            [card_id.as_str()],
            |r| r.get(0),
        )?;
        let sr_status: String = conn.query_row(
            "SELECT status FROM workflow_board_stage_runs WHERE id = ?1",
            [started.stage_run_id.as_str()],
            |r| r.get(0),
        )?;
        (active_sr, sr_status)
    };
    assert_eq!(active_sr.as_deref(), Some(started.stage_run_id.as_str()));
    assert_eq!(sr_status, "running");

    Box::pin(assert_stale_cli_plan_blocks_before_run(
        &orch, &board, repo_path,
    ))
    .await?;
    Box::pin(assert_native_card_blocks_before_run(
        &orch, &board, project_id,
    ))
    .await?;
    Ok(())
}

async fn assert_stale_cli_plan_blocks_before_run(
    orch: &std::sync::Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    repo_path: &Path,
) -> TestResult {
    let repo2 = repo_path.parent().expect("parent").join("repo-stale");
    fs::create_dir_all(&repo2)?;
    copy_ready_cli_fixture(&repo2)?;
    let p2 = ProjectId::new("proj-stale")?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        legion_persistence::project_store::create_project(
            &*conn,
            &legion_persistence::project_store::CreateProjectInput::new(
                repo2.to_string_lossy().as_ref(),
            )
            .with_project_id("proj-stale")
            .with_title("Stale CLI Project"),
        )?;
    }
    let imp2 = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id: p2,
        cli_dir: None,
        acknowledge_unsupported_policies: Some(true),
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "UPDATE workflow_board_approvals SET status = 'stale' WHERE card_id = ?1 AND gate = 'plan'",
            [imp2.card_id.as_str()],
        )?;
    }
    let stale_err = board
        .start_stage(&execute_input(&imp2.card_id, 1, Some(true)))
        .await
        .expect_err("stale CLI plan approval must fail");
    assert_eq!(
        stale_err.code,
        WorkflowBoardRequestErrorCode::PlanNotApproved
    );
    let stale_runs: i64 = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.query_row(
            "SELECT COUNT(*) FROM workflow_board_stage_runs WHERE card_id = ?1",
            [imp2.card_id.as_str()],
            |r| r.get(0),
        )?
    };
    assert_eq!(stale_runs, 0);
    Ok(())
}

async fn assert_native_card_blocks_before_run(
    orch: &std::sync::Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    project_id: ProjectId,
) -> TestResult {
    let native = board.create_card(
        &legion_contracts::workflow_board::WorkflowBoardCreateCardInput {
            project_id,
            title: "Native Unapproved".to_owned(),
            brief: Some("Brief".to_owned()),
            intake_goal: None,
            brief_path: None,
            input_paths: None,
            source_type: None,
            assistance_mode: None,
            wireframes_enabled: None,
            wireframes_index: legion_contracts::orchestration::PatchValue::Absent,
            workspace_strategy: None,
            model_selection: Some(replay_model()),
            runtime_mode: Some(RuntimeMode::FullAccess),
            interaction_mode: None,
        },
    )?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "UPDATE workflow_board_cards SET stage = 'execute' WHERE card_id = ?1",
            [native.summary.card_id.as_str()],
        )?;
    }
    let nat_err = board
        .start_stage(&execute_input(&native.summary.card_id, 1, None))
        .await
        .expect_err("native card without valid plan approval must fail");
    assert_eq!(nat_err.code, WorkflowBoardRequestErrorCode::PlanNotApproved);
    let nat_runs: i64 = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.query_row(
            "SELECT COUNT(*) FROM workflow_board_stage_runs WHERE card_id = ?1",
            [native.summary.card_id.as_str()],
            |r| r.get(0),
        )?
    };
    assert_eq!(nat_runs, 0);
    Ok(())
}

async fn assert_tampered_cli_deliverable_blocks_execute(
    orch: &std::sync::Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    base_parent: &Path,
    slug: &str,
    rel_file: &str,
    expect_spec_stale: bool,
) -> TestResult {
    let repo_dir = base_parent.join(format!("repo-tamper-{slug}"));
    fs::create_dir_all(&repo_dir)?;
    copy_ready_cli_fixture(&repo_dir)?;
    let pid_str = format!("proj-tamper-{slug}");
    let pid = ProjectId::new(&pid_str)?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        legion_persistence::project_store::create_project(
            &*conn,
            &legion_persistence::project_store::CreateProjectInput::new(
                repo_dir.to_string_lossy().as_ref(),
            )
            .with_project_id(&pid_str)
            .with_title("Tamper Project"),
        )?;
    }
    let imp = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id: pid,
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: Some(RuntimeMode::FullAccess),
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    let card_dir = repo_dir
        .join(".legion/workflows")
        .join(imp.card_id.as_str());
    let mut contents = fs::read_to_string(card_dir.join(rel_file))?;
    contents.push_str("\n<!-- external tampered deliverable -->\n");
    fs::write(card_dir.join(rel_file), contents)?;

    let (runs_before, threads_before): (i64, i64) = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let r = conn.query_row("SELECT COUNT(*) FROM workflow_runs", [], |r| r.get(0))?;
        let t = conn.query_row("SELECT COUNT(*) FROM projection_threads", [], |r| r.get(0))?;
        (r, t)
    };

    let err = board
        .start_stage(&execute_input(&imp.card_id, 1, Some(true)))
        .await
        .expect_err("tampered CLI deliverable must reject startStage(Execute)");
    assert_eq!(err.code, WorkflowBoardRequestErrorCode::PlanNotApproved);

    let (sr_count, runs_after, threads_after, ext_state, plan_st, spec_st): (
        i64,
        i64,
        i64,
        Option<String>,
        String,
        String,
    ) = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        let sr = conn.query_row(
            "SELECT COUNT(*) FROM workflow_board_stage_runs WHERE card_id = ?1",
            [imp.card_id.as_str()],
            |r| r.get(0),
        )?;
        let r = conn.query_row("SELECT COUNT(*) FROM workflow_runs", [], |r| r.get(0))?;
        let t = conn.query_row("SELECT COUNT(*) FROM projection_threads", [], |r| r.get(0))?;
        let ext = conn.query_row(
            "SELECT external_edit_state FROM workflow_board_cards WHERE card_id = ?1",
            [imp.card_id.as_str()],
            |r| r.get(0),
        )?;
        let ps = conn.query_row(
            "SELECT status FROM workflow_board_approvals WHERE card_id = ?1 AND gate = 'plan'",
            [imp.card_id.as_str()],
            |r| r.get(0),
        )?;
        let ss = conn.query_row(
            "SELECT status FROM workflow_board_approvals WHERE card_id = ?1 AND gate = 'spec'",
            [imp.card_id.as_str()],
            |r| r.get(0),
        )?;
        (sr, r, t, ext, ps, ss)
    };
    assert_eq!(sr_count, 0);
    assert_eq!(runs_after, runs_before);
    assert_eq!(threads_after, threads_before);
    assert_eq!(ext_state.as_deref(), Some("deliverable_stale"));
    assert_eq!(plan_st, "stale");
    assert_eq!(
        spec_st,
        if expect_spec_stale {
            "stale"
        } else {
            "historical"
        }
    );
    Ok(())
}

#[tokio::test]
async fn external_deliverable_tampering_blocks_cli_start_stage_execute_with_zero_runs() -> TestResult
{
    let (_tmp, orch, board, repo_str) = setup_m2_board()?;
    let parent = Path::new(&repo_str).parent().expect("parent");
    for (slug, rel, spec_stale) in [
        ("plan", "plan.md", false),
        ("task", "tasks/TSK-0002.md", false),
        ("design", "design.md", false),
        ("spec", "spec.md", true),
    ] {
        Box::pin(assert_tampered_cli_deliverable_blocks_execute(
            &orch, &board, parent, slug, rel, spec_stale,
        ))
        .await?;
    }
    Ok(())
}

fn six_completed_lenses() -> Vec<WorkflowBoardPlanLensEntry> {
    [
        WorkflowBoardPlanLens::Ux,
        WorkflowBoardPlanLens::Functional,
        WorkflowBoardPlanLens::Architecture,
        WorkflowBoardPlanLens::Quality,
        WorkflowBoardPlanLens::Infrastructure,
        WorkflowBoardPlanLens::Delivery,
    ]
    .into_iter()
    .map(|lens| WorkflowBoardPlanLensEntry {
        lens,
        applicable: Some(true),
        summary: Some("Verified lens".to_owned()),
        reason: None,
    })
    .collect()
}

async fn reapprove_spec_and_plan_after_return(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    cid: &legion_contracts::ids::WorkflowCardId,
    expected_revision: u64,
    expected_spec_digest: Option<String>,
) -> TestResult {
    let spec_app = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision,
                expected_digest: expected_spec_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(
        spec_app.spec_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );

    let plan_upd = board
        .update_plan(&WorkflowBoardUpdatePlanInput {
            card_id: cid.clone(),
            expected_revision: spec_app.revision,
            lenses: Some(six_completed_lenses()),
            alternatives: Some(vec![
                WorkflowBoardPlanAlternative {
                    id: "alt-a".to_owned(),
                    title: "Alt A".to_owned(),
                    summary: "Direct".to_owned(),
                    tradeoffs: "Fast".to_owned(),
                },
                WorkflowBoardPlanAlternative {
                    id: "alt-b".to_owned(),
                    title: "Alt B".to_owned(),
                    summary: "Phased".to_owned(),
                    tradeoffs: "Safe".to_owned(),
                },
            ]),
            selected_alternative_id: PatchValue::Set("alt-a".to_owned()),
            selection_rationale: Some("Selected for native re-approval".to_owned()),
            revise_alternatives_feedback: None,
            strategy: None,
            strategy_rationale: Some("Risk-first".to_owned()),
            strategy_metadata: None,
            granularity: None,
            tasks: None,
            checks: None,
            ac_mappings: None,
        })
        .await?;
    assert_eq!(
        plan_upd.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Valid)
    );

    let plan_app = board
        .approve_plan(
            &WorkflowBoardApprovePlanInput {
                card_id: cid.clone(),
                expected_revision: plan_upd.revision,
                expected_spec_digest: spec_app.card.spec.spec_digest,
                expected_plan_digest: plan_upd.plan_digest,
            },
            "client",
        )
        .await?;
    assert_eq!(
        plan_app.plan_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );
    assert_eq!(plan_app.card.summary.stage, WorkflowBoardStage::Execute);
    Ok(())
}

#[tokio::test]
async fn return_for_revision_on_cli_card_stales_historical_approvals_and_allows_reapproval()
-> TestResult {
    let (_tmp, _orch, board, repo_str) = setup_m2_board()?;
    copy_ready_cli_fixture(Path::new(&repo_str))?;
    let project_id = ProjectId::new("proj-board-1")?;
    let applied = board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id,
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
    })?;
    let cid = applied.card_id.clone();

    let ret_plan = board
        .return_for_revision(&WorkflowBoardReturnForRevisionInput {
            card_id: cid.clone(),
            expected_revision: 1,
            target_stage: WorkflowBoardReturnTargetStage::Plan,
            reason: "Revise CLI plan".to_owned(),
        })
        .await?;
    assert_eq!(ret_plan.stage, WorkflowBoardStage::Plan);
    assert_eq!(
        ret_plan.plan_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    assert_eq!(
        ret_plan.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Historical)
    );

    let ret_spec = board
        .return_for_revision(&WorkflowBoardReturnForRevisionInput {
            card_id: cid.clone(),
            expected_revision: ret_plan.revision,
            target_stage: WorkflowBoardReturnTargetStage::Spec,
            reason: "Revise CLI spec".to_owned(),
        })
        .await?;
    assert_eq!(ret_spec.stage, WorkflowBoardStage::Spec);
    assert_eq!(
        ret_spec.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    assert_eq!(
        ret_spec.plan_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );

    reapprove_spec_and_plan_after_return(
        &board,
        &cid,
        ret_spec.revision,
        ret_spec.card.spec.spec_digest.clone(),
    )
    .await
}
