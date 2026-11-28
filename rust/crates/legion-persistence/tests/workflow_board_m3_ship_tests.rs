//! Integration tests for M3 snapshot-bound checks, fresh-thread review, lifecycle controls
//! (`pause`, `resume`, `retry`, `amendment`), manual acceptance, and `approve_delivery`.

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveDeliveryInput, LinkStageRunThreadInput, PauseCardInput, ProposeAmendmentInput,
    ReconcileInterruptedActionInput, RecordCheckReceiptInput, RecordExternalActionInput,
    RecordManualAcceptanceInput, RecordReviewRunInput, ResolveFindingInput, ResumeCardInput,
    RetryStepInput, ReviewFindingInput, UpsertStageRunInput, WorkflowBoardRepoError,
    approve_delivery, compute_ship_blockers, get_card_execution_detail, link_stage_run_thread,
    pause_card, propose_amendment, reconcile_interrupted_action, record_check_receipt,
    record_external_action, record_manual_acceptance, record_review_run, resolve_finding,
    resume_card, retry_step, upsert_or_get_stage_run_by_launch_key,
};
use ship_helpers::complete_default_tasks_and_check;
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn checks_fresh_review_manual_acceptance_ship_and_pause_resume_retry_amendment() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-ship")?;
    let t = "2026-11-24T02:00:00Z";
    let rev = complete_default_tasks_and_check(conn, "card-ship", rev, "sha256:snap-v1", t)?;
    let rev = run_fresh_review_and_acceptance(conn, rev, t)?;
    verify_pause_ship_and_amendment(conn, rev, t)
}

fn run_fresh_review_and_acceptance(
    conn: &rusqlite::Connection,
    cur_rev: i64,
    t: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let _ = upsert_or_get_stage_run_by_launch_key(
        conn,
        &UpsertStageRunInput {
            id: "sr-plan-1",
            card_id: "card-ship",
            stage: "plan",
            task_id: None,
            attempt: 1,
            stable_launch_key: "card-ship:plan:1",
            card_revision: 3,
            provider_instance_id: "devReplay",
            model: "dev-replay",
            model_options_json: None,
            runtime_mode: "auto-accept-edits",
            interaction_mode: "plan",
            workspace_strategy_json: "{\"type\":\"root\"}",
            worktree_path: None,
            branch: None,
            brief_digest: None,
        },
        t,
    )?;
    link_stage_run_thread(
        conn,
        &LinkStageRunThreadInput {
            id: "sr-plan-1",
            status: "completed",
            thread_id: Some("thr-plan"),
            workflow_run_id: None,
            worktree_path: None,
            branch: None,
            last_error: None,
            ended_at: Some(t),
        },
        t,
    )?;
    assert!(matches!(
        record_review_run(
            conn,
            &RecordReviewRunInput {
                card_id: "card-ship",
                revision_id: "r-rev-bad",
                review_id: "rr-bad",
                expected_revision: cur_rev,
                stage_run_id: None,
                thread_id: Some("thr-plan"),
                snapshot_digest: Some("sha256:snap-v1"),
                verdict: "pass",
                summary: Some("Reused thread"),
                findings: &[],
            },
            t,
        ),
        Err(WorkflowBoardRepoError::ReuseConflict(_))
    ));
    record_finding_and_manual_acceptance(conn, cur_rev, t)
}

