#![forbid(unsafe_code)]
//! Integration tests for built-in `workflowBoard.plan@1` stage job, `open_with_builtins`,
//! non-blocking `start_agent_turn` + `wait` suspension, replay safety, and `build_plan_brief`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardAssistanceMode, WorkflowBoardIntakeGoal,
    WorkflowBoardPlanAlternative, WorkflowBoardPlanLens, WorkflowBoardPlanLensEntry,
    WorkflowBoardPlanningStrategy, WorkflowBoardTaskGranularity,
};
use legion_persistence::sqlite::SqliteStore;
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::definitions_fs::WorkflowsLocation;
use legion_workflows::traits::{
    AgentInterruptOutcome, AgentInterruptTarget, AgentModel, AgentResult, AgentStartInput,
    AgentTurn, BoxFuture, UnavailableForgeDriver, WorkflowAgentDriver, WorkflowAgentError,
};
use legion_workflows::{
    BriefChallengeConcern, PlanBriefInput, SpecBriefInput, WORKFLOW_BOARD_PLAN_NAME,
    WORKFLOW_BOARD_PLAN_VERSION, WORKFLOW_BOARD_SPEC_NAME, WORKFLOW_BOARD_SPEC_VERSION,
    WorkflowRunStatus, WorkflowStore, build_plan_brief, build_spec_brief,
    builtin_stage_definitions,
};
use serde_json::json;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct RecordingAgentDriver {
    starts: AtomicUsize,
    interrupts: AtomicUsize,
    last_input: Mutex<Option<AgentStartInput>>,
    last_interrupt: Mutex<Option<AgentInterruptTarget>>,
}

impl WorkflowAgentDriver for RecordingAgentDriver {
    fn start(
        &self,
        input: AgentStartInput,
    ) -> BoxFuture<'_, Result<AgentTurn, WorkflowAgentError>> {
        Box::pin(async move {
            let n = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
            if let Ok(mut g) = self.last_input.lock() {
                *g = Some(input);
            }
            Ok(AgentTurn {
                thread_id: format!("thr-plan-{n}"),
                turn_id: format!("turn-plan-{n}"),
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
            panic!("PlanStageDriver must not block on WorkflowAgentDriver::result");
        })
    }

    fn interrupt(
        &self,
        target: AgentInterruptTarget,
    ) -> BoxFuture<'_, Result<AgentInterruptOutcome, WorkflowAgentError>> {
        Box::pin(async move {
            self.interrupts.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut g) = self.last_interrupt.lock() {
                *g = Some(target);
            }
            Ok(AgentInterruptOutcome::Stopped)
        })
    }
}

fn open_store(dir: &std::path::Path) -> Result<WorkflowStore, Box<dyn std::error::Error>> {
    let sqlite = SqliteStore::open(dir.join("state.sqlite"))?;
    let db = Arc::new(Mutex::new(sqlite.into_connection()));
    Ok(WorkflowStore::new(db)?)
}

