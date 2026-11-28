#![forbid(unsafe_code)]
//! Shared workspace and service bootstrap for M2 workflow board tests.

use std::fs;
use std::process::Command as StdCommand;
use std::sync::Arc;

use legion_contracts::ids::{EnvironmentId, ProviderInstanceId};
use legion_contracts::orchestration::ModelSelection;
use legion_orchestrator::Orchestrator;
use legion_persistence::paths::{ServerDerivedPaths, ensure_server_directories};
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_server_runtime::{
    OrchestratorWorkflowAgentDriver, ProjectWorkflowBoardService, ScmWorkflowForgeDriver,
};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::WorkflowsLocation;
use legion_workflows::{WorkflowStore, builtin_stage_definitions};
use tempfile::TempDir;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type M2BoardSetup = (
    TempDir,
    Arc<Orchestrator>,
    ProjectWorkflowBoardService,
    String,
);

pub fn setup_m2_board() -> Result<M2BoardSetup, Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let paths = ServerDerivedPaths::derive(tmp.path(), false, true);
    ensure_server_directories(&paths)?;
    let repo_dir = tmp.path().join("repo-main");
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
    fs::write(repo_dir.join("README.md"), "# Board Repo\n")?;
    StdCommand::new("git")
        .current_dir(&repo_dir)
        .args(["add", "."])
        .output()?;
    StdCommand::new("git")
        .current_dir(&repo_dir)
        .args(["commit", "-m", "init"])
        .output()?;
    let repo_str = repo_dir.to_string_lossy().into_owned();
    let store = SqliteStore::open(&paths.db_path)?;
    ensure_project_store_schema(store.connection())?;
    create_project(
        store.connection(),
        &CreateProjectInput::new(&repo_str)
            .with_project_id("proj-board-1")
            .with_title("Board Project"),
    )?;
    drop(store);
    let orch = Arc::new(Orchestrator::from_sqlite_store(SqliteStore::open(
        paths.db_path,
    )?));
    let env_id = EnvironmentId::new("env-board-test")?;
    let wf_mgr = WorkflowDefinitionsManager::open_with_builtins(
        WorkflowStore::new(Arc::clone(orch.sink().db()))?,
        Arc::new(
            OrchestratorWorkflowAgentDriver::new(Arc::clone(&orch), paths.worktrees_dir.clone())
                .with_settings_path(Some(paths.settings_path.clone()))
                .with_environment_id(env_id.clone()),
        ),
        Arc::new(ScmWorkflowForgeDriver::new(Arc::clone(orch.sink().db()))),
        WorkflowsLocation {
            directory: paths.state_dir.join("workflows"),
            single: None,
        },
        builtin_stage_definitions(),
        true,
    )?;
    let board = ProjectWorkflowBoardService::new(
        Arc::clone(&orch),
        paths.worktrees_dir,
        Some(paths.settings_path),
        env_id,
        Some(wf_mgr),
    );
    Ok((tmp, orch, board, repo_str))
}

pub fn replay_model() -> ModelSelection {
    ModelSelection {
        instance_id: ProviderInstanceId::new("devReplay").expect("valid provider"),
        model: "dev-replay".to_owned(),
        options: None,
    }
}
