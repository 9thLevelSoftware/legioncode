//! Regression tests for M3 review archival, finding status guards, orthogonal blockers,
//! read-time snapshot validation in Ship blockers, and shipped check-receipt protection.

#[path = "workflow_board_m3_support/ship_helpers.rs"]
mod ship_helpers;
#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ApproveDeliveryInput, ClaimOrStartTaskInput, CompleteTaskInput, RecordCheckReceiptInput,
    RecordManualAcceptanceInput, RecordReviewRunInput, ResolveFindingInput, ReviewFindingInput,
    WorkflowBoardRepoError, approve_delivery, claim_or_start_task, complete_task,
    compute_ship_blockers, record_check_receipt, record_manual_acceptance, record_review_run,
    resolve_finding,
};
use ship_helpers::complete_default_tasks_and_check;
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn review_archival_finding_guards_and_orthogonal_blockers() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-rev-guard")?;
    let t = "2026-11-24T04:00:00Z";

    // Create a pending contract violation on TSK-1, then complete TSK-2
    let s1 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-rev-guard",
            revision_id: "r-rg-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let v1 = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-rev-guard",
            revision_id: "r-rg-v1",
            expected_revision: s1.planning.card_detail.card.revision,
            task_id: "TSK-1",
            outcome_status: "completed",
            stage_run_id: None,
            thread_id: None,
            workflow_run_id: None,
            diff_digest: None,
            product_snapshot_digest: Some("sha256:snap-rg"),
            error_message: None,
            violation_id: Some("viol-rg"),
            changed_files: &["src/a.rs", "src/extra.rs"],
            unexpected_files: &["src/extra.rs"],
            forbidden_files: &[],
        },
        t,
    )?;

    // Probe 7.3: resolve_finding(dismiss) must NOT clear blocked status while violation_pending exists
    let rr1 = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-rev-guard",
            revision_id: "r-rg-rr1",
            review_id: "rr-1",
            expected_revision: v1.planning.card_detail.card.revision,
            stage_run_id: None,
            thread_id: Some("thr-rg-1"),
            snapshot_digest: Some("sha256:snap-rg"),
            verdict: "findings",
            summary: Some("Finding 1"),
            findings: &[ReviewFindingInput {
                id: "f-1",
                severity: "major",
                criterion_id: None,
                file_path: None,
                line: Some(10),
                message: "Fix this",
            }],
        },
        t,
    )?;
    let dismissed = resolve_finding(
        conn,
        &ResolveFindingInput {
            card_id: "card-rev-guard",
            revision_id: "r-rg-res1",
            expected_revision: rr1.planning.card_detail.card.revision,
            review_id: None,
            finding_id: "f-1",
            disposition: "dismiss",
            reason: Some("Not applicable"),
            resolved_by: "client",
        },
        t,
    )?;
    assert_eq!(dismissed.planning.card_detail.card.status, "blocked");
    assert_ne!(
        dismissed.planning.card_detail.card.next_action.as_deref(),
        Some("ready_for_ship")
    );
    verify_fix_archival_and_historical_guard(conn, dismissed.planning.card_detail.card.revision, t)
}

