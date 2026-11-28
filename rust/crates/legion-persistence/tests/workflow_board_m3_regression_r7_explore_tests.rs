//! Integration regression tests for M3.1 Iteration 7 `explore_step` fixes (`12.4a`..`12.4d`).

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveDeliveryInput, ApproveSpecInput, CardDetail, CardPlanningDetail, ClaimOrStartTaskInput,
    CreateCardInput, ExploreAnswerInput, ExploreStepInput, PauseCardInput,
    ReconcileInterruptedActionInput, RecordExternalActionInput, RecordManualAcceptanceInput,
    RecordReviewRunInput, WorkflowBoardRepoError, approve_delivery, approve_spec,
    claim_or_start_task, create_card, explore_step, get_card_detail, get_card_execution_detail,
    invalidate_spec_approval, pause_card, reconcile_interrupted_action, record_external_action,
    record_manual_acceptance, record_review_run,
};
use ship_helpers::complete_default_tasks_and_check;
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn create_spec_card(conn: &rusqlite::Connection, card_id: &str, t: &str) -> TestResult {
    create_card(
        conn,
        &CreateCardInput {
            card_id,
            revision_id: &format!("rev-{card_id}-1"),
            project_id: "proj-1",
            title: "R7 Explore Card",
            brief: "Brief",
            intake_goal: Some("change"),
            brief_path: None,
            brief_digest: None,
            input_paths_json: None,
            source_type: "idea",
            assistance_mode: "guided",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: Some("/repo/.wt/r7"),
            branch: Some("feat/r7"),
            model_selection_json: None,
            runtime_mode: None,
            interaction_mode: None,
            spec_json: "{\"acceptanceCriteria\":[{\"id\":\"AC-01\",\"text\":\"Works\"}]}",
            spec_digest: Some("sha256:spec-r7"),
            wireframes_digest: None,
        },
        t,
    )?;
    Ok(())
}

fn edit_ans(decision_id: &'static str) -> [ExploreAnswerInput<'static>; 1] {
    [ExploreAnswerInput {
        decision_id,
        action: "edit",
        answer: Some("Updated decision"),
        prompt: None,
        recommendation: None,
        rationale: None,
    }]
}

fn exp(
    conn: &rusqlite::Connection,
    card_id: &str,
    revision_id: &str,
    expected_revision: i64,
    action: Option<&str>,
    answers: &[ExploreAnswerInput<'_>],
    t: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    explore_step(
        conn,
        &ExploreStepInput {
            card_id,
            revision_id,
            expected_revision,
            action,
            continue_round: None,
            answers,
        },
        t,
    )
}

fn approve_spec_simple(
    conn: &rusqlite::Connection,
    card_id: &str,
    suffix: &str,
    expected_revision: i64,
    t: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    approve_spec(
        conn,
        &ApproveSpecInput {
            card_id,
            revision_id: &format!("rev-{card_id}-{suffix}"),
            approval_id: &format!("apr-{card_id}-{suffix}"),
            expected_revision,
            expected_spec_digest: None,
            wireframes_digest: None,
            approved_by: "client",
        },
        t,
    )
}

fn seed_interrupted(conn: &rusqlite::Connection, id: &str, card_id: &str, t: &str) -> TestResult {
    record_external_action(
        conn,
        &RecordExternalActionInput {
            id,
            card_id,
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: id,
            status: "interrupted",
            expected_snapshot_digest: None,
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    Ok(())
}

#[test]
fn explore_step_resume_preserves_interrupted_action_blocker_until_reconciled() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T09:00:00Z";

    // Case A: spec-stage pause + resume with unreconciled interrupted action (`12.4a`)
    create_spec_card(conn, "card-124a-spec", t)?;
    seed_interrupted(conn, "act-124a-1", "card-124a-spec", t)?;
    let paused = exp(conn, "card-124a-spec", "r-124a-p", 1, Some("pause"), &[], t)?;
    assert_eq!(paused.card_detail.card.status, "paused");

    let r = paused.card_detail.card.revision;
    let resumed = exp(
        conn,
        "card-124a-spec",
        "r-124a-r",
        r,
        Some("resume"),
        &[],
        t,
    )?;
    assert_eq!(resumed.card_detail.card.status, "blocked");
    assert_eq!(resumed.card_detail.card.paused_from_status, None);
    assert_eq!(resumed.card_detail.card.pause_reason, None);
    assert_eq!(resumed.card_detail.card.paused_at, None);
    assert_eq!(
        resumed.card_detail.card.blocker_reason.as_deref(),
        Some("Interrupted external action requires reconciliation")
    );

    reconcile_interrupted_action(
        conn,
        &ReconcileInterruptedActionInput {
            card_id: "card-124a-spec",
            action_id: "act-124a-1",
            status: "reconciled",
            observed_snapshot_digest: None,
            note: Some("Verified"),
            reconciled_by: "client",
        },
        t,
    )?;
    let rev_res = resumed.card_detail.card.revision;
    let approved = approve_spec_simple(conn, "card-124a-spec", "as", rev_res, t)?;
    assert_eq!(approved.card.stage, "plan");
    assert_eq!(approved.card.status, "active");
    assert_eq!(approved.card.blocker_reason, None);

    // Case B: execute-stage regressing pause + resume with unreconciled interrupted action (`12.4a`)
    let exec_rev = setup_approved_execute_card(conn, "card-124a-exec")?;
    seed_interrupted(conn, "act-124a-2", "card-124a-exec", t)?;
    let ans = edit_ans("D1");
    let reg_p = exp(
        conn,
        "card-124a-exec",
        "r-124a-ep",
        exec_rev,
        Some("pause"),
        &ans,
        t,
    )?;
    assert_eq!(reg_p.card_detail.card.stage, "spec");
    assert_eq!(reg_p.card_detail.card.status, "paused");
    let reg_rev = reg_p.card_detail.card.revision;
    let reg_r = exp(
        conn,
        "card-124a-exec",
        "r-124a-er",
        reg_rev,
        Some("resume"),
        &[],
        t,
    )?;
    assert_eq!(reg_r.card_detail.card.status, "blocked");
    assert_eq!(
        reg_r.card_detail.card.blocker_reason.as_deref(),
        Some("Interrupted external action requires reconciliation")
    );
    Ok(())
}

#[test]
fn explore_step_resume_after_pause_card_clears_pause_metadata() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T09:05:00Z";

    create_spec_card(conn, "card-124b", t)?;
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-124b",
            revision_id: "rev-124b-p",
            expected_revision: 1,
            reason: Some("Operator pause"),
        },
        t,
    )?;
    assert_eq!(paused.planning.card_detail.card.status, "paused");
    assert_eq!(
        paused.planning.card_detail.card.pause_reason.as_deref(),
        Some("Operator pause")
    );

    let p_rev = paused.planning.card_detail.card.revision;
    let resumed = exp(conn, "card-124b", "r-124b-r", p_rev, Some("resume"), &[], t)?;
    let card = &resumed.card_detail.card;
    assert_eq!(card.status, "active");
    assert_eq!(card.paused_from_status, None);
    assert_eq!(card.pause_reason, None);
    assert_eq!(card.paused_at, None);
    assert_eq!(card.blocker_reason, None);
    assert_eq!(card.next_action.as_deref(), Some("explore_spec"));
    Ok(())
}

