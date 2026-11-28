#![forbid(unsafe_code)]
//! Unit & engine tests for M3 `workflowBoard.execute@1`, `workflowBoard.review@1`,
//! and `build_execute_brief` / `build_review_brief`.

use std::sync::{Arc, Mutex};

use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardAcceptanceCriterion, WorkflowBoardEvidenceMethod,
    WorkflowBoardPlannedCheck, WorkflowBoardTaskContract, WorkflowBoardTaskStatus,
    WorkflowBoardTaskTestingMethod,
};
use legion_persistence::sqlite::SqliteStore;
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::WorkflowsLocation;
use legion_workflows::traits::{
    AgentEnding, AgentInterruptOutcome, AgentInterruptTarget, AgentResult, AgentStartInput,
    AgentTurn, BoxFuture, UnavailableForgeDriver, WorkflowAgentDriver, WorkflowAgentError,
};
use legion_workflows::types::{SignalAddress, WorkflowRunStatus};
use legion_workflows::{
    ExecuteBriefInput, ReviewBriefInput, WORKFLOW_BOARD_EXECUTE_NAME,
    WORKFLOW_BOARD_EXECUTE_VERSION, WORKFLOW_BOARD_REVIEW_NAME, WORKFLOW_BOARD_REVIEW_VERSION,
    WorkflowStore, build_execute_brief, build_review_brief, builtin_stage_definitions,
};
use serde_json::json;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct StubAgentDriver;
impl WorkflowAgentDriver for StubAgentDriver {
    fn start(
        &self,
        input: AgentStartInput,
    ) -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>> {
        Box::pin(async move {
            Ok(AgentTurn {
                thread_id: format!("thr-stub-{}", input.origin.step),
                turn_id: "turn-1".to_owned(),
                environment: None,
                started_at: Some(1),
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
            Ok(AgentResult {
                ending: AgentEnding::Completed,
                detail: "done".to_owned(),
                reply: "ok".to_owned(),
                output: None,
            })
        })
    }

    fn interrupt(
        &self,
        _target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>> {
        Box::pin(async { Ok(AgentInterruptOutcome::Ended) })
    }
}

#[test]
fn builtins_register_execute_and_review_and_briefs_render_contracts() {
    let defs = builtin_stage_definitions();
    assert!(
        defs.iter().any(|d| d.name == WORKFLOW_BOARD_EXECUTE_NAME
            && d.version == WORKFLOW_BOARD_EXECUTE_VERSION)
    );
    assert!(defs.iter().any(
        |d| d.name == WORKFLOW_BOARD_REVIEW_NAME && d.version == WORKFLOW_BOARD_REVIEW_VERSION
    ));

    let exec_brief = build_execute_brief(&ExecuteBriefInput {
        card_id: "card-1".to_owned(),
        card_title: "Implement feature".to_owned(),
        project_id: "proj-1".to_owned(),
        workspace_root: "/ws".to_owned(),
        worktree_path: Some("/wt/card-1".to_owned()),
        branch: Some("feat/card-1".to_owned()),
        card_revision: 3,
        spec_digest: "sha256:spec1".to_owned(),
        plan_digest: "sha256:plan1".to_owned(),
        product_snapshot_digest: Some("sha256:snap1".to_owned()),
        task: WorkflowBoardTaskContract {
            id: None,
            task_id: "TSK-1".to_owned(),
            title: "Write widget".to_owned(),
            status: Some(WorkflowBoardTaskStatus::InProgress),
            depends_on: Some(Vec::new()),
            blocked_by: Some(Vec::new()),
            blocks: Some(Vec::new()),
            files_allowed: vec!["src/**".to_owned()],
            files_forbidden: Some(vec!["secrets/**".to_owned()]),
            expected_artifacts: Some(vec!["src/widget.rs".to_owned()]),
            verification_commands: Some(vec![vec!["cargo".to_owned(), "test".to_owned()]]),
            testing_method: WorkflowBoardTaskTestingMethod::RegressionFirst,
            testing_limitations: None,
        },
        completed_task_ids: Vec::new(),
        planned_checks: vec![WorkflowBoardPlannedCheck {
            id: "chk-1".to_owned(),
            argv: vec!["cargo".to_owned(), "test".to_owned()],
            criterion_id: Some("AC-01".to_owned()),
            description: Some("Run tests".to_owned()),
        }],
        acceptance_criteria: vec![WorkflowBoardAcceptanceCriterion {
            id: "AC-01".to_owned(),
            text: "Must compile".to_owned(),
        }],
        constraints: vec!["Must stay under 50ms".to_owned()],
        source_refs: vec!["src/widget.rs".to_owned()],
        expected_output_contract: "Call workflow_board_propose".to_owned(),
    });
    assert!(exec_brief.markdown.contains("TSK-1"));
    assert!(exec_brief.markdown.contains("src/**"));
    assert!(exec_brief.markdown.contains("secrets/**"));
    assert!(exec_brief.markdown.contains("Must stay under 50ms"));
    assert!(exec_brief.markdown.contains("src/widget.rs"));
    assert!(exec_brief.brief_digest.starts_with("sha256:"));

    let rev_brief = build_review_brief(&ReviewBriefInput {
        card_id: "card-1".to_owned(),
        card_title: "Implement feature".to_owned(),
        project_id: "proj-1".to_owned(),
        workspace_root: "/ws".to_owned(),
        worktree_path: Some("/wt/card-1".to_owned()),
        branch: Some("feat/card-1".to_owned()),
        card_revision: 4,
        spec_digest: "sha256:spec1".to_owned(),
        plan_digest: "sha256:plan1".to_owned(),
        product_snapshot_digest: "sha256:snap1".to_owned(),
        spec_summary: "Widget feature spec".to_owned(),
        acceptance_criteria: vec![WorkflowBoardAcceptanceCriterion {
            id: "AC-01".to_owned(),
            text: "Must compile".to_owned(),
        }],
        ac_mappings: vec![WorkflowBoardAcEvidenceMapping {
            criterion_id: "AC-01".to_owned(),
            method: WorkflowBoardEvidenceMethod::Automated,
            check_ids: Some(vec!["chk-1".to_owned()]),
            command: None,
            procedure: None,
            external_ref: None,
            external_source: None,
            limitations: None,
        }],
        task_summaries: vec!["TSK-1: completed".to_owned()],
        check_receipt_summaries: vec!["chk-1: passed (exit 0)".to_owned()],
        changed_files_summary: vec!["src/widget.rs".to_owned()],
        diff_excerpt: "+fn widget() {}".to_owned(),
        expected_output_contract: "Record findings with disposition = \"unresolved\"".to_owned(),
    });
    assert!(rev_brief.markdown.contains("AC-01"));
    assert!(rev_brief.markdown.contains("unresolved"));
    assert!(rev_brief.brief_digest.starts_with("sha256:"));
}

#[tokio::test]
async fn execute_and_review_stage_workflows_suspend_and_complete_on_signal() -> TestResult {
    let tmp = TempDir::new()?;
    let sqlite = SqliteStore::open_in_memory()?;
    let store = WorkflowStore::new(Arc::new(Mutex::new(sqlite.into_connection())))?;
    let mgr = WorkflowDefinitionsManager::open_with_builtins(
        store.clone(),
        Arc::new(StubAgentDriver),
        Arc::new(UnavailableForgeDriver),
        WorkflowsLocation {
            directory: tmp.path().join("wf"),
            single: None,
        },
        builtin_stage_definitions(),
        true,
    )?;

    for (name, ver, wait_key, key) in [
        (
            WORKFLOW_BOARD_EXECUTE_NAME,
            WORKFLOW_BOARD_EXECUTE_VERSION,
            "await_execute_completion",
            "k-exec",
        ),
        (
            WORKFLOW_BOARD_REVIEW_NAME,
            WORKFLOW_BOARD_REVIEW_VERSION,
            "await_review_completion",
            "k-rev",
        ),
    ] {
        let payload = json!({
            "cardId": "card-1",
            "projectId": "proj-1",
            "title": "Test Card",
            "briefMarkdown": "# Brief",
            "briefPath": "brief.md",
            "specPath": "spec.md",
            "workspaceRoot": "/tmp/ws",
            "modelSelection": { "instanceId": "devReplay", "model": "dev-replay" },
            "runtimeMode": "full-access",
            "interactionMode": "default",
            "workspaceStrategy": { "type": "root" },
            "environmentId": "env-1",
            "stableLaunchKey": key,
            "taskId": "TSK-1",
        });
        let run_id = mgr
            .engine()
            .start(name, payload, key, None, Some(ver))
            .await?;
        let mid = mgr.engine().get(&run_id)?;
        assert_eq!(mid.run.status, WorkflowRunStatus::Waiting);

        mgr.engine()
            .signal(
                SignalAddress::Run {
                    run_id: run_id.clone(),
                    wait: wait_key.to_owned(),
                },
                json!({ "status": "completed", "verdict": "pass" }),
                &format!("sig-{key}"),
                None,
            )
            .await?;
        let ended = mgr.engine().get(&run_id)?;
        assert_eq!(ended.run.status, WorkflowRunStatus::Completed);
    }
    Ok(())
}
