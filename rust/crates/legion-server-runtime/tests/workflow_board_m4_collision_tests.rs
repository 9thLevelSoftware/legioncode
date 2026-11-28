#![forbid(unsafe_code)]
//! Regression tests for multi-project `.legion-cli` `specId` collision handling.

#[path = "workflow_board_m2_support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use legion_contracts::ids::ProjectId;
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApprovalStatus, WorkflowBoardGetCardInput,
    WorkflowBoardStageRunStage, WorkflowBoardStartStageInput, WorkflowBoardTaskStatus,
};
use legion_persistence::project_store::{CreateProjectInput, create_project};
use serde_json::Value;
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

fn init_ready_project_repo(root: &Path, name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let repo_dir = root.join(name);
    fs::create_dir_all(&repo_dir)?;
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@legion.local"],
        vec!["config", "user.name", "Legion Test"],
    ] {
        StdCommand::new("git")
            .current_dir(&repo_dir)
            .args(args)
            .output()?;
    }
    fs::write(repo_dir.join("README.md"), format!("# {name}\n"))?;
    StdCommand::new("git")
        .current_dir(&repo_dir)
        .args(["add", "."])
        .output()?;
    StdCommand::new("git")
        .current_dir(&repo_dir)
        .args(["commit", "-m", "init"])
        .output()?;

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.legion-cli");
    let dst = repo_dir.join(".legion-cli");
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
    Ok(repo_dir)
}

fn apply_ready_import(
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    project_id: ProjectId,
) -> Result<
    legion_contracts::workflow_board::WorkflowBoardApplyCliImportResult,
    Box<dyn std::error::Error>,
> {
    Ok(board.apply_cli_import(&WorkflowBoardApplyCliImportInput {
        project_id,
        cli_dir: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
        model_selection: Some(replay_model()),
        runtime_mode: Some(RuntimeMode::FullAccess),
        interaction_mode: None,
        workspace_strategy: None,
    })?)
}

async fn assert_clean_colliding_card_and_execute(
    orch: &std::sync::Arc<legion_orchestrator::Orchestrator>,
    board: &legion_server_runtime::ProjectWorkflowBoardService,
    repo2: &Path,
    proj2: ProjectId,
    card2_id: &legion_contracts::ids::WorkflowCardId,
) -> TestResult {
    let card2_dir = repo2.join(".legion/workflows/cli-spec-t3-code-1");
    let expected_line = "- Card ID: `cli-spec-t3-code-1`";
    for rel in ["spec.md", "design.md", "plan.md", "tasks/TSK-0002.md"] {
        let content = fs::read_to_string(card2_dir.join(rel))?;
        assert!(
            content.contains(expected_line),
            "{rel} should embed `{expected_line}`, got:\n{content}"
        );
    }
    let card_json: Value = serde_json::from_str(&fs::read_to_string(card2_dir.join("card.json"))?)?;
    assert_eq!(card_json["cardId"], "cli-spec-t3-code-1");

    let detail2 = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card2_id.clone(),
        project_id: Some(proj2.clone()),
    })?;
    assert_eq!(
        detail2.plan_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Historical)
    );
    assert_eq!(
        detail2
            .recovery_state
            .as_ref()
            .map(|r| r.requires_reconciliation),
        Some(false)
    );
    assert!(
        detail2
            .recovery_state
            .as_ref()
            .is_some_and(|r| r.external_edits.is_empty())
    );
    let ext_state_before: Option<String> = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.query_row(
            "SELECT external_edit_state FROM workflow_board_cards WHERE card_id = ?1",
            [card2_id.as_str()],
            |r| r.get(0),
        )?
    };
    assert_eq!(ext_state_before, None);

    let re_import2 = apply_ready_import(board, proj2)?;
    assert!(!re_import2.created);
    assert_eq!(re_import2.card_id.as_str(), "cli-spec-t3-code-1");
    assert_eq!(re_import2.revision, 1);

    let started = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: card2_id.clone(),
            expected_revision: 1,
            stage: WorkflowBoardStageRunStage::Execute,
            task_id: None,
            stable_launch_key: None,
            model_selection: Some(replay_model()),
            runtime_mode: Some(RuntimeMode::FullAccess),
            interaction_mode: None,
            workspace_strategy: None,
            acknowledge_unsupported_policies: Some(true),
            acknowledged_policies: None,
        })
        .await?;
    assert_eq!(started.task_id.as_deref(), Some("TSK-0002"));
    assert!(started.run_id.is_some());
    assert!(!started.resumed);

    let after_start = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card2_id.clone(),
        project_id: None,
    })?;
    let tsk2 = after_start
        .tasks
        .iter()
        .find(|t| t.task_id == "TSK-0002")
        .ok_or("missing TSK-0002")?;
    assert_eq!(tsk2.status, Some(WorkflowBoardTaskStatus::InProgress));
    let ext_state_after: Option<String> = {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.query_row(
            "SELECT external_edit_state FROM workflow_board_cards WHERE card_id = ?1",
            [card2_id.as_str()],
            |r| r.get(0),
        )?
    };
    assert_ne!(ext_state_after.as_deref(), Some("deliverable_stale"));
    Ok(())
}

#[tokio::test]
async fn multi_project_spec_id_collision_renders_authoritative_card_id_and_starts_execute()
-> TestResult {
    let (tmp, orch, board, _repo_str) = setup_m2_board()?;
    let repo1 = init_ready_project_repo(tmp.path(), "repo-proj-1")?;
    let repo2 = init_ready_project_repo(tmp.path(), "repo-proj-2")?;

    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        for (pid, repo, title) in [
            ("proj-1", &repo1, "Project 1"),
            ("proj-2", &repo2, "Project 2"),
        ] {
            create_project(
                &*conn,
                &CreateProjectInput::new(repo.to_string_lossy().as_ref())
                    .with_project_id(pid)
                    .with_title(title),
            )?;
        }
    }

    let proj1 = ProjectId::new("proj-1")?;
    let proj2 = ProjectId::new("proj-2")?;
    let imp1 = apply_ready_import(&board, proj1)?;
    assert_eq!(imp1.card_id.as_str(), "cli-spec-t3-code");

    let imp2 = apply_ready_import(&board, proj2.clone())?;
    assert_eq!(imp2.card_id.as_str(), "cli-spec-t3-code-1");
    assert_eq!(imp2.source_spec_id, "spec-t3-code");
    assert!(imp2.created);

    Box::pin(assert_clean_colliding_card_and_execute(
        &orch,
        &board,
        &repo2,
        proj2,
        &imp2.card_id,
    ))
    .await
}
