//! Integration regression tests for M3.1 Iteration 6 `10.6` remediation:
//! `next_action` (`reconcile_external_action`) on blocked re-entry via `approve_plan`,
//! and clearing stale `'blocked'` / `blocker_reason` after `reconcile_interrupted_action`
//! before `approve_plan` or `approve_spec`.

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApprovePlanInput, ApproveSpecInput, ReconcileInterruptedActionInput, RecordExternalActionInput,
    ReturnForRevisionInput, approve_plan, approve_spec, reconcile_interrupted_action,
    record_external_action, return_for_revision,
};
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn reconciled_action_and_blocked_reentry_next_action_probe_106() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T08:30:00Z";

    let rev = setup_approved_execute_card(conn, "card-106-act")?;
    record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-106",
            card_id: "card-106-act",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "k-106",
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;

    // 1. Interrupted action in 'plan' -> approve_plan sets status='blocked' & next_action='reconcile_external_action'
    let ret_p = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-106-act",
            revision_id: "r-106a-rp",
            expected_revision: rev,
            target_stage: "plan",
            reason: "Re-plan with interrupted action",
        },
        t,
    )?;
    assert_eq!(ret_p.card_detail.card.status, "blocked");
    let app_blocked = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-106-act",
            revision_id: "r-106a-ap1",
            approval_id: "apr-106a-1",
            expected_revision: ret_p.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_blocked.card_detail.card.stage, "execute");
    assert_eq!(app_blocked.card_detail.card.status, "blocked");
    assert_eq!(
        app_blocked.card_detail.card.next_action.as_deref(),
        Some("reconcile_external_action")
    );

    // 2. Reconcile in 'plan' -> approve_plan clears stale 'blocked' & sets next_action='start_execute'
    let ret_p2 = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-106-act",
            revision_id: "r-106a-rp2",
            expected_revision: app_blocked.card_detail.card.revision,
            target_stage: "plan",
            reason: "Reconcile in plan",
        },
        t,
    )?;
    reconcile_interrupted_action(
        conn,
        &ReconcileInterruptedActionInput {
            card_id: "card-106-act",
            action_id: "act-106",
            status: "reconciled",
            observed_snapshot_digest: None,
            note: Some("Cleaned in plan"),
            reconciled_by: "client",
        },
        t,
    )?;
    let app_clear = approve_plan(
        conn,
        &ApprovePlanInput {
            card_id: "card-106-act",
            revision_id: "r-106a-ap2",
            approval_id: "apr-106a-2",
            expected_revision: ret_p2.card_detail.card.revision,
            expected_spec_digest: None,
            expected_plan_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(app_clear.card_detail.card.stage, "execute");
    assert_eq!(app_clear.card_detail.card.status, "active");
    assert_eq!(app_clear.card_detail.card.blocker_reason, None);
    assert_eq!(
        app_clear.card_detail.card.next_action.as_deref(),
        Some("start_execute")
    );

    verify_reconciled_action_clears_blocked_in_approve_spec(conn, t)
}

fn verify_reconciled_action_clears_blocked_in_approve_spec(
    conn: &rusqlite::Connection,
    t: &str,
) -> TestResult {
    // 3a. Unreconciled interrupted action in 'spec' -> approve_spec preserves status='blocked'
    let rev_b = setup_approved_execute_card(conn, "card-106-spec-keep")?;
    record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-106sk",
            card_id: "card-106-spec-keep",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "k-106sk",
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    let ret_keep = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-106-spec-keep",
            revision_id: "r-106sk-rs",
            expected_revision: rev_b,
            target_stage: "spec",
            reason: "Keep blocked in spec",
        },
        t,
    )?;
    let spec_keep = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-106-spec-keep",
            revision_id: "r-106sk-as",
            approval_id: "apr-106sk",
            expected_revision: ret_keep.card_detail.card.revision,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(spec_keep.card.stage, "plan");
    assert_eq!(spec_keep.card.status, "blocked");
    assert_eq!(
        spec_keep.card.blocker_reason.as_deref(),
        Some("Interrupted external action requires reconciliation")
    );
    verify_reconciled_spec_clears_blocked(conn, t)
}

fn verify_reconciled_spec_clears_blocked(conn: &rusqlite::Connection, t: &str) -> TestResult {
    // 3b. Reconcile in 'spec' -> approve_spec clears stale 'blocked' & blocker_reason
    let rev_s = setup_approved_execute_card(conn, "card-106-spec-act")?;
    record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-106s",
            card_id: "card-106-spec-act",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "k-106s",
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    let returned_spec = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-106-spec-act",
            revision_id: "r-106s-rs",
            expected_revision: rev_s,
            target_stage: "spec",
            reason: "Reconcile in spec",
        },
        t,
    )?;
    assert_eq!(returned_spec.card_detail.card.status, "blocked");
    reconcile_interrupted_action(
        conn,
        &ReconcileInterruptedActionInput {
            card_id: "card-106-spec-act",
            action_id: "act-106s",
            status: "reconciled",
            observed_snapshot_digest: None,
            note: Some("Cleaned in spec"),
            reconciled_by: "client",
        },
        t,
    )?;
    let spec_clear = approve_spec(
        conn,
        &ApproveSpecInput {
            card_id: "card-106-spec-act",
            revision_id: "r-106s-as",
            approval_id: "apr-106s",
            expected_revision: returned_spec.card_detail.card.revision,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(spec_clear.card.stage, "plan");
    assert_eq!(spec_clear.card.status, "active");
    assert_eq!(spec_clear.card.blocker_reason, None);
    assert_eq!(spec_clear.card.next_action.as_deref(), Some("start_plan"));
    Ok(())
}
