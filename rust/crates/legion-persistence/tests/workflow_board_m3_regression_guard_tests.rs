//! Integration regression tests for Challenger 2 Iteration 4 probes 10.5 and 10.6:
//! - 10.5: partial regression (`update_plan(tasks: None)`, `update_spec`, `return_for_revision`) resets `'in_progress'` tasks to `'todo'`
//! - 10.6: `approve_plan` re-entering `'execute'` immediately restores `'blocked'` status for remaining failed tasks

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AlternativeSelectionUpdate, ApprovePlanInput, ApproveSpecInput, CardExecutionDetail,
    ClaimOrStartTaskInput, CompleteTaskInput, ReturnForRevisionInput, TaskContractInput,
    UpdatePlanInput, UpdateSpecInput, WorkflowBoardRepoError, approve_plan, approve_spec,
    claim_or_start_task, complete_task, get_card_execution_detail, return_for_revision,
    update_plan, update_spec,
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

fn spec_update_brief<'a>(
    card_id: &'a str,
    revision_id: &'a str,
    expected_revision: i64,
    brief: &'a str,
    spec_digest: &'a str,
) -> UpdateSpecInput<'a> {
    UpdateSpecInput {
        card_id,
        revision_id,
        expected_revision,
        title: None,
        brief: Some(brief),
        intake_goal: None,
        input_paths_json: None,
        spec_json: None,
        spec_digest: Some(spec_digest),
        wireframes_digest: None,
        clear_wireframes_digest: false,
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

fn claim_task(
    conn: &rusqlite::Connection,
    card_id: &str,
    revision_id: &str,
    expected_revision: i64,
    task_id: &str,
    t: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id,
            revision_id,
            expected_revision,
            task_id,
            stage_run_id: None,
        },
        t,
    )
}

#[test]
fn partial_regression_resets_in_progress_task_and_allows_independent_task_probe_105() -> TestResult
{
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T06:00:00Z";

    // 1. update_plan(tasks: None) resets in_progress TSK-1 -> todo
    let rev_a = setup_two_independent_tasks_card(conn, "card-105-plan", t)?;
    let s1 = claim_task(conn, "card-105-plan", "r-105p-s1", rev_a, "TSK-1", t)?;
    let regressed_plan = update_plan(
        conn,
        &partial_plan_update(
            "card-105-plan",
            "r-105p-upd",
            s1.planning.card_detail.card.revision,
            "sha256:plan-105p-v2",
        ),
        t,
    )?;
    assert_eq!(regressed_plan.card_detail.card.current_task_id, None);
    assert_eq!(regressed_plan.tasks[0].status, "todo");
    let reapp_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-105-plan",
            revision_id: "r-105p-app",
            approval_id: "apr-105p-2",
            expected_revision: regressed_plan.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    let claimed_t2 = claim_task(
        conn,
        "card-105-plan",
        "r-105p-s2",
        reapp_plan.card_detail.card.revision,
        "TSK-2",
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

    // 2. update_spec and return_for_revision also reset in_progress TSK-1 -> todo
    verify_in_progress_reset_via_spec_and_return(conn, t)
}

fn verify_in_progress_reset_via_spec_and_return(
    conn: &rusqlite::Connection,
    t: &str,
) -> TestResult {
    for (card_id, via_spec) in [("card-105-spec", true), ("card-105-ret", false)] {
        let rev = setup_two_independent_tasks_card(conn, card_id, t)?;
        let started = claim_task(conn, card_id, &format!("r-s1-{card_id}"), rev, "TSK-1", t)?;
        let plan_rev = if via_spec {
            let us = update_spec(
                conn,
                &spec_update_brief(
                    card_id,
                    &format!("r-us-{card_id}"),
                    started.planning.card_detail.card.revision,
                    "Revised spec with in-progress task",
                    "sha256:spec-105-v2",
                ),
                t,
            )?;
            let exec = get_card_execution_detail(conn, card_id)?;
            assert_eq!(exec.planning.tasks[0].status, "todo");
            let aps = approve_spec(
                conn,
                &ApproveSpecInput {
                    card_id,
                    revision_id: &format!("r-as-{card_id}"),
                    approval_id: &format!("apr-as-{card_id}"),
                    expected_revision: us.card.revision,
                    expected_spec_digest: None,
                    wireframes_digest: None,
                    approved_by: "client",
                },
                t,
            )?;
            aps.card.revision
        } else {
            let ret = return_for_revision(
                conn,
                &ReturnForRevisionInput {
                    card_id,
                    revision_id: &format!("r-ret-{card_id}"),
                    expected_revision: started.planning.card_detail.card.revision,
                    target_stage: "plan",
                    reason: "Return while task in progress",
                },
                t,
            )?;
            assert_eq!(ret.card_detail.card.current_task_id, None);
            assert_eq!(ret.tasks[0].status, "todo");
            ret.card_detail.card.revision
        };
        let app = approve_plan(
            conn,
            &ApprovePlanInput {
                card_id,
                revision_id: &format!("r-ap-{card_id}"),
                approval_id: &format!("apr-ap-{card_id}"),
                expected_revision: plan_rev,
                expected_spec_digest: None,
                expected_plan_digest: None,
                approved_by: "client",
            },
            t,
        )?;
        let claimed = claim_task(
            conn,
            card_id,
            &format!("r-s2-{card_id}"),
            app.card_detail.card.revision,
            "TSK-2",
            t,
        )?;
        assert_eq!(
            claimed.planning.card_detail.card.current_task_id.as_deref(),
            Some("TSK-2")
        );
    }
    Ok(())
}

#[test]
fn failed_task_restores_blocked_status_on_plan_reapproval_probe_106() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T06:30:00Z";

    verify_failed_task_restores_blocked(conn, "card-106-plan", false, t)?;
    verify_failed_task_restores_blocked(conn, "card-106-spec", true, t)
}

