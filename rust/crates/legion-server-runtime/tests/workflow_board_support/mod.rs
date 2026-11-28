#![forbid(unsafe_code)]
//! Shared test fixtures and verification helpers for `workflow_board_service` integration tests.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use legion_contracts::ids::{EnvironmentId, ProjectId, ProviderInstanceId, WorkflowCardId};
use legion_contracts::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardApprovalStatus, WorkflowBoardApproveSpecInput,
    WorkflowBoardArtifactKind, WorkflowBoardCreateCardInput, WorkflowBoardGetArtifactInput,
    WorkflowBoardGetCardInput, WorkflowBoardRequestErrorCode, WorkflowBoardStageRunStage,
    WorkflowBoardStartStageInput, WorkflowBoardUpdateSpecInput,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::stage_write;
use legion_persistence::workflow_board_repo::types::StageWriteInput;
use legion_server_runtime::{
    OrchestratorWorkflowAgentDriver, ProjectWorkflowBoardService, ScmWorkflowForgeDriver,
    reconcile_incomplete_write_journal,
};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::WorkflowsLocation;
use legion_workflows::{WorkflowStore, builtin_stage_definitions, sha256_prefixed};
use rusqlite::params;
use tempfile::TempDir;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type BoardStack = (
    Arc<Orchestrator>,
    Arc<WorkflowDefinitionsManager>,
    ProjectWorkflowBoardService,
);

pub fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .expect("git exec");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

pub fn init_git_repo(dir: &Path) {
    run_git(dir, &["init"]);
    run_git(dir, &["config", "user.email", "test@legion.local"]);
    run_git(dir, &["config", "user.name", "Legion Test"]);
    fs::write(dir.join("README.md"), "# Board Repo\n").expect("write readme");
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "-m", "initial commit"]);
}

pub fn open_board_stack(
    paths: &ServerDerivedPaths,
    workflows_enabled: bool,
) -> Result<BoardStack, Box<dyn std::error::Error>> {
    let store = SqliteStore::open(&paths.db_path)?;
    let orchestrator = Arc::new(Orchestrator::from_sqlite_store(store));
    let env_id = EnvironmentId::new("env-board-test")?;
    let wf_store = WorkflowStore::new(Arc::clone(orchestrator.sink().db()))?;
    let wf_mgr = WorkflowDefinitionsManager::open_with_builtins(
        wf_store,
        Arc::new(
            OrchestratorWorkflowAgentDriver::new(
                Arc::clone(&orchestrator),
                paths.worktrees_dir.clone(),
            )
            .with_settings_path(Some(paths.settings_path.clone()))
            .with_environment_id(env_id.clone()),
        ),
        Arc::new(ScmWorkflowForgeDriver::new(Arc::clone(
            orchestrator.sink().db(),
        ))),
        WorkflowsLocation {
            directory: paths.state_dir.join("workflows"),
            single: None,
        },
        builtin_stage_definitions(),
        workflows_enabled,
    )?;
    let board = ProjectWorkflowBoardService::new(
        Arc::clone(&orchestrator),
        paths.worktrees_dir.clone(),
        Some(paths.settings_path.clone()),
        env_id,
        Some(Arc::clone(&wf_mgr)),
    );
    Ok((orchestrator, wf_mgr, board))
}

pub fn setup_workspace() -> Result<(TempDir, ServerDerivedPaths, String), Box<dyn std::error::Error>>
{
    let tmp = TempDir::new()?;
    let paths = ServerDerivedPaths::derive(tmp.path(), false, true);
    ensure_server_directories(&paths)?;
    let repo_dir = tmp.path().join("repo-main");
    fs::create_dir_all(&repo_dir)?;
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();
    let store = SqliteStore::open(&paths.db_path)?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(&repo_str)
            .with_project_id("proj-board-1")
            .with_title("Board Project"),
    )?;
    Ok((tmp, paths, repo_str))
}

pub fn dev_replay_selection() -> ModelSelection {
    ModelSelection {
        instance_id: ProviderInstanceId::new("devReplay").expect("valid provider"),
        model: "dev-replay".to_owned(),
        options: None,
    }
}

