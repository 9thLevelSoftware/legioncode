//! Integration regression tests for Challenger 2 Iteration 4 probe 10.7:
//! `approve_spec` and `approve_plan` reject `'paused'`, `'completed'`, `'shipped'` cards,
//! and `approve_spec` rejects non-`'spec'` stages with `InvalidStateTransition`.

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveDeliveryInput, ApprovePlanInput, ApproveSpecInput, CreateCardInput, PauseCardInput,
    RecordManualAcceptanceInput, RecordReviewRunInput, ReturnForRevisionInput,
    WorkflowBoardRepoError, approve_delivery, approve_plan, approve_spec, create_card,
    get_card_detail, pause_card, record_manual_acceptance, record_review_run, return_for_revision,
};
use ship_helpers::complete_default_tasks_and_check;
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn seed_spec_card(conn: &rusqlite::Connection, card_id: &str, t: &str) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            revision_id: &format!("r-c-{card_id}"),
            project_id: "proj-1",
            title: "Guard Card",
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
            spec_digest: Some("sha256:spec-guard"),
            wireframes_digest: None,
        },
        t,
    )?;
    Ok(())
}

#[test]
fn approval_gates_reject_paused_completed_shipped_and_wrong_stage_probe_107() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T07:00:00Z";

    // 1. approve_spec rejects paused and completed in spec stage
    seed_spec_card(conn, "card-107-spec-pause", t)?;
    let sp = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-107-spec-pause",
            revision_id: "r-107-sp",
            expected_revision: 1,
            reason: Some("Paused in spec"),
        },
        t,
    )?;
    assert!(matches!(
        approve_spec(
            conn,
            &ApproveSpecInput {
                card_id: "card-107-spec-pause",
                revision_id: "r-107-sp-fail",
                approval_id: "apr-107-sp",
                expected_revision: sp.planning.card_detail.card.revision,
                expected_spec_digest: None,
                wireframes_digest: None,
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    let spec_after = get_card_detail(conn, "card-107-spec-pause")?;
    assert_eq!(spec_after.card.stage, "spec");
    assert_eq!(spec_after.card.status, "paused");
    assert_eq!(spec_after.card.next_action.as_deref(), Some("resume_card"));

    seed_spec_card(conn, "card-107-spec-comp", t)?;
    conn.execute(
        "UPDATE workflow_board_cards SET status = 'completed' WHERE card_id = 'card-107-spec-comp'",
        [],
    )?;
    assert!(matches!(
        approve_spec(
            conn,
            &ApproveSpecInput {
                card_id: "card-107-spec-comp",
                revision_id: "r-107-sc-fail",
                approval_id: "apr-107-sc",
                expected_revision: 1,
                expected_spec_digest: None,
                wireframes_digest: None,
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));

    verify_plan_stage_approval_guards(conn, t)?;
    verify_approve_spec_rejects_shipped_card(conn, t)
}

fn verify_plan_stage_approval_guards(conn: &rusqlite::Connection, t: &str) -> TestResult {
    // 2. approve_plan rejects paused, completed, and shipped in plan stage; approve_spec rejects plan stage
    let rev_exec = setup_approved_execute_card(conn, "card-107-plan")?;
    let ret = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-107-plan",
            revision_id: "r-107-ret",
            expected_revision: rev_exec,
            target_stage: "plan",
            reason: "Check plan guards",
        },
        t,
    )?;
    assert!(matches!(
        approve_spec(
            conn,
            &ApproveSpecInput {
                card_id: "card-107-plan",
                revision_id: "r-107-wrong-stage",
                approval_id: "apr-107-wrong",
                expected_revision: ret.card_detail.card.revision,
                expected_spec_digest: None,
                wireframes_digest: None,
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    let pp = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-107-plan",
            revision_id: "r-107-pp",
            expected_revision: ret.card_detail.card.revision,
            reason: Some("Paused in plan"),
        },
        t,
    )?;
    assert!(matches!(
        approve_plan(
            conn,
            &ApprovePlanInput {
                card_id: "card-107-plan",
                revision_id: "r-107-pp-fail",
                approval_id: "apr-107-pp",
                expected_revision: pp.planning.card_detail.card.revision,
                expected_spec_digest: None,
                expected_plan_digest: None,
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    let plan_after = get_card_detail(conn, "card-107-plan")?;
    assert_eq!(plan_after.card.stage, "plan");
    assert_eq!(plan_after.card.status, "paused");
    assert_eq!(plan_after.card.next_action.as_deref(), Some("resume_card"));

    for terminal_status in ["completed", "shipped"] {
        conn.execute(
            "UPDATE workflow_board_cards SET status = ?1 WHERE card_id = 'card-107-plan'",
            [terminal_status],
        )?;
        assert!(matches!(
            approve_plan(
                conn,
                &ApprovePlanInput {
                    card_id: "card-107-plan",
                    revision_id: &format!("r-107-p-{terminal_status}"),
                    approval_id: &format!("apr-107-p-{terminal_status}"),
                    expected_revision: plan_after.card.revision,
                    expected_spec_digest: None,
                    expected_plan_digest: None,
                    approved_by: "client",
                },
                t,
            ),
            Err(WorkflowBoardRepoError::InvalidStateTransition(_))
        ));
    }
    Ok(())
}

fn verify_approve_spec_rejects_shipped_card(conn: &rusqlite::Connection, t: &str) -> TestResult {
    let rev_s = setup_approved_execute_card(conn, "card-107-shipped")?;
    let rev_s =
        complete_default_tasks_and_check(conn, "card-107-shipped", rev_s, "sha256:snap-107", t)?;
    let reviewed = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-107-shipped",
            revision_id: "r-107-rev",
            review_id: "rr-107",
            expected_revision: rev_s,
            stage_run_id: None,
            thread_id: Some("thr-107"),
            snapshot_digest: Some("sha256:snap-107"),
            verdict: "pass",
            summary: Some("Clean"),
            findings: &[],
        },
        t,
    )?;
    let accepted = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id: "card-107-shipped",
            revision_id: "r-107-ac",
            expected_revision: reviewed.planning.card_detail.card.revision,
            criterion_id: "AC-02",
            verdict: "pass",
            note: Some("Verified"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-107"),
            observed_by: "client",
        },
        t,
    )?;
    verify_approve_delivery_guards(conn, accepted.planning.card_detail.card.revision, t)
}

fn verify_approve_delivery_guards(
    conn: &rusqlite::Connection,
    accepted_rev: i64,
    t: &str,
) -> TestResult {
    // Probe 11.6: approve_delivery rejects completed card
    conn.execute(
        "UPDATE workflow_board_cards SET status = 'completed' WHERE card_id = 'card-107-shipped'",
        [],
    )?;
    assert!(matches!(
        approve_delivery(
            conn,
            &ApproveDeliveryInput {
                card_id: "card-107-shipped",
                revision_id: "r-107-comp-fail",
                approval_id: "apr-107-comp-fail",
                delivery_receipt_id: "del-107-comp-fail",
                expected_revision: accepted_rev,
                expected_product_snapshot_digest: Some("sha256:snap-107"),
                commit_sha: None,
                diff_digest: None,
                note: Some("Should fail on completed"),
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    conn.execute(
        "UPDATE workflow_board_cards SET status = 'active' WHERE card_id = 'card-107-shipped'",
        [],
    )?;
    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id: "card-107-shipped",
            revision_id: "r-107-ship",
            approval_id: "apr-107-del",
            delivery_receipt_id: "del-107",
            expected_revision: accepted_rev,
            expected_product_snapshot_digest: Some("sha256:snap-107"),
            commit_sha: None,
            diff_digest: None,
            note: Some("Shipped"),
            approved_by: "client",
        },
        t,
    )?;
    // Probe 11.8: approve_delivery rejects already-shipped card
    assert!(matches!(
        approve_delivery(
            conn,
            &ApproveDeliveryInput {
                card_id: "card-107-shipped",
                revision_id: "r-107-reship-fail",
                approval_id: "apr-107-reship-fail",
                delivery_receipt_id: "del-107-reship-fail",
                expected_revision: shipped.planning.card_detail.card.revision,
                expected_product_snapshot_digest: Some("sha256:snap-107"),
                commit_sha: None,
                diff_digest: None,
                note: Some("Should fail on shipped"),
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    assert!(matches!(
        approve_spec(
            conn,
            &ApproveSpecInput {
                card_id: "card-107-shipped",
                revision_id: "r-107-ship-fail",
                approval_id: "apr-107-ship-fail",
                expected_revision: shipped.planning.card_detail.card.revision,
                expected_spec_digest: None,
                wireframes_digest: None,
                approved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    Ok(())
}
