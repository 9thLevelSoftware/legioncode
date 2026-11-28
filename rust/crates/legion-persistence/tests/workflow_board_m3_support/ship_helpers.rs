//! Shared ship/review test setup helper for M3 integration tests.

use legion_persistence::workflow_board_repo::{
    ClaimOrStartTaskInput, CompleteTaskInput, RecordCheckReceiptInput, WorkflowBoardRepoError,
    claim_or_start_task, complete_task, record_check_receipt,
};
use rusqlite::Connection;

pub fn complete_default_tasks_and_check(
    conn: &Connection,
    card_id: &str,
    mut cur_rev: i64,
    snapshot_digest: &str,
    t: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    for (tid, rid1, rid2) in [("TSK-1", "r1a", "r1b"), ("TSK-2", "r2a", "r2b")] {
        let s = claim_or_start_task(
            conn,
            &ClaimOrStartTaskInput {
                card_id,
                revision_id: &format!("{card_id}-{rid1}"),
                expected_revision: cur_rev,
                task_id: tid,
                stage_run_id: None,
            },
            t,
        )?;
        let d = complete_task(
            conn,
            &CompleteTaskInput {
                card_id,
                revision_id: &format!("{card_id}-{rid2}"),
                expected_revision: s.planning.card_detail.card.revision,
                task_id: tid,
                outcome_status: "completed",
                stage_run_id: None,
                thread_id: None,
                workflow_run_id: None,
                diff_digest: None,
                product_snapshot_digest: Some(snapshot_digest),
                error_message: None,
                violation_id: None,
                changed_files: &[],
                unexpected_files: &[],
                forbidden_files: &[],
            },
            t,
        )?;
        cur_rev = d.planning.card_detail.card.revision;
    }
    let cmd = vec!["cargo".to_owned(), "test".to_owned()];
    let chk = record_check_receipt(
        conn,
        &RecordCheckReceiptInput {
            card_id,
            revision_id: &format!("{card_id}-r-chk-1"),
            receipt_id: &format!("{card_id}-rcpt-1"),
            expected_revision: cur_rev,
            check_id: "chk-1",
            argv: &cmd,
            status: "passed",
            exit_code: Some(0),
            log_ref: Some("logs/chk-1.log"),
            stdout_excerpt: Some("ok"),
            stderr_excerpt: None,
            duration_ms: Some(120),
            snapshot_digest,
            commit_sha: Some("abc123"),
        },
        t,
    )?;
    Ok(chk.planning.card_detail.card.revision)
}
