//! Integration regression tests for M3.1 Iteration 6 remediation items `11.1` and `11.7`:
//! `propose_amendment` resets `'in_progress'` tasks + clears `current_task_id`,
//! and rejects forward `'spec' -> 'plan'` transitions with `InvalidStateTransition`.

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AlternativeSelectionUpdate, ApprovePlanInput, ClaimOrStartTaskInput, CreateCardInput,
    ProposeAmendmentInput, TaskContractInput, UpdatePlanInput, WorkflowBoardRepoError,
    approve_plan, claim_or_start_task, create_card, propose_amendment, update_plan,
};
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn partial_plan_update<'a>(
    card_id: &'a str,
    revision_id: &'a str,
    expected_revision: i64,
    plan_digest: &'a str,
) -> UpdatePlanInput<'a> {
    UpdatePlanInput {
        card_id,
        revision_id,
        expected_revision,
        lenses: None,
        alternatives: None,
        selected_alternative_id: AlternativeSelectionUpdate::Keep,
        selection_rationale: None,
        revise_alternatives_feedback: None,
        strategy: None,
        strategy_rationale: None,
        strategy_metadata_json: None,
        granularity: None,
        tasks: None,
        checks: None,
        ac_mappings: None,
        plan_json: None,
        plan_digest: Some(plan_digest),
        design_digest: None,
        tasks_digest: None,
    }
}

fn setup_two_independent_tasks_card(
    conn: &rusqlite::Connection,
    card_id: &str,
    t: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    let rev = setup_approved_execute_card(conn, card_id)?;
    let tasks = [
        TaskContractInput {
            task_id: "TSK-1",
            title: "Independent Task 1",
            depends_on: &[],
            blocks: &[],
            files_allowed: &["src/a.rs"],
            files_forbidden: &[],
            expected_artifacts: &[],
            verification_commands: &[],
            testing_method: "test-first",
            testing_limitations: None,
            status: None,
        },
        TaskContractInput {
            task_id: "TSK-2",
            title: "Independent Task 2",
            depends_on: &[],
            blocks: &[],
            files_allowed: &["src/b.rs"],
            files_forbidden: &[],
            expected_artifacts: &[],
            verification_commands: &[],
            testing_method: "test-first",
            testing_limitations: None,
            status: None,
        },
    ];
    let upd = update_plan(
        conn,
        &UpdatePlanInput {
            tasks: Some(&tasks),
            ..partial_plan_update(card_id, &format!("r-ind-{card_id}"), rev, "sha256:plan-ind")
        },
        t,
    )?;
    let app = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id,
            revision_id: &format!("r-ind-app-{card_id}"),
            approval_id: &format!("apr-ind-{card_id}"),
            expected_revision: upd.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    Ok(app.card_detail.card.revision)
}

#[test]
fn propose_amendment_in_progress_reset_and_spec_stage_guard_probes_111_117() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T08:00:00Z";

    verify_spec_stage_amendment_rejected(conn, t)?;
    verify_in_progress_amendment_resets_task(conn, t)
}

fn verify_spec_stage_amendment_rejected(conn: &rusqlite::Connection, t: &str) -> TestResult {
    // Probe 11.7: propose_amendment("plan") on a 'spec'-stage card fails with InvalidStateTransition
    create_card(
        conn,
        &CreateCardInput {
            card_id: "card-117",
            revision_id: "r-117-c",
            project_id: "proj-1",
            title: "Spec Card",
            brief: "Brief",
            intake_goal: None,
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "direct",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Works\"}]}",
            spec_digest: Some("sha256:spec-117"),
            wireframes_digest: None,
        },
        t,
    )?;
    assert!(matches!(
        propose_amendment(
            conn,
            &ProposeAmendmentInput {
                card_id: "card-117",
                revision_id: "r-117-fail",
                amendment_id: "am-117",
                expected_revision: 1,
                target_stage: "plan",
                reason: "Illegal forward amendment",
                summary: None,
                source_task_id: None,
                unexpected_files: &[],
                proposed_changes_json: "{}",
                proposed_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    Ok(())
}

fn verify_in_progress_amendment_resets_task(conn: &rusqlite::Connection, t: &str) -> TestResult {
    // Probe 11.1: claim TSK-1 -> propose_amendment("plan") clears current_task_id & resets TSK-1 -> todo
    let rev = setup_two_independent_tasks_card(conn, "card-111", t)?;
    let s1 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-111",
            revision_id: "r-111-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let amended = propose_amendment(
        conn,
        &ProposeAmendmentInput {
            card_id: "card-111",
            revision_id: "r-111-am",
            amendment_id: "am-111",
            expected_revision: s1.planning.card_detail.card.revision,
            target_stage: "plan",
            reason: "Adjust plan before completing TSK-1",
            summary: None,
            source_task_id: Some("TSK-1"),
            unexpected_files: &[],
            proposed_changes_json: "{}",
            proposed_by: "client",
        },
        t,
    )?;
    assert_eq!(amended.planning.card_detail.card.stage, "plan");
    assert_eq!(amended.planning.card_detail.card.current_task_id, None);
    assert_eq!(amended.planning.tasks[0].status, "todo");

    let upd = update_plan(
        conn,
        &partial_plan_update(
            "card-111",
            "r-111-upd",
            amended.planning.card_detail.card.revision,
            "sha256:plan-111-v2",
        ),
        t,
    )?;
    let app = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-111",
            revision_id: "r-111-app",
            approval_id: "apr-111-2",
            expected_revision: upd.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    let claimed_t2 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-111",
            revision_id: "r-111-s2",
            expected_revision: app.card_detail.card.revision,
            task_id: "TSK-2",
            stage_run_id: None,
        },
        t,
    )?;
    assert_eq!(
        claimed_t2
            .planning
            .card_detail
            .card
            .current_task_id
            .as_deref(),
        Some("TSK-2")
    );
    Ok(())
}