pub async fn create_and_approve_card(
    paths: &ServerDerivedPaths,
    repo_path: &Path,
    head_before: &str,
) -> Result<(WorkflowCardId, u64, Option<String>), Box<dyn std::error::Error>> {
    let (_orch, _wf_mgr, board) = open_board_stack(paths, true)?;
    let created = board.create_card(&WorkflowBoardCreateCardInput {
        project_id: ProjectId::new("proj-board-1")?,
        title: "Workflow Board Feature".to_owned(),
        brief: Some("Build the 4-stage board".to_owned()),
        brief_path: None,
        input_paths: None,
        intake_goal: None,
        wireframes_enabled: None,
        wireframes_index: legion_contracts::orchestration::PatchValue::Absent,
        source_type: None,
        assistance_mode: None,
        workspace_strategy: Some(WorkspaceStrategy::Worktree {
            base_ref: "HEAD".to_owned(),
            branch: None,
            start_from_origin: None,
            repositories: None,
        }),
        model_selection: Some(dev_replay_selection()),
        runtime_mode: Some(RuntimeMode::FullAccess),
        interaction_mode: Some(ProviderInteractionMode::Default),
    })?;
    let cid = created.summary.card_id.clone();

    let updated = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: created.summary.revision,
            title: None,
            brief: Some("Updated brief with AC-01".to_owned()),
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: Some(vec![WorkflowBoardAcceptanceCriterion {
                id: "AC-01".to_owned(),
                text: "Persists across restart".to_owned(),
            }]),
            constraints: Some(vec!["Zero auto-commits".to_owned()]),
            source_refs: Some(vec!["README.md".to_owned()]),
            wireframes_index: legion_contracts::orchestration::PatchValue::Null,
        })
        .await?;

    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: updated.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        approved.spec_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );

    let spec_disk = fs::read(
        repo_path
            .join(".legion/workflows")
            .join(cid.as_str())
            .join("spec.md"),
    )?;
    assert_eq!(
        approved.spec_approval.spec_digest.as_deref(),
        Some(sha256_prefixed(&spec_disk).as_str())
    );
    assert_eq!(run_git(repo_path, &["rev-parse", "HEAD"]), head_before);
    Ok((cid, approved.revision, approved.spec_approval.spec_digest))
}

pub async fn verify_start_stage_idempotency(
    orchestrator: &Orchestrator,
    board2: &ProjectWorkflowBoardService,
    wf_mgr2: &Arc<WorkflowDefinitionsManager>,
    card_id: &WorkflowCardId,
    approved_rev: u64,
    repo_path: &Path,
    head_before: &str,
) -> TestResult {
    let start_req = WorkflowBoardStartStageInput {
        card_id: card_id.clone(),
        expected_revision: approved_rev,
        stage: WorkflowBoardStageRunStage::Plan,
        task_id: None,
        stable_launch_key: None,
        model_selection: None,
        runtime_mode: None,
        interaction_mode: None,
        workspace_strategy: None,
        acknowledge_unsupported_policies: None,
        acknowledged_policies: None,
    };
    let (r1, r2) = tokio::join!(
        board2.start_stage(&start_req),
        board2.start_stage(&start_req)
    );
    let (first, conflict) = match (r1, r2) {
        (Ok(ok), Err(err)) | (Err(err), Ok(ok)) => (ok, err),
        other => panic!("expected 1 success + 1 RevisionConflict, got {other:?}"),
    };
    assert_eq!(
        conflict.code,
        WorkflowBoardRequestErrorCode::RevisionConflict
    );
    assert!(!first.resumed);

    let second = board2
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: card_id.clone(),
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
    assert!(second.resumed);
    assert_eq!(second.thread_id, first.thread_id);
    assert_eq!(second.run_id, first.run_id);

    let art = board2.get_artifact(&WorkflowBoardGetArtifactInput {
        card_id: card_id.clone(),
        key: None,
        kind: Some(WorkflowBoardArtifactKind::Plan),
    })?;
    assert_eq!(art.key, "plan-brief.md");
    assert!(art.content.contains("AC-01"));
    assert_eq!(run_git(repo_path, &["rev-parse", "HEAD"]), head_before);

    let detail_after = board2.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: card_id.clone(),
        project_id: None,
    })?;
    assert_eq!(detail_after.stage_runs.len(), 1);
    let wt = detail_after.stage_runs[0]
        .worktree_path
        .as_deref()
        .ok_or("expected worktree_path on stage run")?;
    assert!(Path::new(wt).is_dir());
    let branch = detail_after.stage_runs[0]
        .branch
        .as_deref()
        .ok_or("expected branch")?;
    verify_plan_brief_workspace(
        orchestrator,
        &art.content,
        &first.thread_id,
        wt,
        branch,
        repo_path,
    )?;

    let wf_run_id = first.run_id.as_ref().ok_or("missing runId")?.as_str();
    wf_mgr2
        .engine()
        .cancel(wf_run_id, Some("retry test".to_owned()))
        .await?;
    let retried = board2
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: card_id.clone(),
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
    assert!(!retried.resumed);
    assert_ne!(retried.thread_id, first.thread_id);
    Ok(())
}

