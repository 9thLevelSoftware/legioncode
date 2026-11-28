//! Shared M3 repository test fixture helper (`setup_approved_execute_card`).

use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, ApprovePlanInput,
    ApproveSpecInput, CreateCardInput, LensInput, PlannedCheckInput, TaskContractInput,
    UpdatePlanInput, WorkflowBoardRepoError, approve_plan, approve_spec, create_card, update_plan,
};
use rusqlite::Connection;

pub fn setup_approved_execute_card(
    conn: &Connection,
    card_id: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    let t = "2026-11-24T00:00:00Z";
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            revision_id: &format!("rev-{card_id}-1"),
            project_id: "proj-1",
            title: "M3 Repo Test",
            brief: "Test M3 repos",
            intake_goal: Some("change"),
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "direct",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: Some("/repo/.wt/m3"),
            branch: Some("feat/m3"),
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Tests pass\"},{\"id\":\"AC-02\",\"text\":\"Manual check\"}]}",
            spec_digest: Some("sha256:spec-1"),
            wireframes_digest: None,
        },
        t,
    )?;
    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id,
            revision_id: &format!("rev-{card_id}-2"),
            approval_id: &format!("apr-{card_id}-spec"),
            expected_revision: 1,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    seed_default_plan(conn, card_id, t)?;
    let approved = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id,
            revision_id: &format!("rev-{card_id}-4"),
            approval_id: &format!("apr-{card_id}-plan"),
            expected_revision: 3,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    Ok(approved.card_detail.card.revision)
}

fn seed_default_plan(
    conn: &Connection,
    card_id: &str,
    t: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let tasks = default_tasks();
    let checks = [PlannedCheckInput {
        check_id: "chk-1",
        owner_task_id: Some("TSK-1"),
        argv: &cmd,
        label: Some("Run tests"),
    }];
    let ac_mappings = default_ac_mappings();
    let lenses = [
        "ux",
        "functional",
        "architecture",
        "quality",
        "infrastructure",
        "delivery",
    ]
    .map(|lens| LensInput {
        lens,
        applicable: Some(true),
        summary: Some("Covered"),
        reason: None,
    });
    let alternatives = [
        AlternativeInput {
            id: "a",
            title: "A",
            summary: "Normalized",
            tradeoffs: "Explicit tables",
        },
        AlternativeInput {
            id: "b",
            title: "B",
            summary: "Blob",
            tradeoffs: "Coarse",
        },
    ];
    update_plan(
        conn,
        &UpdatePlanInput {
            card_id,
            revision_id: &format!("rev-{card_id}-3"),
            expected_revision: 2,
            lenses: Some(&lenses),
            alternatives: Some(&alternatives),
            selected_alternative_id: AlternativeSelectionUpdate::Set("a"),
            selection_rationale: Some("Normalized tables"),
            revise_alternatives_feedback: None,
            strategy: Some("outcomes"),
            strategy_rationale: None,
            strategy_metadata_json: None,
            granularity: Some("balanced"),
            tasks: Some(&tasks),
            checks: Some(&checks),
            ac_mappings: Some(&ac_mappings),
            plan_json: None,
            plan_digest: Some("sha256:plan-1"),
            design_digest: None,
            tasks_digest: None,
        },
        t,
    )?;
    Ok(())
}

fn default_tasks() -> [TaskContractInput<'static>; 2] {
    [
        TaskContractInput {
            task_id: "TSK-1",
            title: "Task 1",
            depends_on: &[],
            blocks: &[],
            files_allowed: &["src/a.rs"],
            files_forbidden: &["src/secret.rs"],
            expected_artifacts: &[],
            verification_commands: &[],
            testing_method: "test-first",
            testing_limitations: None,
            status: None,
        },
        TaskContractInput {
            task_id: "TSK-2",
            title: "Task 2",
            depends_on: &["TSK-1"],
            blocks: &[],
            files_allowed: &["src/b.rs"],
            files_forbidden: &[],
            expected_artifacts: &[],
            verification_commands: &[],
            testing_method: "test-first",
            testing_limitations: None,
            status: None,
        },
    ]
}

fn default_ac_mappings() -> [AcMappingInput<'static>; 2] {
    [
        AcMappingInput {
            criterion_id: "AC-01",
            method: "automated",
            check_ids: &["chk-1"],
            command_argv: None,
            procedure: None,
            evidence_ref: None,
            external_source: None,
        },
        AcMappingInput {
            criterion_id: "AC-02",
            method: "manual",
            check_ids: &[],
            command_argv: None,
            procedure: Some("Inspect UI"),
            evidence_ref: None,
            external_source: None,
        },
    ]
}