#[test]
fn build_plan_brief_renders_all_sections_deterministically() {
    let input = PlanBriefInput {
        card_id: "card-1".to_owned(),
        card_title: "Add Workflow Board".to_owned(),
        project_id: "proj-1".to_owned(),
        workspace_root: "/repo".to_owned(),
        worktree_path: Some("/repo/.worktrees/wt-1".to_owned()),
        branch: Some("feat/board".to_owned()),
        spec_revision: 3,
        spec_digest: "sha256:abc123".to_owned(),
        spec_brief: "Deliver four-column workflow board.".to_owned(),
        spec_markdown: "# Add Workflow Board\nBody".to_owned(),
        acceptance_criteria: vec![WorkflowBoardAcceptanceCriterion {
            id: "AC-01".to_owned(),
            text: "Survives restart".to_owned(),
        }],
        constraints: vec!["No git auto-commits".to_owned()],
        unresolved_decisions: vec!["Decide cache TTL".to_owned()],
        source_refs: vec!["rust/crates/legion-cli/src/lib.rs".to_owned()],
        wireframes_index: Some("wireframes/index.html".to_owned()),
        assistance_mode: WorkflowBoardAssistanceMode::Balanced,
        planning_strategy: Some(WorkflowBoardPlanningStrategy::RiskFirst),
        strategy_rationale: Some("Probe WAL first".to_owned()),
        task_granularity: Some(WorkflowBoardTaskGranularity::Balanced),
        exploration_summary: Some("Round 1 resolved 1 decision".to_owned()),
        resolved_decisions: vec!["DEC-1: Use SQLite".to_owned()],
        challenge_receipt_summary: Some("status=complete".to_owned()),
        challenge_concerns: vec![BriefChallengeConcern {
            id: "CON-1".to_owned(),
            summary: "WAL contention".to_owned(),
            disposition: "answer".to_owned(),
            operator_note: Some("Single writer lock".to_owned()),
        }],
        design_lenses: vec![WorkflowBoardPlanLensEntry {
            lens: WorkflowBoardPlanLens::Ux,
            applicable: Some(false),
            summary: None,
            reason: Some("Backend only".to_owned()),
        }],
        design_alternatives: vec![WorkflowBoardPlanAlternative {
            id: "alt-a".to_owned(),
            title: "Normalized".to_owned(),
            summary: "7 tables".to_owned(),
            tradeoffs: "More DDL".to_owned(),
        }],
        selected_alternative_id: Some("alt-a".to_owned()),
        selection_rationale: Some("Clean FKs".to_owned()),
        existing_tasks_summary: vec!["TSK-0001: Probe".to_owned()],
        expected_output_contract: String::new(),
    };
    let a = build_plan_brief(&input);
    let b = build_plan_brief(&input);
    assert_eq!(a, b);
    assert!(a.brief_digest.starts_with("sha256:"));
    assert!(a.markdown.contains("Approved Spec Revision: `3`"));
    assert!(a.markdown.contains("Approved Spec Digest: `sha256:abc123`"));
    assert!(a.markdown.contains("**AC-01**: Survives restart"));
    assert!(a.markdown.contains("No git auto-commits"));
    assert!(a.markdown.contains("Decide cache TTL"));
    assert!(a.markdown.contains("DEC-1: Use SQLite"));
    assert!(a.markdown.contains("WAL contention"));
    assert!(a.markdown.contains("Selected Alternative: `alt-a`"));
    assert!(a.markdown.contains("wireframes/index.html"));
    assert!(a.markdown.contains("six design lenses"));

    let spec_brief = build_spec_brief(&SpecBriefInput {
        card_id: "card-1".to_owned(),
        card_title: "Add Workflow Board".to_owned(),
        card_description: "Initial idea".to_owned(),
        project_id: "proj-1".to_owned(),
        workspace_root: "/repo".to_owned(),
        worktree_path: None,
        branch: None,
        card_revision: 1,
        assistance_mode: WorkflowBoardAssistanceMode::Guided,
        intake_goal: Some(WorkflowBoardIntakeGoal::Change),
        current_spec_markdown: None,
        acceptance_criteria: vec![],
        constraints: vec![],
        source_refs: vec!["README.md".to_owned()],
        wireframes_index: None,
        unresolved_decisions: vec!["DEC-1: Choose scope".to_owned()],
        expected_output_contract: String::new(),
    });
    assert!(spec_brief.brief_digest.starts_with("sha256:"));
    assert!(
        spec_brief
            .markdown
            .contains("# Spec Brief: Add Workflow Board")
    );
    assert!(
        spec_brief
            .markdown
            .contains("legion_workflow_board_propose")
    );
}

