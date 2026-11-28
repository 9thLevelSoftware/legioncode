//! Integration regression tests for M3.1 Iteration 8 `explore_step` guards (`13.1`, `13.2`, `13.6`, `13.7`).

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveDeliveryInput, CardDetail, CardPlanningDetail, ClaimOrStartTaskInput,
    ExploreAnswerInput, ExploreStepInput, PauseCardInput, RecordCheckReceiptInput,
    RecordManualAcceptanceInput, RecordReviewRunInput, ResumeCardInput, ReturnForRevisionInput,
    WorkflowBoardRepoError, approve_delivery, claim_or_start_task, explore_step, get_card_detail,
    get_card_execution_detail, pause_card, record_check_receipt, record_manual_acceptance,
    record_review_run, resume_card, return_for_revision,
};
use ship_helpers::complete_default_tasks_and_check;
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const TS: &str = "2026-11-24T10:00:00Z";

fn exp(
    conn: &rusqlite::Connection,
    card_id: &str,
    revision_id: &str,
    expected_revision: i64,
    action: Option<&str>,
    answers: &[ExploreAnswerInput<'_>],
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    let input = ExploreStepInput {
        card_id,
        revision_id,
        expected_revision,
        action,
        continue_round: None,
        answers,
    };
    explore_step(conn, &input, TS)
}

fn assert_invalid_exp(
    conn: &rusqlite::Connection,
    card_id: &str,
    rid: &str,
    rev: i64,
    action: Option<&str>,
    answers: &[ExploreAnswerInput<'_>],
) {
    assert!(matches!(
        exp(conn, card_id, rid, rev, action, answers),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
}

fn assert_card(
    conn: &rusqlite::Connection,
    card_id: &str,
    stage: &str,
    status: &str,
    rev: i64,
) -> Result<CardDetail, Box<dyn std::error::Error>> {
    let d = get_card_detail(conn, card_id)?;
    assert_eq!(
        (&*d.card.stage, &*d.card.status, d.card.revision),
        (stage, status, rev)
    );
    Ok(d)
}

fn ans(
    decision_id: &'static str,
    action: &'static str,
    answer: Option<&'static str>,
) -> ExploreAnswerInput<'static> {
    ExploreAnswerInput {
        decision_id,
        action,
        answer,
        prompt: None,
        recommendation: None,
        rationale: None,
    }
}

fn start_tsk1(
    conn: &rusqlite::Connection,
    card_id: &str,
    rid: &str,
    rev: i64,
) -> Result<i64, WorkflowBoardRepoError> {
    let input = ClaimOrStartTaskInput {
        card_id,
        revision_id: rid,
        expected_revision: rev,
        task_id: "TSK-1",
        stage_run_id: None,
    };
    Ok(claim_or_start_task(conn, &input, TS)?
        .planning
        .card_detail
        .card
        .revision)
}

fn seed_round1(conn: &rusqlite::Connection, card_id: &str) -> TestResult {
    conn.execute(
        "INSERT INTO workflow_board_decisions (card_id, decision_id, round_index, prompt, last_action, status, answer, created_at, updated_at)
         VALUES (?1, 'D1', 1, 'Scope?', 'edit', 'resolved', 'WAL', ?2, ?2)",
        rusqlite::params![card_id, TS],
    )?;
    conn.execute(
        "INSERT INTO workflow_board_exploration_rounds (id, card_id, round_index, status, decision_ids_json, pending_continue_prompt, created_at, updated_at)
         VALUES (?1, ?2, 1, 'awaiting_continue', '[\"D1\"]', 1, ?3, ?3)",
        rusqlite::params![format!("er-{card_id}-r1"), card_id, TS],
    )?;
    Ok(())
}

fn setup_active_ship(
    conn: &rusqlite::Connection,
    card_id: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    let rev = setup_approved_execute_card(conn, card_id)?;
    let rev = complete_default_tasks_and_check(conn, card_id, rev, "sha256:snap-r8", TS)?;
    let reviewed = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id,
            revision_id: &format!("r-{card_id}-rev"),
            review_id: &format!("rr-{card_id}"),
            expected_revision: rev,
            stage_run_id: None,
            thread_id: Some("thr-r8"),
            snapshot_digest: Some("sha256:snap-r8"),
            verdict: "pass",
            summary: Some("Pass"),
            findings: &[],
        },
        TS,
    )?;
    Ok(reviewed.planning.card_detail.card.revision)
}

#[test]
fn r8_13_1_explore_step_pause_on_active_or_blocked_execute_rejects_without_mutation() -> TestResult
{
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    // A: Execute card with no decisions (latest_round = None)
    let rev_a = setup_approved_execute_card(conn, "card-131-no-dec")?;
    let r_a = start_tsk1(conn, "card-131-no-dec", "r-131a-s1", rev_a)?;
    assert_invalid_exp(
        conn,
        "card-131-no-dec",
        "r-131a-bad",
        r_a,
        Some("pause"),
        &[],
    );
    assert_card(conn, "card-131-no-dec", "execute", "active", r_a)?;
    let exec_a = get_card_execution_detail(conn, "card-131-no-dec")?;
    assert_eq!(
        exec_a.planning.card_detail.card.current_task_id.as_deref(),
        Some("TSK-1")
    );
    assert_eq!(exec_a.planning.tasks[0].status, "in_progress");

    // B: Execute card with latest_round = Some(1) and blocked status from failed check
    let rev_b = setup_approved_execute_card(conn, "card-131-round1")?;
    seed_round1(conn, "card-131-round1")?;
    let r_b = start_tsk1(conn, "card-131-round1", "r-131b-s1", rev_b)?;
    assert_invalid_exp(
        conn,
        "card-131-round1",
        "r-131b-bad1",
        r_b,
        Some("pause"),
        &[],
    );
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let blocked = record_check_receipt(
        conn,
        &RecordCheckReceiptInput {
            card_id: "card-131-round1",
            revision_id: "r-131b-chk",
            receipt_id: "rcpt-131b",
            expected_revision: r_b,
            check_id: "chk-1",
            argv: &cmd,
            status: "failed",
            exit_code: Some(1),
            log_ref: None,
            stdout_excerpt: None,
            stderr_excerpt: Some("failed"),
            duration_ms: Some(10),
            snapshot_digest: "sha256:snap-131b",
            commit_sha: None,
        },
        TS,
    )?;
    let r_blk = blocked.planning.card_detail.card.revision;
    assert_eq!(blocked.planning.card_detail.card.status, "blocked");
    assert_invalid_exp(
        conn,
        "card-131-round1",
        "r-131b-bad2",
        r_blk,
        Some("pause"),
        &[],
    );
    assert_card(conn, "card-131-round1", "execute", "blocked", r_blk)?;
    let exec_b = get_card_execution_detail(conn, "card-131-round1")?;
    assert_eq!(exec_b.planning.tasks[0].status, "in_progress");
    Ok(())
}

#[test]
fn r8_13_2_explore_step_resume_cannot_unpause_execute_or_bypass_write_journal() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let rev = setup_approved_execute_card(conn, "card-132-exec")?;
    seed_round1(conn, "card-132-exec")?;
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-132-exec",
            revision_id: "r-132-p",
            expected_revision: rev,
            reason: Some("Operator paused"),
        },
        TS,
    )?;
    let p_rev = paused.planning.card_detail.card.revision;
    conn.execute(
        "INSERT INTO workflow_board_write_journal (id, card_id, target_path, expected_digest, status, created_at, updated_at)
         VALUES ('wj-132', 'card-132-exec', 'card.json', 'sha256:wj', 'staged', ?1, ?1)",
        rusqlite::params![TS],
    )?;

    let resume_in = ResumeCardInput {
        card_id: "card-132-exec",
        revision_id: "r-132-rc",
        expected_revision: p_rev,
        note: None,
    };
    assert!(matches!(
        resume_card(conn, &resume_in, TS),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    assert_invalid_exp(
        conn,
        "card-132-exec",
        "r-132-bypass",
        p_rev,
        Some("resume"),
        &[],
    );
    let after = assert_card(conn, "card-132-exec", "execute", "paused", p_rev)?;
    assert_eq!(after.card.pause_reason.as_deref(), Some("Operator paused"));
    assert_ne!(after.card.next_action.as_deref(), Some("explore_spec"));
    Ok(())
}

#[test]
fn r8_13_6_explore_step_non_regressing_rejects_plan_and_active_ship() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let exec_rev = setup_approved_execute_card(conn, "card-136-plan")?;
    let returned = return_for_revision(
        conn,
        &ReturnForRevisionInput {
            card_id: "card-136-plan",
            revision_id: "r-136p-ret",
            expected_revision: exec_rev,
            target_stage: "plan",
            reason: "Adjust plan",
        },
        TS,
    )?;
    let plan_rev = returned.card_detail.card.revision;
    let ship_rev = setup_active_ship(conn, "card-136-ship")?;
    conn.execute(
        "UPDATE workflow_board_cards SET stage = 'ship', status = 'active' WHERE card_id = 'card-136-ship'",
        [],
    )?;
    for (cid, stage, rev) in [
        ("card-136-plan", "plan", plan_rev),
        ("card-136-ship", "ship", ship_rev),
    ] {
        for (i, act) in [Some("pause"), Some("resume"), Some("step"), None]
            .into_iter()
            .enumerate()
        {
            assert_invalid_exp(conn, cid, &format!("r-136-{stage}-{i}"), rev, act, &[]);
        }
        assert_card(conn, cid, stage, "active", rev)?;
    }
    Ok(())
}

