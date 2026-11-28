//! Integration regression tests for M3.1 Iteration 6 `explore_step` regression cleanup:
//! regressing from `'execute'` / `'shipped'` clears `current_task_id`, resets `'in_progress'`
//! tasks to `'todo'`, clears terminal/pause/stale-blocker metadata, preserves `'interrupted'`
//! external actions when not paused, and allows subsequent `approve_spec`.

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveDeliveryInput, ApproveSpecInput, CardPlanningDetail, ClaimOrStartTaskInput,
    ExploreAnswerInput, ExploreStepInput, PauseCardInput, RecordExternalActionInput,
    RecordManualAcceptanceInput, RecordReviewRunInput, WorkflowBoardRepoError, approve_delivery,
    approve_spec, claim_or_start_task, explore_step, get_card_execution_detail, pause_card,
    record_external_action, record_manual_acceptance, record_review_run,
};
use ship_helpers::complete_default_tasks_and_check;
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run_explore_edit(
    conn: &rusqlite::Connection,
    card_id: &str,
    revision_id: &str,
    expected_revision: i64,
    decision_id: &str,
    t: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    explore_step(
        conn,
        &ExploreStepInput {
            card_id,
            revision_id,
            expected_revision,
            action: None,
            continue_round: None,
            answers: &[ExploreAnswerInput {
                decision_id,
                action: "edit",
                answer: Some("Updated decision"),
                prompt: None,
                recommendation: None,
                rationale: None,
            }],
        },
        t,
    )
}

#[test]
fn explore_step_regression_clears_tasks_terminal_status_and_metadata() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T08:15:00Z";

    // A. Regress from paused execute card with in_progress task
    let rev = setup_approved_execute_card(conn, "card-exp-exec")?;
    let s1 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-exp-exec",
            revision_id: "r-ee-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-exp-exec",
            revision_id: "r-ee-p",
            expected_revision: s1.planning.card_detail.card.revision,
            reason: Some("Pause before explore"),
        },
        t,
    )?;
    let explored = run_explore_edit(
        conn,
        "card-exp-exec",
        "r-ee-exp",
        paused.planning.card_detail.card.revision,
        "D1",
        t,
    )?;
    let card = &explored.card_detail.card;
    assert_eq!(card.stage, "spec");
    assert_eq!(card.status, "active");
    assert_eq!(card.current_task_id, None);
    assert_eq!(card.paused_from_status, None);
    assert_eq!(card.pause_reason, None);
    assert_eq!(card.paused_at, None);
    assert_eq!(card.blocker_reason, None);
    assert_eq!(card.next_action.as_deref(), Some("explore_spec"));
    let exec_after = get_card_execution_detail(conn, "card-exp-exec")?;
    assert_eq!(exec_after.planning.tasks[0].status, "todo");
    let reapp_spec = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-exp-exec",
            revision_id: "r-ee-as",
            approval_id: "apr-ee-as",
            expected_revision: card.revision,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(reapp_spec.card.stage, "plan");

    verify_explore_regression_from_shipped_and_interrupted(conn, t)
}

fn verify_explore_regression_from_shipped_and_interrupted(
    conn: &rusqlite::Connection,
    t: &str,
) -> TestResult {
    let rev_s = setup_approved_execute_card(conn, "card-exp-ship")?;
    let rev_s =
        complete_default_tasks_and_check(conn, "card-exp-ship", rev_s, "sha256:snap-es", t)?;
    let rev_r = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-exp-ship",
            revision_id: "r-es-rev",
            review_id: "rr-es",
            expected_revision: rev_s,
            stage_run_id: None,
            thread_id: Some("thr-es"),
            snapshot_digest: Some("sha256:snap-es"),
            verdict: "pass",
            summary: Some("Pass"),
            findings: &[],
        },
        t,
    )?;
    let acc = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id: "card-exp-ship",
            revision_id: "r-es-ac",
            expected_revision: rev_r.planning.card_detail.card.revision,
            criterion_id: "AC-02",
            verdict: "pass",
            note: Some("Ok"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-es"),
            observed_by: "client",
        },
        t,
    )?;
    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id: "card-exp-ship",
            revision_id: "r-es-del",
            approval_id: "apr-es-del",
            delivery_receipt_id: "del-es",
            expected_revision: acc.planning.card_detail.card.revision,
            expected_product_snapshot_digest: Some("sha256:snap-es"),
            commit_sha: None,
            diff_digest: None,
            note: Some("Shipped"),
            approved_by: "client",
        },
        t,
    )?;
    let exp_ship = run_explore_edit(
        conn,
        "card-exp-ship",
        "r-es-exp",
        shipped.planning.card_detail.card.revision,
        "D1",
        t,
    )?;
    assert_eq!(exp_ship.card_detail.card.stage, "spec");
    assert_eq!(exp_ship.card_detail.card.status, "active");
    assert_eq!(exp_ship.card_detail.card.completed_at, None);
    let spec_ok = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-exp-ship",
            revision_id: "r-es-as",
            approval_id: "apr-es-as",
            expected_revision: exp_ship.card_detail.card.revision,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(spec_ok.card.stage, "plan");
    verify_explore_regression_preserves_interrupted_action(conn, t)
}

fn verify_explore_regression_preserves_interrupted_action(
    conn: &rusqlite::Connection,
    t: &str,
) -> TestResult {
    let rev_i = setup_approved_execute_card(conn, "card-exp-int")?;
    record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-exp-int",
            card_id: "card-exp-int",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "k-exp-int",
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    let exp_int = run_explore_edit(conn, "card-exp-int", "r-ei-exp", rev_i, "D1", t)?;
    assert_eq!(exp_int.card_detail.card.stage, "spec");
    assert_eq!(exp_int.card_detail.card.status, "blocked");
    assert_eq!(
        exp_int.card_detail.card.blocker_reason.as_deref(),
        Some("Interrupted external action requires reconciliation")
    );
    Ok(())
}
