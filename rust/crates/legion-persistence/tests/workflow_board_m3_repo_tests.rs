//! Integration tests for M3 `workflow_board_repo` task execution, DAG readiness, and
//! contract-diff violation resolution.

#[path = "workflow_board_m3_support/mod.rs"]
mod support;

use legion_persistence::sqlite::SqliteStore;
use legion_persistence::workflow_board_repo::{
    ClaimOrStartTaskInput, CompleteTaskInput, RecordCheckReceiptInput,
    ResolveContractViolationInput, RetryStepInput, WorkflowBoardRepoError, claim_or_start_task,
    complete_task, record_check_receipt, resolve_contract_violation, retry_step,
};
use support::setup_approved_execute_card;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn task_dag_single_mutating_task_and_contract_violation_flow() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-tasks")?;
    let t = "2026-11-24T01:00:00Z";

    assert!(matches!(
        claim_or_start_task(
            conn,
            &ClaimOrStartTaskInput {
                card_id: "card-tasks",
                revision_id: "r-bad-dag",
                expected_revision: rev,
                task_id: "TSK-2",
                stage_run_id: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let started = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-tasks",
            revision_id: "r-start-1",
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
    assert!(matches!(
        claim_or_start_task(
            conn,
            &ClaimOrStartTaskInput {
                card_id: "card-tasks",
                revision_id: "r-concurrent",
                expected_revision: started.planning.card_detail.card.revision,
                task_id: "TSK-2",
                stage_run_id: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));
    verify_violation_and_resolution_flow(conn, started.planning.card_detail.card.revision, t)
}

fn verify_violation_and_resolution_flow(
    conn: &rusqlite::Connection,
    start_rev: i64,
    t: &str,
) -> TestResult {
    let violated = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-tasks",
            revision_id: "r-viol-1",
            expected_revision: start_rev,
            task_id: "TSK-1",
            outcome_status: "completed",
            stage_run_id: None,
            thread_id: Some("thr-1"),
            workflow_run_id: None,
            diff_digest: Some("sha256:diff-1"),
            product_snapshot_digest: Some("sha256:snap-1"),
            error_message: None,
            violation_id: Some("viol-1"),
            changed_files: &["src/a.rs", "src/extra.rs"],
            unexpected_files: &["src/extra.rs"],
            forbidden_files: &[],
        },
        t,
    )?;
    assert_eq!(violated.planning.card_detail.card.status, "blocked");
    assert_eq!(violated.contract_violations.len(), 1);
    assert!(violated.contract_violations[0].preserved_in_worktree);
    assert_eq!(violated.contract_violations[0].status, "violation_pending");

    assert!(matches!(
        claim_or_start_task(
            conn,
            &ClaimOrStartTaskInput {
                card_id: "card-tasks",
                revision_id: "r-blocked",
                expected_revision: violated.planning.card_detail.card.revision,
                task_id: "TSK-2",
                stage_run_id: None,
            },
            t,
        ),
        Err(WorkflowBoardRepoError::PreconditionFailed(_))
    ));

    let resolved = resolve_contract_violation(
        conn,
        &ResolveContractViolationInput {
            card_id: "card-tasks",
            revision_id: "r-res-viol",
            expected_revision: violated.planning.card_detail.card.revision,
            violation_id: "viol-1",
            disposition: "accept_scope",
            note: Some("Safe addition"),
            resolved_by: "client",
        },
        t,
    )?;
    assert_eq!(resolved.planning.card_detail.card.status, "active");

    let t2 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-tasks",
            revision_id: "r-start-2",
            expected_revision: resolved.planning.card_detail.card.revision,
            task_id: "TSK-2",
            stage_run_id: None,
        },
        t,
    )?;
    let t2_done = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-tasks",
            revision_id: "r-done-2",
            expected_revision: t2.planning.card_detail.card.revision,
            task_id: "TSK-2",
            outcome_status: "completed",
            stage_run_id: None,
            thread_id: Some("thr-2"),
            workflow_run_id: None,
            diff_digest: Some("sha256:diff-2"),
            product_snapshot_digest: Some("sha256:snap-2"),
            error_message: None,
            violation_id: None,
            changed_files: &["src/b.rs"],
            unexpected_files: &[],
            forbidden_files: &[],
        },
        t,
    )?;
    assert_eq!(t2_done.planning.card_detail.card.status, "active");
    Ok(())
}

#[test]
fn complete_task_rejects_unstarted_tasks() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-unstarted")?;
    let t = "2026-11-24T01:30:00Z";

    // Probe 2.4: complete_task must reject unstarted ('todo') task
    assert!(matches!(
        complete_task(
            conn,
            &CompleteTaskInput {
                card_id: "card-unstarted",
                revision_id: "r-unstarted",
                expected_revision: rev,
                task_id: "TSK-2",
                outcome_status: "completed",
                stage_run_id: None,
                thread_id: None,
                workflow_run_id: None,
                diff_digest: None,
                product_snapshot_digest: None,
                error_message: None,
                violation_id: None,
                changed_files: &[],
                unexpected_files: &[],
                forbidden_files: &[],
            },
            t,
        ),
        Err(WorkflowBoardRepoError::InvalidStateTransition(_))
    ));
    Ok(())
}

#[test]
fn retry_step_preserves_orthogonal_blockers() -> TestResult {
    let store = SqliteStore::open_in_memory()?;
    let conn = store.connection();
    let rev = setup_approved_execute_card(conn, "card-guards")?;
    let t = "2026-11-24T01:30:00Z";

    // Reviewer 2: retry_step on failed check must keep card blocked while contract violation is pending
    let s1 = claim_or_start_task(
        conn,
        &ClaimOrStartTaskInput {
            card_id: "card-guards",
            revision_id: "r-s1",
            expected_revision: rev,
            task_id: "TSK-1",
            stage_run_id: None,
        },
        t,
    )?;
    let viol = complete_task(
        conn,
        &CompleteTaskInput {
            card_id: "card-guards",
            revision_id: "r-v1",
            expected_revision: s1.planning.card_detail.card.revision,
            task_id: "TSK-1",
            outcome_status: "completed",
            stage_run_id: None,
            thread_id: None,
            workflow_run_id: None,
            diff_digest: None,
            product_snapshot_digest: Some("sha256:snap-g1"),
            error_message: None,
            violation_id: Some("viol-g1"),
            changed_files: &["src/a.rs", "src/extra.rs"],
            unexpected_files: &["src/extra.rs"],
            forbidden_files: &[],
        },
        t,
    )?;
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let failed_chk = record_check_receipt(
        conn,
        &RecordCheckReceiptInput {
            card_id: "card-guards",
            revision_id: "r-chk-fail",
            receipt_id: "rcpt-fail",
            expected_revision: viol.planning.card_detail.card.revision,
            check_id: "chk-1",
            argv: &cmd,
            status: "failed",
            exit_code: Some(1),
            log_ref: None,
            stdout_excerpt: None,
            stderr_excerpt: Some("fail"),
            duration_ms: Some(10),
            snapshot_digest: "sha256:snap-g1",
            commit_sha: None,
        },
        t,
    )?;
    let retried = retry_step(
        conn,
        &RetryStepInput {
            card_id: "card-guards",
            revision_id: "r-retry-chk",
            expected_revision: failed_chk.planning.card_detail.card.revision,
            step_kind: "check",
            target_id: Some("chk-1"),
            reason: Some("Retry check"),
        },
        t,
    )?;
    assert_eq!(retried.planning.card_detail.card.status, "blocked");
    assert!(retried.planning.card_detail.card.blocker_reason.is_some());
    Ok(())
}
