#![forbid(unsafe_code)]
//! Integration tests for M1.1 R3 `ThreadLauncher` & `OrchestratorWorkflowAgentDriver`:
//! - Strict project lookup (no first-project fallback)
//! - Strict model & runtime mode validation (no `codex/gpt-5` or `FullAccess` fallback)
//! - `Worktree` binding & event-sourced `$.workflowRun` surviving `ThreadState` updates
//! - Stable launch identity reuse (`1` thread total) & turn-scoped `RunInterrupt`

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use legion_contracts::ids::{CommandId, EnvironmentId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, ModelSelection, OrchestrationActor, OrchestrationCommand, PatchValue,
    ProviderInteractionMode, ThreadMetadataUpdateCommand, WorkspaceStrategy,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::thread_launch_store::{get_thread_launch, mark_thread_launch_failed};
use legion_server_runtime::{
    LaunchThreadRequest, OrchestratorWorkflowAgentDriver, ThreadLaunchError, ThreadLauncher,
};
use legion_workflows::traits::{
    AgentInterruptOutcome, AgentInterruptTarget, AgentLaunch, AgentModel, AgentOrigin,
    AgentStartInput, WorkflowAgentDriver,
};
use serde_json::json;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct TestEnv {
    tmp: TempDir,
    paths: ServerDerivedPaths,
    orchestrator: Arc<Orchestrator>,
}

fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let status = StdCommand::new("git")
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .status()
            .expect("git exec");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init"]);
    run(&["config", "user.email", "test@legion.local"]);
    run(&["config", "user.name", "Legion Test"]);
    fs::write(dir.join("README.md"), "# Test Repo\n").expect("write readme");
    run(&["add", "."]);
    run(&["commit", "-m", "initial commit"]);
}

fn setup_env() -> Result<TestEnv, Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let paths = ServerDerivedPaths::derive(tmp.path(), false, true);
    ensure_server_directories(&paths)?;
    let store = SqliteStore::open(&paths.db_path)?;
    let orchestrator = Arc::new(Orchestrator::from_sqlite_store(store));

    let repo_dir = tmp.path().join("repo-alpha");
    fs::create_dir_all(&repo_dir)?;
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();
    {
        let conn = orchestrator.sink().db().lock().map_err(|_| "db lock")?;
        ensure_project_store_schema(&conn)?;
        create_project(
            &*conn,
            &CreateProjectInput::new(&repo_str)
                .with_project_id("proj-alpha")
                .with_title("Alpha"),
        )?;
    }
    Ok(TestEnv {
        tmp,
        paths,
        orchestrator,
    })
}

fn dev_replay_model() -> AgentModel {
    AgentModel {
        instance_id: "devReplay".to_owned(),
        model: "dev-replay".to_owned(),
        options: None,
    }
}

fn sample_start_input(
    project: Option<&str>,
    model: Option<AgentModel>,
    launch: Option<AgentLaunch>,
) -> AgentStartInput {
    AgentStartInput {
        key: "run-1/plan/0/start".to_owned(),
        origin: AgentOrigin {
            run_id: "run-1".to_owned(),
            workflow: "feature_flow".to_owned(),
            step: "plan".to_owned(),
        },
        title: "Plan Step".to_owned(),
        prompt: "Draft plan".to_owned(),
        output_schema: None,
        model,
        project: project.map(str::to_owned),
        environment: Some("env-local".to_owned()),
        thread: None,
        issue: None,
        launch,
        stable_launch_key: None,
    }
}

fn build_launch_req(
    cmd_id: &str,
    project_id: &str,
    stable_key: Option<&str>,
    workspace_strategy: WorkspaceStrategy,
) -> Result<LaunchThreadRequest, Box<dyn std::error::Error>> {
    Ok(LaunchThreadRequest {
        command_id: CommandId::new(cmd_id)?,
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        thread_id: None,
        stable_launch_key: stable_key.map(str::to_owned),
        reuse_existing_thread: false,
        project_id: project_id.to_owned(),
        title: "Launch Test".to_owned(),
        title_renamed: None,
        generate_title: None,
        model_selection: ModelSelection {
            instance_id: ProviderInstanceId::new("devReplay")?,
            model: "dev-replay".to_owned(),
            options: None,
        },
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        workspace_strategy,
        workflow_run: None,
        link_origin: None,
        delegated_from: None,
        initial_message: None,
    })
}