fn record_finding_and_manual_acceptance(
    conn: &rusqlite::Connection,
    cur_rev: i64,
    t: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let rev_with_finding = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-ship",
            revision_id: "r-rev-1",
            review_id: "rr-1",
            expected_revision: cur_rev,
            stage_run_id: None,
            thread_id: Some("thr-fresh-review"),
            snapshot_digest: Some("sha256:snap-v1"),
            verdict: "findings",
            summary: Some("One minor issue"),
            findings: &[ReviewFindingInput {
                id: "f-1",
                severity: "minor",
                criterion_id: Some("AC-02"),
                file_path: Some("src/a.rs"),
                line: Some(12),
                message: "Add doc comment",
            }],
        },
        t,
    )?;
    assert_eq!(rev_with_finding.planning.card_detail.card.status, "blocked");
    let resolved_f = resolve_finding(
        conn,
        &ResolveFindingInput {
            card_id: "card-ship",
            revision_id: "r-f-1",
            expected_revision: rev_with_finding.planning.card_detail.card.revision,
            review_id: None,
            finding_id: "f-1",
            disposition: "accept_risk",
            reason: Some("Covered in follow-up"),
            resolved_by: "client",
        },
        t,
    )?;
    assert_eq!(resolved_f.planning.card_detail.card.status, "active");

    let manual = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id: "card-ship",
            revision_id: "r-ac-2",
            expected_revision: resolved_f.planning.card_detail.card.revision,
            criterion_id: "AC-02",
            verdict: "not-applicable",
            note: Some("CLI-only change"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-v1"),
            observed_by: "client",
        },
        t,
    )?;
    assert_eq!(manual.planning.ac_evidence[1].status, "not_applicable");
    assert_eq!(compute_ship_blockers(conn, "card-ship")?.len(), 0);
    Ok(manual.planning.card_detail.card.revision)
}

