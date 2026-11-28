#![forbid(unsafe_code)]
//! Integration tests for built-in `workflowBoard.spec@1` stage job.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use legion_persistence::sqlite::SqliteStore;
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::WorkflowsLocation;
use legion_workflows::traits::{
    AgentInterruptOutcome, AgentInterruptTarget, AgentModel, AgentResult, AgentStartInput,
    AgentTurn, BoxFuture, UnavailableForgeDriver, WorkflowAgentDriver, WorkflowAgentError,
};
use legion_workflows::{
    WORKFLOW_BOARD_SPEC_NAME, WORKFLOW_BOARD_SPEC_VERSION, WorkflowRunStatus, WorkflowStore,
    builtin_stage_definitions,
};
use serde_json::json;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct CountingDriver {
    starts: AtomicUsize,
}

impl WorkflowAgentDriver for CountingDriver {
    fn start(
        &self,
        _input: AgentStartInput,
    ) -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>> {
        Box::pin(async move {
            let n = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(AgentTurn {
                thread_id: format!("thr-spec-{n}"),
                turn_id: format!("turn-spec-{n}"),
                environment: Some("env-test".to_owned()),
                started_at: Some(1_000),
                failed: None,
            })
        })
    }

    fn result(
        &self,
        _turn: AgentTurn,
        _quiet_ms: Option<i64>,
    ) -> BoxFuture<'_, Result<AgentResult, WorkflowAgentError>> {
        Box::pin(async {
            panic!("SpecStageDriver must not block on WorkflowAgentDriver::result");
        })
    }

    fn interrupt(
        &self,
        _target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>> {
        Box::pin(async { Ok(AgentInterruptOutcome::Stopped) })
    }
}

#[tokio::test]
async fn spec_stage_driver_suspends_non_blocking_replays_safely_and_cancels() -> TestResult {
    let tmp = TempDir::new()?;
    let store = WorkflowStore::new(Arc::new(std::sync::Mutex::new(
        SqliteStore::open(tmp.path().join("state.sqlite"))?.into_connection(),
    )))?;
    let agents = Arc::new(CountingDriver::default());
    let mgr = WorkflowDefinitionsManager::open_with_builtins(
        store,
        agents.clone(),
        Arc::new(UnavailableForgeDriver),
        WorkflowsLocation {
            directory: tmp.path().join("workflows"),
            single: None,
        },
        builtin_stage_definitions(),
        true,
    )?;
    assert!(
        builtin_stage_definitions()
            .iter()
            .any(|d| d.name == WORKFLOW_BOARD_SPEC_NAME && d.version == WORKFLOW_BOARD_SPEC_VERSION)
    );
    let engine = mgr.engine();
    let payload = json!({
        "cardId": "card-1",
        "projectId": "proj-1",
        "title": "Spec Feature",
        "briefMarkdown": "# Spec Brief\nDetails",
        "briefPath": "/repo/.legion/workflows/card-1/spec-brief.md",
        "specPath": "/repo/.legion/workflows/card-1/spec.md",
        "workspaceRoot": "/repo",
        "worktreePath": null,
        "modelSelection": AgentModel { instance_id: "devReplay".to_owned(), model: "dev-replay".to_owned(), options: None },
        "runtimeMode": "full-access",
        "interactionMode": "default",
        "workspaceStrategy": { "type": "root" },
        "environmentId": "env-test",
        "stableLaunchKey": "board:card-1:spec:rev-1:attempt-1",
    });
    let run_id = engine
        .start(WORKFLOW_BOARD_SPEC_NAME, payload, "k-spec-1", None, Some(1))
        .await?;
    assert_eq!(agents.starts.load(Ordering::SeqCst), 1);
    assert_eq!(engine.get(&run_id)?.run.status, WorkflowRunStatus::Waiting);
    engine.drive_run(&run_id).await?;
    assert_eq!(agents.starts.load(Ordering::SeqCst), 1);
    engine.cancel(&run_id, Some("stop".to_owned())).await?;
    assert_eq!(
        engine.get(&run_id)?.run.status,
        WorkflowRunStatus::Cancelled
    );
    Ok(())
}