#[test]
fn explore_step_rejects_non_regressing_calls_on_shipped_and_completed_cards() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T09:10:00Z";

    let rev = setup_approved_execute_card(conn, "card-124c-ship")?;
    let rev = complete_default_tasks_and_check(conn, "card-124c-ship", rev, "sha256:snap-124c", t)?;
    let reviewed = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-124c-ship",
            revision_id: "r-124c-rev",
            review_id: "rr-124c",
            expected_revision: rev,
            stage_run_id: None,
            thread_id: Some("thr-124c"),
            snapshot_digest: Some("sha256:snap-124c"),
            verdict: "pass",
            summary: Some("Pass"),
            findings: &[],
        },
        t,
    )?;
    let accepted = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id: "card-124c-ship",
            revision_id: "r-124c-ac",
            expected_revision: reviewed.planning.card_detail.card.revision,
            criterion_id: "AC-02",
            verdict: "pass",
            note: Some("Ok"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-124c"),
            observed_by: "client",
        },
        t,
    )?;
    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id: "card-124c-ship",
            revision_id: "r-124c-del",
            approval_id: "apr-124c-del",
            delivery_receipt_id: "del-124c",
            expected_revision: accepted.planning.card_detail.card.revision,
            expected_product_snapshot_digest: Some("sha256:snap-124c"),
            commit_sha: None,
            diff_digest: None,
            note: Some("Shipped"),
            approved_by: "client",
        },
        t,
    )?;
    let ship_rev = shipped.planning.card_detail.card.revision;

    for (idx, act) in [Some("pause"), Some("resume"), None]
        .into_iter()
        .enumerate()
    {
        let rid = format!("r-124c-bad-{idx}");
        let err = exp(conn, "card-124c-ship", &rid, ship_rev, act, &[], t);
        assert!(matches!(
            err,
            Err(WorkflowBoardRepoError::InvalidStateTransition(_))
        ));
    }
    let after_ship = get_card_detail(conn, "card-124c-ship")?;
    assert_eq!(after_ship.card.stage, "ship");
    assert_eq!(after_ship.card.status, "shipped");
    assert_eq!(after_ship.card.revision, ship_rev);
    assert!(after_ship.card.completed_at.is_some());

    create_spec_card(conn, "card-124c-comp", t)?;
    conn.execute(
        "UPDATE workflow_board_cards SET status = 'completed', completed_at = ?2 WHERE card_id = ?1",
        rusqlite::params!["card-124c-comp", t],
    )?;
    for (idx, act) in [Some("pause"), Some("resume")].into_iter().enumerate() {
        let rid = format!("r-124c-cb-{idx}");
        let err = exp(conn, "card-124c-comp", &rid, 1, act, &[], t);
        assert!(matches!(
            err,
            Err(WorkflowBoardRepoError::InvalidStateTransition(_))
        ));
    }
    Ok(())
}

#[test]
fn explore_step_regresses_execute_card_when_spec_approval_already_stale() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let t = "2026-11-24T09:15:00Z";

    let rev = setup_approved_execute_card(conn, "card-124d")?;
    let started = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-124d",
            revision_id: "r-124d-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    assert_eq!(
        started.planning.card_detail.card.current_task_id.as_deref(),
        Some("TSK-1")
    );
    assert!(invalidate_spec_approval(
        conn,
        "card-124d",
        "external_spec_edit",
        t
    )?);

    let s_rev = started.planning.card_detail.card.revision;
    let ans = edit_ans("D1");
    let regressed = exp(conn, "card-124d", "r-124d-exp", s_rev, None, &ans, t)?;
    assert_eq!(regressed.card_detail.card.stage, "spec");
    assert_eq!(regressed.card_detail.card.status, "active");
    assert_eq!(regressed.card_detail.card.current_task_id, None);
    let exec = get_card_execution_detail(conn, "card-124d")?;
    assert_eq!(exec.planning.tasks[0].status, "todo");

    let reg_rev = regressed.card_detail.card.revision;
    let reapproved = approve_spec_simple(conn, "card-124d", "as", reg_rev, t)?;
    assert_eq!(reapproved.card.stage, "plan");
    Ok(())
}
