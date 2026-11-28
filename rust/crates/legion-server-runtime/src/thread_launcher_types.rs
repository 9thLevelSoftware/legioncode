//! Public request, result, and error types for [`ThreadLauncher`](crate::thread_launcher::ThreadLauncher).

use legion_contracts::ids::{CommandId, ThreadId};
use legion_contracts::orchestration::{
    CreationSource, DelegatedFrom, InitialThreadMessage, LinkOrigin, ModelSelection,
    OrchestrationActor, ProviderInteractionMode, WorkflowOrigin, WorkspaceStrategy,
};
use legion_contracts::policy::RuntimeMode;
use legion_orchestrator::projections::WireThreadProjection;
use serde_json::Value;

/// Validated parameters for launching or idempotently resuming a thread.
#[derive(Debug, Clone)]
pub struct LaunchThreadRequest {
    /// Idempotency command ID for `ThreadCreate`.
    pub command_id: CommandId,
    /// Actor creating the thread.
    pub created_by: OrchestrationActor,
    /// Client/surface initiating the launch.
    pub creation_source: CreationSource,
    /// Optional caller-supplied thread ID.
    pub thread_id: Option<ThreadId>,
    /// Optional stable launch identity persisted in `orchestration_v2_thread_launch_workflows`.
    pub stable_launch_key: Option<String>,
    /// Reuse existing thread if `thread_id` (or `stable_launch_key`) already exists.
    pub reuse_existing_thread: bool,
    /// Project ID or workspace path hint (must resolve to an active project).
    pub project_id: String,
    /// Display title.
    pub title: String,
    /// Whether the thread title was explicitly set (`titleRenamed`).
    pub title_renamed: Option<bool>,
    /// Whether to generate an LLM title asynchronously.
    pub generate_title: Option<bool>,
    /// Explicit provider instance + model selection.
    pub model_selection: ModelSelection,
    /// Explicit runtime permission mode.
    pub runtime_mode: RuntimeMode,
    /// Explicit provider interaction mode.
    pub interaction_mode: ProviderInteractionMode,
    /// Workspace strategy (`Root`, `ExistingWorktree`, or `Worktree`).
    pub workspace_strategy: WorkspaceStrategy,
    /// Optional workflow run provenance stored via event-sourcing (`$.workflowRun`).
    pub workflow_run: Option<WorkflowOrigin>,
    /// Optional MCP link provenance.
    pub link_origin: Option<LinkOrigin>,
    /// Optional remote parent provenance.
    pub delegated_from: Option<DelegatedFrom>,
    /// Optional atomic initial message.
    pub initial_message: Option<InitialThreadMessage>,
}

/// Result returned by [`ThreadLauncher::launch_thread`](crate::thread_launcher::ThreadLauncher::launch_thread).
#[derive(Debug, Clone)]
pub struct LaunchThreadResult {
    /// Target thread ID (created or resumed).
    pub thread_id: ThreadId,
    /// Latest run ID on the thread, if any.
    pub run_id: Option<String>,
    /// Full materialized wire thread projection.
    pub projection: WireThreadProjection,
    /// True when an existing committed thread was reused.
    pub resumed: bool,
    /// Projection snapshot / receipt sequence.
    pub sequence: i64,
    /// Bound branch name, if any.
    pub branch: Option<String>,
    /// Bound worktree path, if any.
    pub worktree_path: Option<String>,
}

pub(crate) fn build_resumed_result(
    thread_id: ThreadId,
    projection: WireThreadProjection,
    sequence: i64,
    branch: Option<String>,
    worktree_path: Option<String>,
) -> LaunchThreadResult {
    let run_id = projection
        .runs
        .last()
        .and_then(|r| r.get("id").and_then(Value::as_str))
        .map(str::to_owned);
    LaunchThreadResult {
        thread_id,
        run_id,
        projection,
        resumed: true,
        sequence,
        branch,
        worktree_path,
    }
}
