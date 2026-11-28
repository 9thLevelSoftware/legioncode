use legion_persistence::workflow_board_repo::types::{
    CreateCardInput, LinkStageRunThreadInput, UpsertStageRunInput,
};
use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, ApprovePlanInput,
    ApproveSpecInput, ClaimOrStartTaskInput, LensInput, PlannedCheckInput, TaskContractInput,
    UpdatePlanInput, approve_plan, approve_spec, claim_or_start_task, create_card,
    link_stage_run_thread, update_plan, upsert_or_get_stage_run_by_launch_key,
};
use rusqlite::Connection;

const TS: &str = "2026-11-24T12:00:00Z";
type TestResult = Result<(), Box<dyn std::error::Error>>;

pub fn seed_execute_card(conn: &Connection, card_id: &str) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            project_id: "proj-mcp-m3",
            revision_id: &format!("rev-{card_id}-1"),
            title: "M3 MCP Card",
            brief: "Test scoped MCP execute and review",
            source_type: "idea",
            assistance_mode: "direct",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            wireframes_digest: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Check passes\"}]}",
            spec_digest: Some("sha256:spec-m3"),
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
        },
        TS,
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
        TS,
    )?;
    let approved_rev = seed_execute_plan(conn, card_id)?;
    seed_execute_stage_run(conn, card_id, approved_rev)
}

fn seed_execute_plan(conn: &Connection, card_id: &str) -> Result<i64, Box<dyn std::error::Error>> {
    let cmd = vec!["git".to_owned(), "status".to_owned(), "--short".to_owned()];
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
        applicable: Some(false),
        summary: None,
        reason: Some("Test fixture"),
    });
    let alts = [
        AlternativeInput {
            id: "a",
            title: "A",
            summary: "Direct",
            tradeoffs: "Simple",
        },
        AlternativeInput {
            id: "b",
            title: "B",
            summary: "Complex",
            tradeoffs: "Heavy",
        },
    ];
    let allowed = ["src/allowed.txt"];
    let forbidden = ["secrets/key.txt"];
    let tasks = [TaskContractInput {
        task_id: "TSK-1",
        title: "Allowed edit",
        depends_on: &[],
        blocks: &[],
        files_allowed: &allowed,
        files_forbidden: &forbidden,
        expected_artifacts: &allowed,
        verification_commands: std::slice::from_ref(&cmd),
        testing_method: "regression-first",
        testing_limitations: None,
        status: None,
    }];
    let checks = [PlannedCheckInput {
        check_id: "chk-ok",
        owner_task_id: Some("TSK-1"),
        argv: &cmd,
        label: Some("Git status check"),
    }];
    let check_ids = ["chk-ok"];
    let acs = [AcMappingInput {
        criterion_id: "AC-01",
        method: "automated",
        check_ids: &check_ids,
        command_argv: None,
        procedure: None,
        evidence_ref: None,
        external_source: None,
    }];
    update_plan(
        conn,
        &UpdatePlanInput {
            card_id,
            revision_id: &format!("rev-{card_id}-3"),
            expected_revision: 2,
            lenses: Some(&lenses),
            alternatives: Some(&alts),
            selected_alternative_id: AlternativeSelectionUpdate::Set("a"),
            selection_rationale: Some("Direct"),
            revise_alternatives_feedback: None,
            strategy: Some("outcomes"),
            strategy_rationale: None,
            strategy_metadata_json: None,
            granularity: Some("balanced"),
            tasks: Some(&tasks),
            checks: Some(&checks),
            ac_mappings: Some(&acs),
            plan_json: Some("{\"summary\":\"Plan\"}"),
            plan_digest: Some("sha256:plan-m3"),
            design_digest: None,
            tasks_digest: Some("sha256:tasks-m3"),
        },
        TS,
    )?;
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
        TS,
    )?;
    Ok(approved.card_detail.card.revision)
}

fn seed_execute_stage_run(conn: &Connection, card_id: &str, approved_rev: i64) -> TestResult {
    upsert_or_get_stage_run_by_launch_key(
        conn,
        &UpsertStageRunInput {
            id: "sr-mcp-exec",
            card_id,
            stage: "execute",
            task_id: Some("TSK-1"),
            attempt: 1,
            stable_launch_key: "wb:mcp-m3:exec:1",
            card_revision: approved_rev,
            provider_instance_id: "devReplay",
            model: "dev-replay",
            model_options_json: None,
            runtime_mode: "full-access",
            interaction_mode: "default",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            brief_digest: None,
        },
        TS,
    )?;
    link_stage_run_thread(
        conn,
        &LinkStageRunThreadInput {
            id: "sr-mcp-exec",
            status: "running",
            thread_id: Some("thr-mcp-exec"),
            workflow_run_id: None,
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: None,
        },
        TS,
    )?;
    claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id,
            revision_id: &format!("rev-{card_id}-5"),
            expected_revision: approved_rev,
            task_id: "TSK-1",
            stage_run_id: Some("sr-mcp-exec"),
        },
        TS,
    )?;
    Ok(())
}
