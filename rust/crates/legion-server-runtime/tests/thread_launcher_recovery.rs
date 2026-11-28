#![forbid(unsafe_code)]
//! Crash recovery, `ExistingWorktree`, and deleted-thread reuse tests for `ThreadLauncher`.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use legion_contracts::ids::{CommandId, EnvironmentId, ProviderInstanceId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, ModelSelection, OrchestrationActor, OrchestrationCommand,
    ProviderInteractionMode, WorkspaceStrategy,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::Orchestrator;
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_persistence::thread_launch_store::{
    ClaimThreadLaunchInput, claim_thread_launch, get_thread_launch,
    mark_thread_launch_setup_committed,
};
use legion_server_runtime::{
    LaunchThreadRequest, OrchestratorWorkflowAgentDriver, ThreadLaunchError, ThreadLauncher,
};
use legion_workflows::traits::{
    AgentLaunch, AgentModel, AgentOrigin, AgentStartInput, WorkflowAgentDriver,
};
use serde_json::json;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct CrashFixture {
    tmp: TempDir,
    paths: ServerDerivedPaths,
    orchestrator: Arc<Orchestrator>,
    repo_str: String,
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

fn setup_crash_fixture() -> Result<CrashFixture, Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let paths = ServerDerivedPaths::derive(tmp.path(), false, true);
    ensure_server_directories(&paths)?;
    let store = SqliteStore::open(&paths.db_path)?;
    let orchestrator = Arc::new(Orchestrator::from_sqlite_store(store));

    let repo_dir = tmp.path().join("repo-crash");
    fs::create_dir_all(&repo_dir)?;
    init_git_repo(&repo_dir);
    let repo_str = repo_dir.to_string_lossy().into_owned();
    {
        let conn = orchestrator.sink().db().lock().map_err(|_| "db lock")?;
        ensure_project_store_schema(&conn)?;
        create_project(
            &*conn,
            &CreateProjectInput::new(&repo_str)
                .with_project_id("proj-crash")
                .with_title("CrashProj"),
        )?;
    }
    Ok(CrashFixture {
        tmp,
        paths,
        orchestrator,
        repo_str,
    })
}