pub async fn verify_journal_recovery_and_external_edit(
    orch: &Arc<Orchestrator>,
    board: &ProjectWorkflowBoardService,
    cid: &WorkflowCardId,
    card_dir: &Path,
    spec_path: &Path,
    orig_spec: &str,
    created_rev: u64,
) -> TestResult {
    let torn_tmp = card_dir.join("spec.md.wj-torn.tmp");
    fs::write(&torn_tmp, "torn bytes")?;
    let orphan_tmp = card_dir.join("spec.md.tmp");
    fs::write(&orphan_tmp, "orphan bytes")?;
    {
        let conn = orch.sink().db().lock().map_err(|_| "db lock")?;
        conn.execute(
            "INSERT INTO workflow_board_write_journal (id, card_id, target_path, status, created_at)
             VALUES ('wj-raw-pending', ?1, ?2, 'pending', '2026-11-24T00:00:00Z')",
            params![cid.as_str(), spec_path.to_string_lossy().as_ref()],
        )?;
        let _ = stage_write(
            &conn,
            &StageWriteInput {
                id: "wj-uncommitted-staged",
                card_id: cid.as_str(),
                target_path: &spec_path.to_string_lossy(),
                staging_path: Some(&torn_tmp.to_string_lossy()),
                operation: "write",
                expected_digest: Some("sha256:0000"),
                previous_digest: None,
                revision: Some(99),
            },
            "2026-11-24T00:00:01Z",
        )?;
        let report = reconcile_incomplete_write_journal(&conn, "2026-11-24T00:00:02Z")?;
        assert_eq!(report.rolled_back, 2);
        assert!(report.temp_files_cleaned >= 1);
    }
    assert!(!torn_tmp.exists());
    assert!(!orphan_tmp.exists());
    assert_eq!(fs::read_to_string(spec_path)?, orig_spec);

    let updated = board
        .update_spec(&WorkflowBoardUpdateSpecInput {
            card_id: cid.clone(),
            expected_revision: created_rev,
            title: None,
            brief: None,
            intake_goal: None,
            input_paths: None,
            acceptance_criteria: Some(vec![WorkflowBoardAcceptanceCriterion {
                id: "AC-01".to_owned(),
                text: "Journal rollback preserves the spec".to_owned(),
            }]),
            constraints: None,
            source_refs: None,
            wireframes_index: legion_contracts::orchestration::PatchValue::Absent,
        })
        .await?;

    let approved = board
        .approve_spec(
            &WorkflowBoardApproveSpecInput {
                card_id: cid.clone(),
                expected_revision: updated.revision,
                expected_digest: None,
            },
            "client",
        )
        .await?;
    assert_eq!(
        approved.spec_approval.status,
        WorkflowBoardApprovalStatus::Valid
    );

    fs::write(spec_path, "# Tampered externally\n")?;
    let after_edit = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    assert_eq!(
        after_edit.spec_approval.as_ref().map(|a| a.status),
        Some(WorkflowBoardApprovalStatus::Stale)
    );
    let start_err = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: approved.revision,
            stage: WorkflowBoardStageRunStage::Plan,
            task_id: None,
            stable_launch_key: None,
            model_selection: Some(dev_replay_selection()),
            runtime_mode: Some(RuntimeMode::FullAccess),
            interaction_mode: Some(ProviderInteractionMode::Default),
            workspace_strategy: None,
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await
        .expect_err("must reject stale spec approval");
    assert_eq!(
        start_err.code,
        WorkflowBoardRequestErrorCode::SpecNotApproved
    );
    Ok(())
}