fn verify_pause_ship_and_amendment(
    conn: &rusqlite::Connection,
    manual_rev: i64,
    t: &str,
) -> TestResult {
    let _act = record_external_action(
        conn,
        &RecordExternalActionInput {
            id: "act-1",
            card_id: "card-ship",
            action_kind: "git_push",
            target_id: None,
            stage_run_id: None,
            idempotency_key: "push-1",
            status: "in_progress",
            expected_snapshot_digest: Some("sha256:snap-v1"),
            observed_snapshot_digest: None,
            details_json: "{}",
        },
        t,
    )?;
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-ship",
            revision_id: "r-pause",
            expected_revision: manual_rev,
            reason: Some("Operator pause"),
        },
        t,
    )?;
    assert_eq!(paused.planning.card_detail.card.status, "paused");
    assert!(matches!(
        resume_card(
            conn,
            &ResumeCardInput {
                card_id: "card-ship",
                revision_id: "r-res-fail",
                expected_revision: paused.planning.card_detail.card.revision,
                note: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    let _rec = reconcile_interrupted_action(
        conn,
        &ReconcileInterruptedActionInput {
            card_id: "card-ship",
            action_id: "act-1",
            status: "reconciled",
            observed_snapshot_digest: Some("sha256:snap-v1"),
            note: Some("Verified clean"),
            reconciled_by: "client",
        },
        t,
    )?;
    let resumed = resume_card(
        conn,
        &ResumeCardInput {
            card_id: "card-ship",
            revision_id: "r-res-ok",
            expected_revision: paused.planning.card_detail.card.revision,
            note: Some("Resuming"),
        },
        t,
    )?;
    verify_ship_and_amendment(conn, resumed.planning.card_detail.card.revision, t)
}

fn verify_ship_and_amendment(conn: &rusqlite::Connection, resumed_rev: i64, t: &str) -> TestResult {
    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id: "card-ship",
            revision_id: "r-ship",
            approval_id: "apr-del-1",
            delivery_receipt_id: "del-1",
            expected_revision: resumed_rev,
            expected_product_snapshot_digest: Some("sha256:snap-v1"),
            commit_sha: Some("abc123"),
            diff_digest: Some("sha256:diff-final"),
            note: Some("Ship it"),
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(shipped.planning.card_detail.card.status, "shipped");
    assert_eq!(shipped.delivery_receipts.len(), 1);
    // Probe 8.15: record_manual_acceptance rejects already-shipped card
    assert!(matches!(
        record_manual_acceptance(
            conn,
            &RecordManualAcceptanceInput {
                card_id: "card-ship",
                revision_id: "r-ac-shipped-fail",
                expected_revision: shipped.planning.card_detail.card.revision,
                criterion_id: "AC-02",
                verdict: "fail",
                note: Some("Should be rejected on shipped card"),
                evidence_ref: None,
                evidence_digest: None,
                expected_product_snapshot_digest: None,
                observed_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));

    let amended = propose_amendment(
        conn,
        &ProposeAmendmentInput {
            card_id: "card-ship",
            revision_id: "r-amend",
            amendment_id: "am-1",
            expected_revision: shipped.planning.card_detail.card.revision,
            target_stage: "plan",
            reason: "Add follow-up check",
            summary: Some("Re-plan"),
            source_task_id: None,
            unexpected_files: &[],
            proposed_changes_json: "{}",
            proposed_by: "client",
        },
        t,
    )?;
    assert_eq!(amended.planning.card_detail.card.stage, "plan");
    assert_eq!(amended.planning.card_detail.card.status, "active");
    assert_eq!(amended.delivery_receipts[0].status, "stale");
    assert_eq!(amended.check_receipts[0].status, "stale");
    let retried = retry_step(
        conn,
        &RetryStepInput {
            card_id: "card-ship",
            revision_id: "r-retry",
            expected_revision: amended.planning.card_detail.card.revision,
            step_kind: "check",
            target_id: Some("chk-1"),
            reason: Some("Re-run check"),
        },
        t,
    )?;
    let refreshed = get_card_execution_detail(conn, "card-ship")?;
    assert_eq!(
        refreshed.planning.card_detail.card.revision,
        retried.planning.card_detail.card.revision
    );
    Ok(())
}

#[test]
fn snapshot_mismatch_blocks_review_and_ship_and_check_receipt_preserves_paused_and_rejects_shipped()
-> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-snap-guard")?;
    let t = "2026-11-24T03:00:00Z";
    let rev = complete_default_tasks_and_check(conn, "card-snap-guard", rev, "sha256:snap-v2", t)?;

    // Probe 3.3: stale-snapshot review_run rejected when card snapshot is sha256:snap-v2
    for stale_snap in [Some("sha256:snap-v1"), None] {
        assert!(matches!(
            record_review_run(
                conn,
                &RecordReviewRunInput {
                    card_id: "card-snap-guard",
                    revision_id: "r-rev-stale",
                    review_id: "rr-stale",
                    expected_revision: rev,
                    stage_run_id: None,
                    thread_id: Some("thr-rev-fresh"),
                    snapshot_digest: stale_snap,
                    verdict: "pass",
                    summary: Some("Stale snapshot"),
                    findings: &[],
                },
                t,
            ),
            Err(WorkflowBoardRepoError::PreconditionFailed(_))
        ));
    }

    // Probe 7.1: record_check_receipt while paused preserves paused status and pause metadata
    let paused = pause_card(
        conn,
        &PauseCardInput {
            card_id: "card-snap-guard",
            revision_id: "r-sg-pause",
            expected_revision: rev,
            reason: Some("Pause before check"),
        },
        t,
    )?;
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let after_paused_chk = record_check_receipt(
        conn,
        &RecordCheckReceiptInput {
            card_id: "card-snap-guard",
            revision_id: "r-sg-chk-paused",
            receipt_id: "rcpt-sg-paused",
            expected_revision: paused.planning.card_detail.card.revision,
            check_id: "chk-1",
            argv: &cmd,
            status: "failed",
            exit_code: Some(1),
            log_ref: None,
            stdout_excerpt: None,
            stderr_excerpt: Some("fail while paused"),
            duration_ms: Some(15),
            snapshot_digest: "sha256:snap-v2",
            commit_sha: None,
        },
        t,
    )?;
    let paused_card = &after_paused_chk.planning.card_detail.card;
    assert_eq!(paused_card.status, "paused");
    assert_eq!(paused_card.paused_from_status.as_deref(), Some("blocked"));
    assert_eq!(
        paused_card.pause_reason.as_deref(),
        Some("Pause before check")
    );
    assert_eq!(paused_card.paused_at.as_deref(), Some(t));
    Ok(())
}