fn verify_failed_task_restores_blocked(
    conn: &rusqlite::Connection,
    card_id: &str,
    via_spec: bool,
    t: &str,
) -> TestResult {
    let rev = setup_approved_execute_card(conn, card_id)?;
    let s1 = claim_task(conn, card_id, &format!("r-s1-{card_id}"), rev, "TSK-1", t)?;
    let failed = complete_task(
        conn,
        &CompleteTaskInput {
            card_id,
            revision_id: &format!("r-f1-{card_id}"),
            expected_revision: s1.planning.card_detail.card.revision,
            task_id: "TSK-1",
            outcome_status: "failed",
            stage_run_id: None,
            thread_id: None,
            workflow_run_id: None,
            diff_digest: None,
            product_snapshot_digest: None,
            error_message: Some("compilation failed"),
            violation_id: None,
            changed_files: &[],
            unexpected_files: &[],
            forbidden_files: &[],
        },
        t,
    )?;
    assert_eq!(failed.planning.card_detail.card.status, "blocked");

    let plan_rev = if via_spec {
        let us = update_spec(
            conn,
            &spec_update_brief(
                card_id,
                &format!("r-us-{card_id}"),
                failed.planning.card_detail.card.revision,
                "Spec update on failed task",
                "sha256:spec-106-v2",
            ),
            t,
        )?;
        let aps = approve_spec(
            conn,
            &ApproveSpecInput {
                card_id,
                revision_id: &format!("r-as-{card_id}"),
                approval_id: &format!("apr-as-{card_id}"),
                expected_revision: us.card.revision,
                expected_spec_digest: None,
                wireframes_digest: None,
                approved_by: "client",
            },
            t,
        )?;
        aps.card.revision
    } else {
        let up = update_plan(
            conn,
            &partial_plan_update(
                card_id,
                &format!("r-up-{card_id}"),
                failed.planning.card_detail.card.revision,
                "sha256:plan-106-v2",
            ),
            t,
        )?;
        assert_eq!(up.card_detail.card.status, "active");
        up.card_detail.card.revision
    };

    let app = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id,
            revision_id: &format!("r-ap-{card_id}"),
            approval_id: &format!("apr-ap-{card_id}"),
            expected_revision: plan_rev,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    let c = &app.card_detail.card;
    assert_eq!(
        (c.stage.as_str(), c.status.as_str()),
        ("execute", "blocked")
    );
    let expected_reason = Some("Task execution failed or blocked");
    assert_eq!(c.blocker_reason.as_deref(), expected_reason);
    assert_eq!(c.next_action.as_deref(), Some("retry_task"));
    Ok(())
}
