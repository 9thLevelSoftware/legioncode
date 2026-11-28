//! Integration regression tests for Challenger 2 probe 9.8:
//! direct `update_plan` and `update_spec` regressions from shipped and paused cards.

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AcMappingInput, AlternativeSelectionUpdate, ApproveDeliveryInput, ApprovePlanInput,
    ApproveSpecInput, ClaimOrStartTaskInput, PauseCardInput, PlannedCheckInput,
    RecordManualAcceptanceInput, RecordReviewRunInput, RetryStepInput, TaskContractInput,
    UpdatePlanInput, UpdateSpecInput, approve_delivery, approve_plan, approve_spec,
    claim_or_start_task, get_card_execution_detail, pause_card, record_manual_acceptance,
    record_review_run, retry_step, update_plan, update_spec,
};
use ship_helpers::complete_default_tasks_and_check;
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

fn ship_test_card(
    conn: &rusqlite::Connection,
    card_id: &str,
    t: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let rev = setup_approved_execute_card(conn, card_id)?;
    let rev = complete_default_tasks_and_check(conn, card_id, rev, "sha256:snap-ship", t)?;
    let reviewed = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id,
            revision_id: &format!("r-rev-{card_id}"),
            review_id: &format!("rr-{card_id}"),
            expected_revision: rev,
            stage_run_id: None,
            thread_id: Some("thr-ship-review"),
            snapshot_digest: Some("sha256:snap-ship"),
            verdict: "pass",
            summary: Some("Clean"),
            findings: &[],
        },
        t,
    )?;
    let accepted = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id,
            revision_id: &format!("r-ac-{card_id}"),
            expected_revision: reviewed.planning.card_detail.card.revision,
            criterion_id: "AC-02",
            verdict: "pass",
            note: Some("Verified"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-ship"),
            observed_by: "client",
        },
        t,
    )?;
    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id,
            revision_id: &format!("r-del-{card_id}"),
            approval_id: &format!("apr-del-{card_id}"),
            delivery_receipt_id: &format!("del-{card_id}"),
            expected_revision: accepted.planning.card_detail.card.revision,
            expected_product_snapshot_digest: Some("sha256:snap-ship"),
            commit_sha: Some("abc999"),
            diff_digest: Some("sha256:diff-ship"),
            note: Some("Shipped"),
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(shipped.planning.card_detail.card.status, "shipped");
    assert!(shipped.planning.card_detail.card.completed_at.is_some());
    Ok(shipped.planning.card_detail.card.revision)
}

#[test]
fn direct_update_plan_and_spec_clear_shipped_and_paused_state_probe_98() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T05:00:00Z";

    // 1. update_plan -> approve_plan on a shipped card un-wedges it and allows task execution
    let ship_rev = ship_test_card(conn, "card-98-plan", t)?;
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let new_tasks = [TaskContractInput {
        task_id: "TSK-NEW",
        title: "Follow-up task",
        depends_on: &[],
        blocks: &[],
        files_allowed: &["src/c.rs"],
        files_forbidden: &[],
        expected_artifacts: &[],
        verification_commands: &[],
        testing_method: "test-first",
        testing_limitations: None,
        status: None,
    }];
    let new_checks = [PlannedCheckInput {
        check_id: "chk-1",
        owner_task_id: Some("TSK-NEW"),
        argv: &cmd,
        label: Some("Run tests"),
    }];
    let new_acs = [
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
            procedure: Some("Inspect"),
            evidence_ref: None,
            external_source: None,
        },
    ];
    let upd_plan = update_plan(
        conn,
        &UpdatePlanInput {
            tasks: Some(&new_tasks),
            checks: Some(&new_checks),
            ac_mappings: Some(&new_acs),
            ..partial_plan_update("card-98-plan", "r-98p-up", ship_rev, "sha256:plan-98-v2")
        },
        t,
    )?;
    assert_eq!(upd_plan.card_detail.card.stage, "plan");
    assert_eq!(upd_plan.card_detail.card.status, "active");
    assert_eq!(upd_plan.card_detail.card.completed_at, None);
    assert_eq!(
        upd_plan.card_detail.card.next_action.as_deref(),
        Some("revise_plan")
    );
    let exec_after_plan = get_card_execution_detail(conn, "card-98-plan")?;
    assert_eq!(exec_after_plan.delivery_receipts[0].status, "stale");

    let app_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-98-plan",
            revision_id: "r-98p-ap",
            approval_id: "apr-98p-plan",
            expected_revision: upd_plan.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_plan.card_detail.card.stage, "execute");
    assert_eq!(app_plan.card_detail.card.status, "active");
    assert_eq!(app_plan.card_detail.card.completed_at, None);
    let claimed = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-98-plan",
            revision_id: "r-98p-claim",
            expected_revision: app_plan.card_detail.card.revision,
            task_id: "TSK-NEW",
            stage_run_id: None,
        },
        t,
    )?;
    assert_eq!(
        claimed.planning.card_detail.card.current_task_id.as_deref(),
        Some("TSK-NEW")
    );

    verify_shipped_update_spec(conn, t)?;
    verify_paused_direct_updates(conn, t)
}

