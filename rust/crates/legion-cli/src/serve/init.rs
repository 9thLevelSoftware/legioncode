//! Initialization helpers for `legioncode serve` runtime state and workflow paths.

use std::path::Path;
use std::sync::Arc;

use legion_contracts::ids::EnvironmentId;
use legion_orchestrator::Orchestrator;
use legion_persistence::paths::ServerDerivedPaths;
use legion_protocol::milestones::MilestoneHub;
use legion_server_runtime::{
    MilestoneHubWorkflowSink, OrchestratorWorkflowAgentDriver, ProjectWorkflowBoardService,
    RuntimeHttpState, ScmWorkflowForgeDriver, TerminalRuntime, start_orchestration_milestone_pump,
};
use legion_workflows::builtin_stage_definitions;
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::{
    LEGION_WORKFLOWS_ENV, T3_WORKFLOWS_ENV, WorkflowsLocation, resolve_workflows_location,
};
use legion_workflows::store::WorkflowStore;
use tokio::task::JoinHandle;

use super::{ServeConfig, ServeMode, rpc};

pub(super) fn init_workflow_and_bg_tasks(
    config: &ServeConfig,
    paths: &ServerDerivedPaths,
    environment_id: &EnvironmentId,
    orchestrator: &Arc<Orchestrator>,
    milestone_hub: &Arc<MilestoneHub>,
) -> (
    Option<WorkflowStore>,
    Option<Arc<WorkflowDefinitionsManager>>,
    Vec<JoinHandle<()>>,
) {
    let workflows_enabled = rpc::server_settings::read_settings_json(&paths.settings_path)
        .ok()
        .and_then(|v| {
            v.get("workflowsEnabled")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false);
    let wf_location = resolve_workflows_runtime_location(paths, &config.base_dir);
    let workflow_store = WorkflowStore::with_milestones(
        Arc::clone(orchestrator.sink().db()),
        Arc::new(MilestoneHubWorkflowSink::new(Arc::clone(milestone_hub))),
    )
    .ok();
    let workflow_manager = workflow_store.as_ref().and_then(|store| {
        WorkflowDefinitionsManager::open_with_builtins(
            store.clone(),
            Arc::new(
                OrchestratorWorkflowAgentDriver::new(
                    Arc::clone(orchestrator),
                    paths.worktrees_dir.clone(),
                )
                .with_settings_path(Some(paths.settings_path.clone()))
                .with_environment_id(environment_id.clone()),
            ),
            Arc::new(ScmWorkflowForgeDriver::new(Arc::clone(
                orchestrator.sink().db(),
            ))),
            wf_location,
            builtin_stage_definitions(),
            workflows_enabled,
        )
        .ok()
    });
    let mut bg_tasks = Vec::new();
    bg_tasks.push(start_orchestration_milestone_pump(
        Arc::clone(orchestrator),
        Arc::clone(milestone_hub),
    ));
    if let Some(ref mgr) = workflow_manager {
        bg_tasks.push(mgr.spawn_watcher());
    }
    (workflow_store, workflow_manager, bg_tasks)
}

pub(super) fn resolve_workflows_runtime_location(
    paths: &ServerDerivedPaths,
    base_dir: &Path,
) -> WorkflowsLocation {
    let has_override = std::env::var(LEGION_WORKFLOWS_ENV).is_ok_and(|s| !s.trim().is_empty())
        || std::env::var(T3_WORKFLOWS_ENV).is_ok_and(|s| !s.trim().is_empty());
    if has_override {
        return resolve_workflows_location(base_dir);
    }
    WorkflowsLocation {
        directory: paths.state_dir.join("workflows"),
        single: None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Internal server builder wiring initialized subsystems into RuntimeHttpState."
)]
pub(super) fn build_runtime_http_state(
    config: &ServeConfig,
    paths: &ServerDerivedPaths,
    bind_host: &str,
    port: u16,
    environment_id: &EnvironmentId,
    orchestrator: &Arc<Orchestrator>,
    terminal_runtime: &Arc<TerminalRuntime>,
    milestone_hub: &Arc<MilestoneHub>,
    workflow_manager: Option<&Arc<WorkflowDefinitionsManager>>,
    dev_url: Option<String>,
) -> Arc<RuntimeHttpState> {
    let dev_auth_token = legion_contracts::brand::ACTIVE_BRAND
        .read_env("DEV_AUTH_TOKEN")
        .map(|v| v.trim().to_owned());
    let auth_policy = if config.mode == ServeMode::Desktop {
        "desktop-managed-local"
    } else if config.lan || !matches!(bind_host, "127.0.0.1" | "localhost" | "::1") {
        "remote-reachable"
    } else {
        "loopback-browser"
    };
    let mut rt =
        RuntimeHttpState::new(Arc::clone(orchestrator.sink().db()), paths.logs_dir.clone())
            .with_static_dir(config.static_dir.clone())
            .with_dev_auth_token(dev_auth_token)
            .with_port(port)
            .with_auth_policy(auth_policy)
            .with_orchestrator(Arc::clone(orchestrator))
            .with_milestone_hub(Arc::clone(milestone_hub))
            .with_secrets_dir(paths.secrets_dir.clone())
            .with_terminal_runtime(Arc::clone(terminal_runtime));
    if let Some(mgr) = workflow_manager {
        rt = rt.with_workflow_manager(Arc::clone(mgr));
    }
    let workflow_board = Arc::new(ProjectWorkflowBoardService::new(
        Arc::clone(orchestrator),
        paths.worktrees_dir.clone(),
        Some(paths.settings_path.clone()),
        environment_id.clone(),
        workflow_manager.cloned(),
    ));
    rt = rt.with_workflow_board(workflow_board);
    if dev_url.is_some() {
        rt = rt.with_dev_url(dev_url);
    }
    Arc::new(rt)
}