pub async fn verify_plan_workspace_binding(
    board: &ProjectWorkflowBoardService,
    cid: &WorkflowCardId,
    revision: u64,
    repo_str: &str,
    worktree_path: &Path,
) -> TestResult {
    let artifact = board.get_artifact(&WorkflowBoardGetArtifactInput {
        card_id: cid.clone(),
        key: Some("plan-brief.md".to_owned()),
        kind: None,
    })?;
    assert!(
        artifact
            .content
            .contains(&format!("- Worktree Path: `{}`", worktree_path.display()))
    );
    assert!(artifact.content.contains("- Branch: `existing-plan`"));
    let next = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: revision,
            stage: WorkflowBoardStageRunStage::Plan,
            task_id: None,
            stable_launch_key: Some("fresh-bound-plan".to_owned()),
            model_selection: Some(dev_replay_selection()),
            runtime_mode: Some(RuntimeMode::FullAccess),
            interaction_mode: None,
            workspace_strategy: Some(WorkspaceStrategy::Root { branch: None }),
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await?;
    assert!(!next.resumed);
    let next_detail = board.get_card_detail(&WorkflowBoardGetCardInput {
        card_id: cid.clone(),
        project_id: None,
    })?;
    let next_run = next_detail
        .stage_runs
        .iter()
        .find(|run| run.thread_id.as_ref() == Some(&next.thread_id))
        .ok_or("fresh Plan stage run missing")?;
    assert_eq!(
        next_run.worktree_path.as_deref(),
        Some(worktree_path.to_string_lossy().as_ref())
    );
    let next_artifact = board.get_artifact(&WorkflowBoardGetArtifactInput {
        card_id: cid.clone(),
        key: Some("plan-brief.md".to_owned()),
        kind: None,
    })?;
    assert!(
        next_artifact
            .content
            .contains(&format!("- Worktree Path: `{}`", worktree_path.display()))
    );
    assert!(next_artifact.content.contains("- Branch: `existing-plan`"));
    let err = board
        .start_stage(&WorkflowBoardStartStageInput {
            card_id: cid.clone(),
            expected_revision: next.revision,
            stage: WorkflowBoardStageRunStage::Plan,
            task_id: None,
            stable_launch_key: Some(format!("incompatible-plan-{}", uuid::Uuid::new_v4())),
            model_selection: Some(dev_replay_selection()),
            runtime_mode: Some(RuntimeMode::FullAccess),
            interaction_mode: None,
            workspace_strategy: Some(WorkspaceStrategy::ExistingWorktree {
                worktree_path: repo_str.to_owned(),
                branch: None,
            }),
            acknowledge_unsupported_policies: None,
            acknowledged_policies: None,
        })
        .await
        .expect_err("bound Plan must reject a different workspace");
    assert_eq!(err.code, WorkflowBoardRequestErrorCode::ContractConflict);
    let unchanged = board.get_artifact(&WorkflowBoardGetArtifactInput {
        card_id: cid.clone(),
        key: Some("plan-brief.md".to_owned()),
        kind: None,
    })?;
    assert_eq!(unchanged.content, next_artifact.content);

    Ok(())
}

fn verify_plan_brief_workspace(
    orchestrator: &Orchestrator,
    brief: &str,
    thread_id: &legion_contracts::ids::ThreadId,
    wt: &str,
    branch: &str,
    repo_path: &Path,
) -> TestResult {
    assert!(brief.contains(&format!("- Worktree Path: `{wt}`")));
    assert!(brief.contains(&format!("- Branch: `{branch}`")));
    let initial_text = {
        let conn = orchestrator.sink().db().lock().map_err(|_| "db lock")?;
        let thread =
            legion_orchestrator::projections::query_wire_thread_detail(&conn, thread_id.as_str())?;
        thread
            .projection
            .messages
            .iter()
            .filter_map(|message| message.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(initial_text.contains(brief));
    assert!(initial_text.contains(&format!("- Active worktree: `{wt}`")));
    assert!(initial_text.contains(&format!(
        "- Project workspace root: `{}`",
        repo_path.display()
    )));

    Ok(())
}