fn verify_shipped_update_spec(conn: &rusqlite::Connection, t: &str) -> TestResult {
    // 2. update_spec -> approve_spec -> approve_plan on a shipped card un-wedges it
    let ship_rev = ship_test_card(conn, "card-98-spec", t)?;
    let upd_spec = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-98-spec",
            revision_id: "r-98s-us",
            expected_revision: ship_rev,
            title: None,
            brief: Some("Post-ship spec revision"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-98-v2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        t,
    )?;
    assert_eq!(upd_spec.card.stage, "spec");
    assert_eq!(upd_spec.card.status, "active");
    assert_eq!(upd_spec.card.completed_at, None);
    assert_eq!(upd_spec.card.next_action.as_deref(), Some("revise_spec"));
    let exec_after_spec = get_card_execution_detail(conn, "card-98-spec")?;
    assert_eq!(exec_after_spec.delivery_receipts[0].status, "stale");

    let app_spec = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-98-spec",
            revision_id: "r-98s-as",
            approval_id: "apr-98s-spec",
            expected_revision: upd_spec.card.revision,
            expected_spec_digest: Some("sha256:spec-98-v2"),
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    let app_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-98-spec",
            revision_id: "r-98s-ap",
            approval_id: "apr-98s-plan",
            expected_revision: app_spec.card.revision,
            expected_spec_digest: Some("sha256:spec-98-v2"),
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_plan.card_detail.card.stage, "execute");
    assert_eq!(app_plan.card_detail.card.status, "active");
    assert_eq!(app_plan.card_detail.card.completed_at, None);
    let retried = retry_step(
        conn,
        &RetryStepInput {
            card_id: "card-98-spec",
            revision_id: "r-98s-retry",
            expected_revision: app_plan.card_detail.card.revision,
            step_kind: "check",
            target_id: Some("chk-1"),
            reason: Some("Re-verify after re-approval"),
        },
        t,
    )?;
    assert_eq!(retried.planning.card_detail.card.status, "active");
    Ok(())
}

fn verify_paused_direct_updates(conn: &rusqlite::Connection, t: &str) -> TestResult {
    // 3. update_plan and update_spec clear pause metadata on paused cards
    let rev_p1 = setup_approved_execute_card(conn, "card-98-paused-plan")?;
    let p1 = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-98-paused-plan",
            revision_id: "r-98pp-pause",
            expected_revision: rev_p1,
            reason: Some("Pause before update_plan"),
        },
        t,
    )?;
    let u1 = update_plan(
        conn,
        &partial_plan_update(
            "card-98-paused-plan",
            "r-98pp-up",
            p1.planning.card_detail.card.revision,
            "sha256:plan-98pp-v2",
        ),
        t,
    )?;
    assert_eq!(u1.card_detail.card.status, "active");
    assert_eq!(u1.card_detail.card.paused_from_status, None);
    assert_eq!(u1.card_detail.card.pause_reason, None);
    assert_eq!(u1.card_detail.card.paused_at, None);
    assert_eq!(
        u1.card_detail.card.next_action.as_deref(),
        Some("revise_plan")
    );

    let rev_p2 = setup_approved_execute_card(conn, "card-98-paused-spec")?;
    let p2 = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-98-paused-spec",
            revision_id: "r-98ps-pause",
            expected_revision: rev_p2,
            reason: Some("Pause before update_spec"),
        },
        t,
    )?;
    let u2 = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-98-paused-spec",
            revision_id: "r-98ps-us",
            expected_revision: p2.planning.card_detail.card.revision,
            title: None,
            brief: Some("Spec edit while paused"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-98ps-v2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        t,
    )?;
    assert_eq!(u2.card.status, "active");
    assert_eq!(u2.card.paused_from_status, None);
    assert_eq!(u2.card.pause_reason, None);
    assert_eq!(u2.card.paused_at, None);
    assert_eq!(u2.card.next_action.as_deref(), Some("revise_spec"));
    Ok(())
}