#[tokio::test]
async fn open_with_builtins_survives_reloads_and_rejects_fs_conflict() -> TestResult {
    let tmp = TempDir::new()?;
    let store = open_store(tmp.path())?;
    let wf_dir = tmp.path().join("workflows");
    std::fs::create_dir_all(&wf_dir)?;
    let agents = Arc::new(RecordingAgentDriver::default());
    let forge = Arc::new(UnavailableForgeDriver);
    let mgr = WorkflowDefinitionsManager::open_with_builtins(
        store,
        agents,
        forge,
        WorkflowsLocation {
            directory: wf_dir,
            single: None,
        },
        builtin_stage_definitions(),
        true,
    )?;

    assert!(
        mgr.engine()
            .summaries()
            .iter()
            .any(|s| s.name == WORKFLOW_BOARD_PLAN_NAME && s.version == WORKFLOW_BOARD_PLAN_VERSION)
    );
    assert!(
        mgr.engine()
            .summaries()
            .iter()
            .any(|s| s.name == WORKFLOW_BOARD_SPEC_NAME && s.version == WORKFLOW_BOARD_SPEC_VERSION)
    );

    let save_res = mgr.save(
        "user-custom.ts",
        r#"Legion.workflow({
  name: "user.custom",
  version: 1,
  payload: Schema.Struct({}),
  run: () => Effect.gen(function* () { return "ok"; })
});"#,
    )?;
    assert!(save_res.problems.is_empty(), "{:?}", save_res.problems);
    assert!(
        mgr.engine()
            .summaries()
            .iter()
            .any(|s| s.name == "user.custom")
    );
    assert!(
        mgr.engine()
            .summaries()
            .iter()
            .any(|s| s.name == WORKFLOW_BOARD_PLAN_NAME)
    );

    let conflict_res = mgr.save(
        "override-plan.ts",
        r#"Legion.workflow({
  name: "workflowBoard.plan",
  version: 1,
  payload: Schema.Struct({}),
  run: () => Effect.gen(function* () { return "hijack"; })
});"#,
    )?;
    assert!(
        !conflict_res.problems.is_empty(),
        "must reject user file redefining built-in workflowBoard.plan@1"
    );
    assert!(
        conflict_res
            .problems
            .iter()
            .any(|p| p.message.contains("built-in"))
    );

    let _ = mgr.remove("user-custom.ts")?;
    assert!(
        mgr.engine()
            .summaries()
            .iter()
            .any(|s| s.name == WORKFLOW_BOARD_PLAN_NAME)
    );
    Ok(())
}

#[tokio::test]
async fn plan_stage_driver_suspends_non_blocking_replays_safely_and_cancels() -> TestResult {
    let tmp = TempDir::new()?;
    let store = open_store(tmp.path())?;
    let wf_dir = tmp.path().join("workflows");
    let agents = Arc::new(RecordingAgentDriver::default());
    let forge = Arc::new(UnavailableForgeDriver);
    let mgr = WorkflowDefinitionsManager::open_with_builtins(
        store.clone(),
        agents.clone(),
        forge,
        WorkflowsLocation {
            directory: wf_dir,
            single: None,
        },
        builtin_stage_definitions(),
        false,
    )?;

    let payload = json!({
        "cardId": "card-1",
        "projectId": "proj-1",
        "title": "Plan Feature",
        "briefMarkdown": "# Brief\nDetails",
        "briefPath": "/repo/.legion/workflows/card-1/plan-brief.md",
        "specPath": "/repo/.legion/workflows/card-1/spec.md",
        "workspaceRoot": "/repo",
        "worktreePath": null,
        "modelSelection": AgentModel {
            instance_id: "devReplay".to_owned(),
            model: "dev-replay".to_owned(),
            options: None,
        },
        "runtimeMode": "full-access",
        "interactionMode": "default",
        "workspaceStrategy": { "type": "root" },
        "environmentId": "env-test",
        "stableLaunchKey": "board:card-1:plan:apr-1:attempt-1",
    });

    assert!(
        mgr.engine()
            .start(
                WORKFLOW_BOARD_PLAN_NAME,
                payload.clone(),
                "k-1",
                None,
                Some(1)
            )
            .await
            .is_err(),
        "workflowsEnabled=false must reject Plan stage start"
    );

    mgr.set_enabled(true);
    let engine = mgr.engine();
    let run_id = engine
        .start(WORKFLOW_BOARD_PLAN_NAME, payload, "k-1", None, Some(1))
        .await?;
    assert_eq!(agents.starts.load(Ordering::SeqCst), 1);

    let detail = engine.get(&run_id)?;
    assert_eq!(detail.run.status, WorkflowRunStatus::Waiting);
    let qt = store
        .latest_thread(&run_id)?
        .ok_or("missing latest_thread")?;
    assert_eq!(qt.thread_id, "thr-plan-1");

    // Replaying drive_run must NOT invoke agents.start a second time.
    engine.drive_run(&run_id).await?;
    assert_eq!(agents.starts.load(Ordering::SeqCst), 1);

    // Cancelling marks run Cancelled and interrupts the turn.
    engine
        .cancel(&run_id, Some("user cancelled".to_owned()))
        .await?;
    let after = engine.get(&run_id)?;
    assert_eq!(after.run.status, WorkflowRunStatus::Cancelled);
    assert_eq!(agents.interrupts.load(Ordering::SeqCst), 1);
    Ok(())
}
