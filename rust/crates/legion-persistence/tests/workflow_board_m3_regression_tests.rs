//! Integration regression tests for Challenger 2 probe 9.7:
//! direct `update_plan` and `update_spec` regressions from blocked cards.

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    AlternativeSelectionUpdate, ApprovePlanInput, ApproveSpecInput, ClaimOrStartTaskInput,
    RecordCheckReceiptInput, RecordExternalActionInput, UpdatePlanInput, UpdateSpecInput,
    WorkflowBoardRepoError, approve_plan, approve_spec, claim_or_start_task, record_check_receipt,
    record_external_action, update_plan, update_spec,
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

fn record_failed_check(
    conn: &rusqlite::Connection,
    card_id: &str,
    rev_id: &str,
    rcpt_id: &str,
    expected_revision: i64,
    t: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let res = record_check_receipt(
        conn,
        &RecordCheckReceiptInput {
            card_id,
            revision_id: rev_id,
            receipt_id: rcpt_id,
            expected_revision,
            check_id: "chk-1",
            argv: &cmd,
            status: "failed",
            exit_code: Some(1),
            log_ref: None,
            stdout_excerpt: None,
            stderr_excerpt: Some("failed check"),
            duration_ms: Some(10),
            snapshot_digest: "sha256:snap-fail",
            commit_sha: None,
        },
        t,
    )?;
    Ok(res.planning.card_detail.card.revision)
}

#[test]
fn direct_update_plan_and_spec_clear_ghost_blocker_and_keep_interrupted_action_probe_97()
-> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T04:00:00Z";

    // 1. update_plan -> approve_plan clears failed-check blocker
    let rev_a = setup_approved_execute_card(conn, "card-97-plan")?;
    let rev_a = record_failed_check(conn, "card-97-plan", "r-f1", "rcpt-f1", rev_a, t)?;
    let upd_plan = update_plan(
        conn,
        &partial_plan_update("card-97-plan", "r-up-1", rev_a, "sha256:plan-97-v2"),
        t,
    )?;
    assert_eq!(upd_plan.card_detail.card.stage, "plan");
    assert_eq!(upd_plan.card_detail.card.status, "active");
    assert_eq!(upd_plan.card_detail.card.blocker_reason, None);
    assert_eq!(
        upd_plan.card_detail.card.next_action.as_deref(),
        Some("revise_plan")
    );
    let app_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-97-plan",
            revision_id: "r-ap-1",
            approval_id: "apr-97-1",
            expected_revision: upd_plan.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_plan.card_detail.card.stage, "execute");
    assert_eq!(app_plan.card_detail.card.status, "active");
    assert_eq!(app_plan.card_detail.card.blocker_reason, None);

    // 2. update_spec -> approve_spec -> approve_plan clears failed-check blocker
    let rev_b = setup_approved_execute_card(conn, "card-97-spec")?;
    let rev_b = record_failed_check(conn, "card-97-spec", "r-f2", "rcpt-f2", rev_b, t)?;
    let upd_spec = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-97-spec",
            revision_id: "r-us-1",
            expected_revision: rev_b,
            title: None,
            brief: Some("Revised brief after check failure"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-97-v2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        t,
    )?;
    assert_eq!(upd_spec.card.stage, "spec");
    assert_eq!(upd_spec.card.status, "active");
    assert_eq!(upd_spec.card.blocker_reason, None);
    assert_eq!(upd_spec.card.next_action.as_deref(), Some("revise_spec"));
    let app_spec = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-97-spec",
            revision_id: "r-as-1",
            approval_id: "apr-97-spec",
            expected_revision: upd_spec.card.revision,
            expected_spec_digest: Some("sha256:spec-97-v2"),
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_spec.card.status, "active");
    assert_eq!(app_spec.card.blocker_reason, None);

    verify_interrupted_action_stays_blocked_across_direct_updates(conn, t)
}

fn verify_interrupted_action_stays_blocked_across_direct_updates(
    conn: &rusqlite::Connection,
    t: &str,
) -> TestResult {
    let rev = setup_approved_execute_card(conn, "card-97-int")?;
    let _ = record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-97-int",
            card_id: "card-97-int",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "push-97-int",
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    let rev = record_failed_check(conn, "card-97-int", "r-97i-f", "rcpt-97i-f", rev, t)?;
    let upd_plan = update_plan(
        conn,
        &partial_plan_update("card-97-int", "r-97i-up", rev, "sha256:plan-97i-v2"),
        t,
    )?;
    assert_eq!(upd_plan.card_detail.card.status, "blocked");
    assert_eq!(
        upd_plan.card_detail.card.blocker_reason.as_deref(),
        Some("Interrupted external action requires reconciliation")
    );
    assert_eq!(
        upd_plan.card_detail.card.next_action.as_deref(),
        Some("revise_plan")
    );
    let app_plan = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-97-int",
            revision_id: "r-97i-ap",
            approval_id: "apr-97i-plan",
            expected_revision: upd_plan.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_plan.card_detail.card.status, "blocked");
    assert!(matches!(
        claim_or_start_task(
            conn,
            &ClaimOrStartTaskInput {
                card_id: "card-97-int",
                revision_id: "r-97i-claim",
                expected_revision: app_plan.card_detail.card.revision,
                task_id: "TSK-1",
                stage_run_id: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    let upd_spec = update_spec(
        conn,
        &UpdateSpecInput {
            card_id: "card-97-int",
            revision_id: "r-97i-us",
            expected_revision: app_plan.card_detail.card.revision,
            title: None,
            brief: Some("Spec update while interrupted action remains"),
            intake_goal: None,
            input_paths_json: None,
            spec_json: None,
            spec_digest: Some("sha256:spec-97i-v2"),
            wireframes_digest: None,
            clear_wireframes_digest: false,
        },
        t,
    )?;
    assert_eq!(upd_spec.card.status, "blocked");
    assert_eq!(
        upd_spec.card.blocker_reason.as_deref(),
        Some("Interrupted external action requires reconciliation")
    );
    assert_eq!(upd_spec.card.next_action.as_deref(), Some("revise_spec"));
    Ok(())
}