async fn assert_driver_validation_rejections(driver: &OrchestratorWorkflowAgentDriver) {
    let valid_launch = AgentLaunch {
        workspace: None,
        runtime_mode: Some("approval-required".to_owned()),
        interaction_mode: None,
        stable_launch_key: None,
    };
    let bad_mode = AgentLaunch {
        workspace: None,
        runtime_mode: Some("yolo-mode".to_owned()),
        interaction_mode: None,
        stable_launch_key: None,
    };
    let cases = [
        sample_start_input(None, Some(dev_replay_model()), Some(valid_launch.clone())),
        sample_start_input(
            Some("proj-unknown"),
            Some(dev_replay_model()),
            Some(valid_launch.clone()),
        ),
        sample_start_input(Some("proj-alpha"), None, Some(valid_launch)),
        sample_start_input(Some("proj-alpha"), Some(dev_replay_model()), None),
        sample_start_input(Some("proj-alpha"), Some(dev_replay_model()), Some(bad_mode)),
    ];
    for input in cases {
        assert!(driver.start(input).await.is_err());
    }
}

#[tokio::test]
async fn strict_validation_rejects_missing_project_model_and_runtime_mode() -> TestResult {
    let env = setup_env()?;
    let launcher = ThreadLauncher::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    );
    let driver = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);

    // (a) Unknown project fails even when proj-alpha is active (zero first-project fallback).
    let missing_proj = launcher
        .launch_thread(build_launch_req(
            "cmd-missing-proj",
            "proj-does-not-exist",
            None,
            WorkspaceStrategy::Root { branch: None },
        )?)
        .await;
    assert!(matches!(
        missing_proj,
        Err(ThreadLaunchError::ProjectNotFound)
    ));
    // (b) Missing project/model or missing/invalid runtimeMode fails explicitly on driver.
    assert_driver_validation_rejections(&driver).await;
    Ok(())
}

#[tokio::test]
async fn worktree_and_workflow_run_survives_metadata_updates() -> TestResult {
    let env = setup_env()?;
    let driver = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);

    let launch_opts = AgentLaunch {
        workspace: Some(json!({ "type": "worktree" })),
        runtime_mode: Some("approval-required".to_owned()),
        interaction_mode: Some("plan".to_owned()),
        stable_launch_key: Some("board:card-1:plan".to_owned()),
    };
    let turn1 = driver
        .start(sample_start_input(
            Some("proj-alpha"),
            Some(dev_replay_model()),
            Some(launch_opts),
        ))
        .await?;
    assert_ne!(turn1.thread_id, "");
    assert_ne!(turn1.turn_id, "");

    let tid = ThreadId::new(&turn1.thread_id)?;
    {
        let conn = env.orchestrator.sink().db().lock().map_err(|_| "db lock")?;
        let raw: String = conn.query_row(
            "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
            [tid.as_str()],
            |r| r.get(0),
        )?;
        let val: serde_json::Value = serde_json::from_str(&raw)?;
        assert_eq!(val["workflowRun"]["runId"], "run-1");
        assert_eq!(val["workflowRun"]["step"], "plan");
        assert_eq!(val["lineage"]["workflowStep"], true);
        assert_eq!(val["titleRenamed"], true);
        let wt = val["worktreePath"].as_str().ok_or("missing worktreePath")?;
        assert!(Path::new(wt).is_dir());
        assert!(
            val["branch"]
                .as_str()
                .is_some_and(|b| b.starts_with("legion/"))
        );
    }

    env.orchestrator
        .dispatch_now(&OrchestrationCommand::ThreadMetadataUpdate(
            ThreadMetadataUpdateCommand {
                command_id: CommandId::new("cmd-meta-1")?,
                thread_id: tid.clone(),
                title: Some("Plan Step Updated".to_owned()),
                regenerate_title: None,
                branch: PatchValue::default(),
                worktree_path: PatchValue::default(),
                expected_worktree_path: PatchValue::default(),
                expected_branch: PatchValue::default(),
                continuation: None,
                expected_empty: None,
                limit_recovery: PatchValue::default(),
                linked_pull_request: PatchValue::default(),
            },
        ))
        .await?;

    let conn = env.orchestrator.sink().db().lock().map_err(|_| "db lock")?;
    let raw: String = conn.query_row(
        "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id = ?1",
        [tid.as_str()],
        |r| r.get(0),
    )?;
    let val: serde_json::Value = serde_json::from_str(&raw)?;
    assert_eq!(val["title"], "Plan Step Updated");
    assert_eq!(val["workflowRun"]["runId"], "run-1");
    assert_eq!(val["lineage"]["workflowStep"], true);
    Ok(())
}