fn verify_fix_archival_and_historical_guard(
    conn: &rusqlite::Connection,
    dismissed_revision: i64,
    t: &str,
) -> TestResult {
    // Probe 4.4 & 7.4: resolve f-1 as 'fix', then record rr-2 -> f-1 is archived to 'historical',
    // resolving f-1 must fail, and resolving f-2 on a clean card unblocks!
    let fixed = resolve_finding(
        conn,
        &ResolveFindingInput {
            card_id: "card-rev-guard",
            revision_id: "r-rg-fix1",
            expected_revision: dismissed_revision,
            review_id: None,
            finding_id: "f-1",
            disposition: "fix",
            reason: None,
            resolved_by: "client",
        },
        t,
    )?;
    let rr2 = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-rev-guard",
            revision_id: "r-rg-rr2",
            review_id: "rr-2",
            expected_revision: fixed.planning.card_detail.card.revision,
            stage_run_id: None,
            thread_id: Some("thr-rg-2"),
            snapshot_digest: Some("sha256:snap-rg"),
            verdict: "findings",
            summary: Some("Finding 2"),
            findings: &[ReviewFindingInput {
                id: "f-2",
                severity: "minor",
                criterion_id: None,
                file_path: None,
                line: Some(12),
                message: "Minor note",
            }],
        },
        t,
    )?;
    let f1 = rr2
        .review_findings
        .iter()
        .find(|f| f.id == "f-1")
        .expect("f-1 present");
    assert_eq!(f1.status, "historical");
    assert!(matches!(
        resolve_finding(
            conn,
            &ResolveFindingInput {
                card_id: "card-rev-guard",
                revision_id: "r-rg-hist-bad",
                expected_revision: rr2.planning.card_detail.card.revision,
                review_id: None,
                finding_id: "f-1",
                disposition: "dismiss",
                reason: Some("Cannot mutate historical"),
                resolved_by: "client",
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    Ok(())
}

#[test]
fn ship_blockers_catch_mismatched_review_or_ac_snapshot_and_rejects_check_after_ship() -> TestResult
{
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-ship-snap")?;
    let t = "2026-11-24T04:30:00Z";
    let rev = complete_default_tasks_and_check(conn, "card-ship-snap", rev, "sha256:snap-a", t)?;

    // Probe 4.4 clean card: f-1 resolved 'fix' in rr-1, then rr-2 -> f-2 'accept_risk' -> active!
    let rr1 = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-ship-snap",
            revision_id: "r-ss-rr1",
            review_id: "rr-ss-1",
            expected_revision: rev,
            stage_run_id: None,
            thread_id: Some("thr-ss-1"),
            snapshot_digest: Some("sha256:snap-a"),
            verdict: "findings",
            summary: Some("First"),
            findings: &[ReviewFindingInput {
                id: "f-ss-1",
                severity: "major",
                criterion_id: None,
                file_path: None,
                line: Some(1),
                message: "Need fix",
            }],
        },
        t,
    )?;
    let fixed = resolve_finding(
        conn,
        &ResolveFindingInput {
            card_id: "card-ship-snap",
            revision_id: "r-ss-fix",
            expected_revision: rr1.planning.card_detail.card.revision,
            review_id: None,
            finding_id: "f-ss-1",
            disposition: "fix",
            reason: None,
            resolved_by: "client",
        },
        t,
    )?;
    let rr2 = record_review_run(
        conn,
        &RecordReviewRunInput {
            card_id: "card-ship-snap",
            revision_id: "r-ss-rr2",
            review_id: "rr-ss-2",
            expected_revision: fixed.planning.card_detail.card.revision,
            stage_run_id: None,
            thread_id: Some("thr-ss-2"),
            snapshot_digest: Some("sha256:snap-a"),
            verdict: "findings",
            summary: Some("Second"),
            findings: &[ReviewFindingInput {
                id: "f-ss-2",
                severity: "info",
                criterion_id: None,
                file_path: None,
                line: Some(2),
                message: "Note",
            }],
        },
        t,
    )?;
    let unblocked = resolve_finding(
        conn,
        &ResolveFindingInput {
            card_id: "card-ship-snap",
            revision_id: "r-ss-acc",
            expected_revision: rr2.planning.card_detail.card.revision,
            review_id: None,
            finding_id: "f-ss-2",
            disposition: "accept_risk",
            reason: Some("Acceptable"),
            resolved_by: "client",
        },
        t,
    )?;
    assert_eq!(unblocked.planning.card_detail.card.status, "active");
    verify_snapshot_read_time_and_shipped_check_guards(
        conn,
        unblocked.planning.card_detail.card.revision,
        t,
    )
}

fn verify_snapshot_read_time_and_shipped_check_guards(
    conn: &rusqlite::Connection,
    unblocked_revision: i64,
    t: &str,
) -> TestResult {
    let manual = record_manual_acceptance(
        conn,
        &RecordManualAcceptanceInput {
            card_id: "card-ship-snap",
            revision_id: "r-ss-ac",
            expected_revision: unblocked_revision,
            criterion_id: "AC-02",
            verdict: "pass",
            note: Some("Verified"),
            evidence_ref: None,
            evidence_digest: None,
            expected_product_snapshot_digest: Some("sha256:snap-a"),
            observed_by: "client",
        },
        t,
    )?;

    // Read-time Ship blocker defense: simulate mismatched review snapshot_digest on completed review
    conn.execute(
        "UPDATE workflow_board_review_runs SET snapshot_digest = 'sha256:snap-old' WHERE id = 'rr-ss-2'",
        [],
    )?;
    let blockers = compute_ship_blockers(conn, "card-ship-snap")?;
    assert!(blockers.iter().any(|b| b.kind == "review_stale"));
    conn.execute(
        "UPDATE workflow_board_review_runs SET snapshot_digest = 'sha256:snap-a' WHERE id = 'rr-ss-2'",
        [],
    )?;

    // Also verify AC evidence snapshot mismatch at read time
    conn.execute(
        "UPDATE workflow_board_ac_evidence SET product_snapshot_digest = 'sha256:snap-old' WHERE card_id = 'card-ship-snap' AND criterion_id = 'AC-02'",
        [],
    )?;
    let ac_blockers = compute_ship_blockers(conn, "card-ship-snap")?;
    assert!(ac_blockers.iter().any(|b| b.kind == "ac_unverified"));
    conn.execute(
        "UPDATE workflow_board_ac_evidence SET product_snapshot_digest = 'sha256:snap-a' WHERE card_id = 'card-ship-snap' AND criterion_id = 'AC-02'",
        [],
    )?;

    let shipped = approve_delivery(
        conn,
        &ApproveDeliveryInput {
            card_id: "card-ship-snap",
            revision_id: "r-ss-ship",
            approval_id: "apr-ss-del",
            delivery_receipt_id: "del-ss-1",
            expected_revision: manual.planning.card_detail.card.revision,
            expected_product_snapshot_digest: Some("sha256:snap-a"),
            commit_sha: None,
            diff_digest: None,
            note: Some("Ship"),
            approved_by: "client",
        },
        t,
    )?;
    assert_eq!(shipped.planning.card_detail.card.status, "shipped");

    // record_check_receipt rejects shipped card
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    assert!(matches!(
        record_check_receipt(
            conn,
            &RecordCheckReceiptInput {
                card_id: "card-ship-snap",
                revision_id: "r-ss-chk-after-ship",
                receipt_id: "rcpt-ss-after-ship",
                expected_revision: shipped.planning.card_detail.card.revision,
                check_id: "chk-1",
                argv: &cmd,
                status: "passed",
                exit_code: Some(0),
                log_ref: None,
                stdout_excerpt: None,
                stderr_excerpt: None,
                duration_ms: Some(10),
                snapshot_digest: "sha256:snap-a",
                commit_sha: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    Ok(())
}

#[test]
fn colliding_review_finding_ids_are_disambiguated_across_rounds_and_cards() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev1 = setup_approved_execute_card(conn, "card-col-1")?;
    let rev2 = setup_approved_execute_card(conn, "card-col-2")?;
    let t = "2026-11-25T05:00:00Z";
    let record_one = |card: &str, rev: i64, suffix: &str| {
        record_review_run(
            conn,
            &RecordReviewRunInput {
                card_id: card,
                revision_id: &format!("r-{suffix}"),
                review_id: &format!("rr-{suffix}"),
                expected_revision: rev,
                stage_run_id: None,
                thread_id: Some(&format!("thr-{suffix}")),
                snapshot_digest: None,
                verdict: "findings",
                summary: Some(suffix),
                findings: &[ReviewFindingInput {
                    id: "fnd-1",
                    severity: "major",
                    criterion_id: None,
                    file_path: None,
                    line: None,
                    message: "Colliding finding",
                }],
            },
            t,
        )
    };
    let rr1 = record_one("card-col-1", rev1, "col-1a")?;
    assert_eq!(rr1.review_findings[0].id, "fnd-1");
    let rr2 = record_one("card-col-2", rev2, "col-2a")?;
    assert_ne!(rr2.review_findings[0].id, "fnd-1");
    Ok(())
}