#[test]
fn r8_13_7_explore_step_pause_only_answers_cannot_unship_or_bypass_non_spec_guard() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();

    let ship_rev = setup_active_ship(conn, "card-137-ship")?;
    let accepted = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id: "card-137-ship",
            revision_id: "r-137-ac",
            expected_revision: ship_rev,
            criterion_id: "AC-02",
            verdict: "pass",
            note: Some("Ok"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-r8"),
            observed_by: "client",
        },
        TS,
    )?;
    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id: "card-137-ship",
            revision_id: "r-137-del",
            approval_id: "apr-137-del",
            delivery_receipt_id: "del-137",
            expected_revision: accepted.planning.card_detail.card.revision,
            expected_product_snapshot_digest: Some("sha256:snap-r8"),
            commit_sha: None,
            diff_digest: None,
            note: Some("Shipped"),
            approved_by: "client",
        },
        TS,
    )?;
    let s_rev = shipped.planning.card_detail.card.revision;
    for (i, act, d_act) in [
        (0, Some("pause"), "pause"),
        (1, None, "pause"),
        (2, Some("resume"), "resume"),
    ] {
        assert_invalid_exp(
            conn,
            "card-137-ship",
            &format!("r-137-bad-{i}"),
            s_rev,
            act,
            &[ans("D1", d_act, None)],
        );
    }
    let after = assert_card(conn, "card-137-ship", "ship", "shipped", s_rev)?;
    assert!(after.card.completed_at.is_some());

    // Completed card and active execute card both reject pause-only answer
    let pause_ans = [ans("D1", "pause", None)];
    let comp_rev = setup_approved_execute_card(conn, "card-137-comp")?;
    conn.execute(
        "UPDATE workflow_board_cards SET status = 'completed', completed_at = ?2 WHERE card_id = ?1",
        rusqlite::params!["card-137-comp", TS],
    )?;
    assert_invalid_exp(
        conn,
        "card-137-comp",
        "r-137c-bad",
        comp_rev,
        Some("pause"),
        &pause_ans,
    );

    // Regressing edit alongside pause answer on execute card (12.3) still regresses cleanly
    let exec_rev = setup_approved_execute_card(conn, "card-137-exec")?;
    assert_invalid_exp(
        conn,
        "card-137-exec",
        "r-137e-bad",
        exec_rev,
        Some("pause"),
        &pause_ans,
    );
    let regressed = exp(
        conn,
        "card-137-exec",
        "r-137e-ok",
        exec_rev,
        Some("pause"),
        &[ans("D1", "edit", Some("Updated")), ans("D2", "pause", None)],
    )?;
    assert_eq!(regressed.card_detail.card.stage, "spec");
    assert_eq!(regressed.card_detail.card.status, "paused");
    Ok(())
}