#[tokio::test]
async fn stable_launch_key_reuses_thread_and_interrupt_is_turn_scoped() -> TestResult {
    let env = setup_env()?;
    let driver = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);

    let launch_opts = AgentLaunch {
        workspace: Some(json!({ "type": "worktree" })),
        runtime_mode: Some("approval-required".to_owned()),
        interaction_mode: Some("plan".to_owned()),
        stable_launch_key: Some("board:card-2:plan".to_owned()),
    };
    let turn1 = driver
        .start(sample_start_input(
            Some("proj-alpha"),
            Some(dev_replay_model()),
            Some(launch_opts.clone()),
        ))
        .await?;

    let driver2 = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);
    let mut second_input = sample_start_input(
        Some("proj-alpha"),
        Some(dev_replay_model()),
        Some(launch_opts),
    );
    second_input.key = "run-2/plan/1/start".to_owned();
    let turn2 = driver2.start(second_input).await?;
    assert_eq!(turn2.thread_id, turn1.thread_id);
    {
        let conn = env.orchestrator.sink().db().lock().map_err(|_| "db lock")?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 1);
    }

    let outcome1 = driver
        .interrupt(AgentInterruptTarget {
            thread_id: turn1.thread_id.clone(),
            environment: None,
            turn_id: Some(turn1.turn_id.clone()),
        })
        .await?;
    assert_eq!(outcome1, AgentInterruptOutcome::Stopped);
    let outcome2 = driver
        .interrupt(AgentInterruptTarget {
            thread_id: turn1.thread_id.clone(),
            environment: None,
            turn_id: Some(turn1.turn_id.clone()),
        })
        .await?;
    assert_eq!(outcome2, AgentInterruptOutcome::Ended);

    let mismatched = driver
        .start(sample_start_input(
            Some("proj-alpha"),
            Some(dev_replay_model()),
            Some(AgentLaunch {
                workspace: Some(json!({ "type": "root" })),
                runtime_mode: Some("approval-required".to_owned()),
                interaction_mode: None,
                stable_launch_key: Some("board:card-2:plan".to_owned()),
            }),
        ))
        .await;
    assert!(mismatched.is_err());
    Ok(())
}

fn count_git_worktrees(repo_dir: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let out = StdCommand::new("git")
        .current_dir(repo_dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout)?;
    Ok(text.lines().filter(|l| l.starts_with("worktree ")).count())
}

fn build_concurrent_req(
    cmd_id: &str,
    stable_key: &str,
    branch: Option<&str>,
) -> Result<LaunchThreadRequest, Box<dyn std::error::Error>> {
    build_launch_req(
        cmd_id,
        "proj-alpha",
        Some(stable_key),
        WorkspaceStrategy::Worktree {
            base_ref: "HEAD".to_owned(),
            branch: branch.map(str::to_owned),
            start_from_origin: None,
            repositories: None,
        },
    )
}

