//! Shared M1/M2 fixture builders for `workflow_board` contract tests.

use legion_contracts::ids::{
    ProjectId, RunId, ThreadId, WorkflowBoardApprovalId, WorkflowBoardStageRunId, WorkflowCardId,
};
use legion_contracts::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use legion_contracts::policy::RuntimeMode;
use legion_contracts::workflow_board::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardApprovalGate, WorkflowBoardApprovalReceipt,
    WorkflowBoardApprovalStatus, WorkflowBoardAssistanceMode, WorkflowBoardCardStatus,
    WorkflowBoardCardSummary, WorkflowBoardRevisionEntry, WorkflowBoardSpecDocument,
    WorkflowBoardSpecSourceType, WorkflowBoardStage, WorkflowBoardStageRunLink,
    WorkflowBoardStageRunStage, WorkflowBoardStageRunStatus, WorkflowCardDetail,
};
use serde_json::json;

pub fn sample_summary() -> WorkflowBoardCardSummary {
    WorkflowBoardCardSummary {
        card_id: WorkflowCardId::new("card-1").expect("card id"),
        project_id: ProjectId::new("proj-1").expect("project id"),
        title: "Add project workflow board".to_owned(),
        stage: WorkflowBoardStage::Plan,
        status: WorkflowBoardCardStatus::Active,
        revision: 3,
        current_task: None,
        blocker_summary: None,
        next_action: Some("start_plan".to_owned()),
        spec_approved_revision: Some(3),
        spec_approval_status: Some(WorkflowBoardApprovalStatus::Valid),
        active_thread_id: Some(ThreadId::new("thr-1").expect("thread id")),
        created_at: "2026-11-24T00:00:00Z".to_owned(),
        updated_at: "2026-11-24T01:00:00Z".to_owned(),
        plan_approved_revision: None,
        plan_approval_status: None,
        incomplete_journal_count: None,
        has_unreconciled_edits: None,
        delivery_status: None,
    }
}

fn sample_spec() -> WorkflowBoardSpecDocument {
    WorkflowBoardSpecDocument {
        title: "Add project workflow board".to_owned(),
        brief: "Ship a 4-stage board".to_owned(),
        source_type: WorkflowBoardSpecSourceType::Idea,
        assistance_mode: WorkflowBoardAssistanceMode::Balanced,
        acceptance_criteria: vec![WorkflowBoardAcceptanceCriterion {
            id: "AC-01".to_owned(),
            text: "Card persists across restart".to_owned(),
        }],
        constraints: vec!["No auto-commits".to_owned()],
        source_refs: vec!["README.md".to_owned()],
        wireframes_index: None,
        spec_digest: Some("sha256:abc123".to_owned()),
        updated_at: "2026-11-24T00:30:00Z".to_owned(),
        intake_goal: None,
        brief_path: None,
        brief_digest: None,
        source_digest: None,
        source_input_digests: Vec::new(),
        intake_analysis: None,
        wireframes_digest: None,
    }
}

pub fn sample_detail() -> WorkflowCardDetail {
    let summary = sample_summary();
    let model_selection: ModelSelection =
        serde_json::from_value(json!({ "instanceId": "devReplay", "model": "dev-replay" }))
            .expect("model selection");
    WorkflowCardDetail {
        summary,
        workspace_strategy: WorkspaceStrategy::ExistingWorktree {
            worktree_path: "/repo/.worktrees/card-1".to_owned(),
            branch: Some("feat/card-1".to_owned()),
        },
        model_selection: Some(model_selection.clone()),
        runtime_mode: Some(RuntimeMode::AutoAcceptEdits),
        interaction_mode: Some(ProviderInteractionMode::Plan),
        spec: sample_spec(),
        spec_approval: Some(WorkflowBoardApprovalReceipt {
            id: WorkflowBoardApprovalId::new("apr-1").expect("approval id"),
            gate: WorkflowBoardApprovalGate::Spec,
            status: WorkflowBoardApprovalStatus::Valid,
            revision: 3,
            artifact_digest: "sha256:abc123".to_owned(),
            spec_digest: Some("sha256:abc123".to_owned()),
            wireframes_digest: None,
            approved_at: "2026-11-24T00:45:00Z".to_owned(),
            approved_by: "client".to_owned(),
            plan_digest: None,
            tasks_digest: None,
            checks_digest: None,
            acceptance_criteria_ids: None,
            product_snapshot_digest: None,
        }),
        stage_runs: vec![WorkflowBoardStageRunLink {
            id: WorkflowBoardStageRunId::new("sr-1").expect("stage run id"),
            stage: WorkflowBoardStageRunStage::Plan,
            task_id: None,
            attempt: None,
            stable_launch_key: "card-1:plan:1".to_owned(),
            status: WorkflowBoardStageRunStatus::Running,
            card_revision: 3,
            run_id: Some(RunId::new("run-1").expect("run id")),
            thread_id: Some(ThreadId::new("thr-1").expect("thread id")),
            project_id: ProjectId::new("proj-1").expect("project id"),
            workspace_strategy: WorkspaceStrategy::Root { branch: None },
            worktree_path: Some("/repo/.worktrees/card-1".to_owned()),
            branch: Some("feat/card-1".to_owned()),
            model_selection,
            runtime_mode: RuntimeMode::AutoAcceptEdits,
            interaction_mode: ProviderInteractionMode::Plan,
            last_error: None,
            created_at: "2026-11-24T00:50:00Z".to_owned(),
            updated_at: "2026-11-24T01:00:00Z".to_owned(),
            ended_at: None,
        }],
        revisions: vec![WorkflowBoardRevisionEntry {
            revision: 1,
            stage: WorkflowBoardStage::Spec,
            kind: "card_created".to_owned(),
            summary: "Created card".to_owned(),
            spec_digest: None,
            created_at: "2026-11-24T00:00:00Z".to_owned(),
            plan_digest: None,
        }],
        artifact_keys: vec![
            "card.json".to_owned(),
            "spec.md".to_owned(),
            "plan-brief.md".to_owned(),
        ],
        wireframes_index: None,
        wireframes_digest: None,
        exploration: None,
        challenge_receipt: None,
        max_challenge_concerns: None,
        plan: None,
        plan_approval: None,
        lenses: Vec::new(),
        alternatives: Vec::new(),
        alternatives_revision_count: None,
        selected_alternative_id: None,
        selection_rationale: None,
        strategy: None,
        strategy_rationale: None,
        strategy_metadata: None,
        granularity: None,
        tasks: Vec::new(),
        checks: Vec::new(),
        ac_mappings: Vec::new(),
        history: Vec::new(),
        task_executions: Vec::new(),
        file_contract_reports: Vec::new(),
        check_receipts: Vec::new(),
        review_report: None,
        ac_observations: Vec::new(),
        ship_readiness: None,
        delivery_receipt: None,
        recovery_state: None,
        amendments: Vec::new(),
        cli_import: None,
    }
}