fn make_launch_req(
    cmd_id: &str,
    stable_launch_key: Option<&str>,
    reuse_existing_thread: bool,
    workspace_strategy: WorkspaceStrategy,
) -> Result<LaunchThreadRequest, Box<dyn std::error::Error>> {
    Ok(LaunchThreadRequest {
        command_id: CommandId::new(cmd_id)?,
        created_by: OrchestrationActor::User,
        creation_source: CreationSource::Web,
        thread_id: None,
        stable_launch_key: stable_launch_key.map(str::to_owned),
        reuse_existing_thread,
        project_id: "proj-crash".to_owned(),
        title: "Crash Test".to_owned(),
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

#[tokio::test]
async fn crash_recovery_resumes_committed_worktree() -> TestResult {
    let fx = setup_crash_fixture()?;
    let driver = OrchestratorWorkflowAgentDriver::new(
        Arc::clone(&fx.orchestrator),
        fx.paths.worktrees_dir.clone(),
    )
    .with_environment_id(EnvironmentId::new("env-local")?);

    let crashed_wt = fx.tmp.path().join("recovered-wt");
    let status = StdCommand::new("git")
        .current_dir(&fx.repo_str)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "worktree",
            "add",
            "-b",
            "legion/recovered",
            crashed_wt.to_str().ok_or("path")?,
            "HEAD",
        ])
        .status()?;
    assert!(status.success());
    {
        let conn = fx.orchestrator.sink().db().lock().map_err(|_| "db lock")?;
        claim_thread_launch(
            &conn,
            &ClaimThreadLaunchInput {
                stable_launch_key: "board:card-crash:execute",
                thread_id: "thr-recovered-1",
                project_id: "proj-crash",
                title: "Recovered",
                now_iso: "2026-11-24T00:00:00Z",
            },
        )?;
        mark_thread_launch_setup_committed(
            &conn,
            "board:card-crash:execute",
            Some(crashed_wt.to_str().ok_or("path")?),
            Some("legion/recovered"),
            "2026-11-24T00:00:01Z",
        )?;
    }

    let recovered_turn = driver
        .start(AgentStartInput {
            key: "run-c/exec/0/start".to_owned(),
            origin: AgentOrigin {
                run_id: "run-c".to_owned(),
                workflow: "feature_flow".to_owned(),
                step: "execute".to_owned(),
            },
            title: "Recovered".to_owned(),
            prompt: "Resume execute".to_owned(),
            output_schema: None,
            model: Some(AgentModel {
                instance_id: "devReplay".to_owned(),
                model: "dev-replay".to_owned(),
                options: None,
            }),
            project: Some("proj-crash".to_owned()),
            environment: Some("env-local".to_owned()),
            thread: None,
            issue: None,
            launch: Some(AgentLaunch {
                workspace: Some(json!({ "type": "worktree" })),
                runtime_mode: Some("approval-required".to_owned()),
                interaction_mode: None,
                stable_launch_key: Some("board:card-crash:execute".to_owned()),
            }),
            stable_launch_key: None,
        })
        .await?;
    assert_eq!(recovered_turn.thread_id, "thr-recovered-1");

    let conn = fx.orchestrator.sink().db().lock().map_err(|_| "db lock")?;
    let rec = get_thread_launch(&conn, "board:card-crash:execute")?.ok_or("missing rec")?;
    assert!(rec.thread_committed);
    assert!(rec.message_committed);
    let stored_wt: String = conn.query_row(
        "SELECT json_extract(payload_json, '$.worktreePath') FROM orchestration_v2_projection_threads WHERE thread_id = 'thr-recovered-1'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(stored_wt, crashed_wt.to_string_lossy());
    Ok(())
}

#[tokio::test]
async fn existing_worktree_and_deleted_thread_validation() -> TestResult {
    let fx = setup_crash_fixture()?;
    let launcher =
        ThreadLauncher::new(Arc::clone(&fx.orchestrator), fx.paths.worktrees_dir.clone());

    let missing_wt = launcher
        .launch_thread(make_launch_req(
            "cmd-missing-wt",
            None,
            false,
            WorkspaceStrategy::ExistingWorktree {
                worktree_path: fx.tmp.path().join("nope").to_string_lossy().into_owned(),
                branch: None,
            },
        )?)
        .await;
    assert!(matches!(
        missing_wt,
        Err(ThreadLaunchError::WorktreeNotFound(_))
    ));

    let wt_dir = fx.tmp.path().join("existing-wt");
    fs::create_dir_all(&wt_dir)?;
    let wt_str = wt_dir.to_string_lossy().into_owned();
    let created = launcher
        .launch_thread(make_launch_req(
            "cmd-existing-wt",
            Some("board:card-existing:plan"),
            true,
            WorkspaceStrategy::ExistingWorktree {
                worktree_path: wt_str.clone(),
                branch: None,
            },
        )?)
        .await?;
    assert_eq!(created.worktree_path.as_deref(), Some(wt_str.as_str()));

    fx.orchestrator
        .dispatch_now(&OrchestrationCommand::ThreadDelete {
            command_id: CommandId::new("cmd-del-rec")?,
            thread_id: ThreadId::new(created.thread_id.as_str())?,
            only_if_unused: None,
        })
        .await?;

    let reuse_deleted = launcher
        .launch_thread(make_launch_req(
            "cmd-reuse-deleted",
            Some("board:card-existing:plan"),
            true,
            WorkspaceStrategy::ExistingWorktree {
                worktree_path: wt_str,
                branch: None,
            },
        )?)
        .await;
    assert!(matches!(
        reuse_deleted,
        Err(ThreadLaunchError::ThreadDeleted(_))
    ));

    // Committed Worktree thread whose directory was deleted on disk must return WorktreeNotFound.
    let wt_created = launcher
        .launch_thread(make_launch_req(
            "cmd-wt-create-delete",
            Some("board:card-wt-deleted:plan"),
            false,
            WorkspaceStrategy::Worktree {
                base_ref: "HEAD".to_owned(),
                branch: None,
                start_from_origin: None,
                repositories: None,
            },
        )?)
        .await?;
    let wt_dir_created = wt_created
        .worktree_path
        .as_deref()
        .ok_or("missing worktree")?;
    assert!(Path::new(wt_dir_created).is_dir());
    fs::remove_dir_all(wt_dir_created)?;

    let resumed_deleted = launcher
        .launch_thread(make_launch_req(
            "cmd-wt-resume-deleted",
            Some("board:card-wt-deleted:plan"),
            false,
            WorkspaceStrategy::Worktree {
                base_ref: "HEAD".to_owned(),
                branch: None,
                start_from_origin: None,
                repositories: None,
            },
        )?)
        .await;
    assert!(matches!(
        resumed_deleted,
        Err(ThreadLaunchError::WorktreeNotFound(_))
    ));
    Ok(())
}