async fn assert_concurrent_driver_launch_coalesces(env: &TestEnv, repo_dir: &Path) -> TestResult {
    let driver1 = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);
    let driver2 = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);
    let mut in1 = sample_start_input(
        Some("proj-alpha"),
        Some(dev_replay_model()),
        Some(AgentLaunch {
            workspace: Some(json!({ "type": "worktree" })),
            runtime_mode: Some("approval-required".to_owned()),
            interaction_mode: Some("plan".to_owned()),
            stable_launch_key: Some("race:driver:plan".to_owned()),
        }),
    );
    "run-d1/plan/0/start".clone_into(&mut in1.key);
    let mut in2 = in1.clone();
    "run-d2/plan/0/start".clone_into(&mut in2.key);
    let (d1, d2) = tokio::join!(driver1.start(in1), driver2.start(in2));
    let (d1, d2) = (d1?, d2?);
    assert_eq!(d1.thread_id, d2.thread_id);
    assert_eq!(count_git_worktrees(repo_dir)?, 4);
    Ok(())
}

fn assert_committed_launch_records(env: &TestEnv, keys: &[&str]) -> TestResult {
    let conn = env.orchestrator.sink().db().lock().map_err(|_| "db lock")?;
    let total_threads: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(total_threads, 3);
    for key in keys {
        let rec = get_thread_launch(&conn, key)?.ok_or("missing launch rec")?;
        assert_eq!(rec.status, "committed");
        assert!(rec.thread_committed);
        assert_eq!(rec.last_error, None);
    }
    // Late mark_thread_launch_failed must never overwrite a committed launch record.
    mark_thread_launch_failed(
        &conn,
        "race:card-1:plan",
        "late failure",
        "2026-11-24T02:00:00Z",
    )?;
    let after = get_thread_launch(&conn, "race:card-1:plan")?.ok_or("missing launch rec")?;
    assert_eq!(after.status, "committed");
    assert_eq!(after.last_error, None);
    Ok(())
}

#[tokio::test]
async fn concurrent_double_launch_on_same_stable_key_coalesces_cleanly() -> TestResult {
    let env = setup_env()?;
    let repo_dir = env.tmp.path().join("repo-alpha");
    let launcher_a = ThreadLauncher::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    );
    let launcher_b = ThreadLauncher::new(
        Arc::clone(&env.orchestrator),
        env.paths.worktrees_dir.clone(),
    );

    // 1. Concurrent Worktree launch with branch=None across separate ThreadLauncher instances.
    let (r1, r2) = tokio::join!(
        launcher_a.launch_thread(build_concurrent_req(
            "cmd-race-1a",
            "race:card-1:plan",
            None
        )?),
        launcher_b.launch_thread(build_concurrent_req(
            "cmd-race-1b",
            "race:card-1:plan",
            None
        )?),
    );
    let (r1, r2) = (r1?, r2?);
    assert_eq!(r1.thread_id, r2.thread_id);
    assert_eq!(r1.worktree_path, r2.worktree_path);
    assert!(r1.resumed ^ r2.resumed);
    assert_eq!(count_git_worktrees(&repo_dir)?, 2);

    let retry = launcher_a
        .launch_thread(build_concurrent_req(
            "cmd-race-1c",
            "race:card-1:plan",
            None,
        )?)
        .await?;
    assert!(retry.resumed);
    assert_eq!(retry.thread_id, r1.thread_id);

    // 2. Concurrent Worktree launch with explicit branch=Some("feat/race").
    let (b1, b2) = tokio::join!(
        launcher_a.launch_thread(build_concurrent_req(
            "cmd-race-2a",
            "race:card-2:plan",
            Some("feat/race"),
        )?),
        launcher_b.launch_thread(build_concurrent_req(
            "cmd-race-2b",
            "race:card-2:plan",
            Some("feat/race"),
        )?),
    );
    let (b1, b2) = (b1?, b2?);
    assert_eq!(b1.thread_id, b2.thread_id);
    assert_eq!(b1.worktree_path, b2.worktree_path);
    assert_eq!(b1.branch.as_deref(), Some("feat/race"));
    assert_eq!(count_git_worktrees(&repo_dir)?, 3);

    // 3. Concurrent OrchestratorWorkflowAgentDriver::start calls & committed record integrity.
    assert_concurrent_driver_launch_coalesces(&env, &repo_dir).await?;
    assert_committed_launch_records(
        &env,
        &["race:card-1:plan", "race:card-2:plan", "race:driver:plan"],
    )
}
