//! M3 snapshot-bound planned check receipts and automatic staleness invalidation.

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::blocker_status::recompute_card_blocker_status;
use super::cards::load_card_row;
use super::execution_check_sync::sync_automated_ac_evidence;
use super::execution_queries::load_card_execution_detail_inner;
use super::invalidation::{RevisionWrite, insert_revision_ledger, stale_approvals_for_gates};
use super::types_m3::{CardExecutionDetail, RecordCheckReceiptInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Marks snapshot-bound planned checks, check receipts, AC evidence, review runs/findings, and
/// delivery receipts `'stale'` when their bound snapshot differs from `current_snapshot_digest`.
pub fn invalidate_snapshot_bound_evidence(
    conn: &Connection,
    card_id: &str,
    current_snapshot_digest: &str,
    reason: &str,
    now_iso: &str,
) -> Result<usize, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let snap = require_non_empty(current_snapshot_digest, "current_snapshot_digest")?;
    let reason = require_non_empty(reason, "reason")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let checks = conn
        .execute(
            "UPDATE workflow_board_planned_checks
             SET status = 'stale',
                 stale_reason = ?3,
                 updated_at = ?4
             WHERE card_id = ?1
               AND status IN ('passed', 'failed')
               AND COALESCE(product_snapshot_digest, '') != ?2",
            params![card_id, snap, reason, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "invalidate_snapshot_planned_checks",
            source,
        })?;
    let receipts = conn
        .execute(
            "UPDATE workflow_board_check_receipts
             SET status = 'stale',
                 stale_reason = ?3,
                 invalidated_at = ?4,
                 updated_at = ?4
             WHERE card_id = ?1
               AND status IN ('passed', 'failed')
               AND COALESCE(snapshot_digest, '') != ?2",
            params![card_id, snap, reason, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "invalidate_snapshot_check_receipts",
            source,
        })?;
    let ac = conn
        .execute(
            "UPDATE workflow_board_ac_evidence
             SET status = 'stale',
                 stale_reason = ?3,
                 updated_at = ?4
             WHERE card_id = ?1
               AND status IN ('pass', 'fail', 'not_applicable')
               AND COALESCE(product_snapshot_digest, '') != ?2",
            params![card_id, snap, reason, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "invalidate_snapshot_ac_evidence",
            source,
        })?;
    let reviews = conn
        .execute(
            "UPDATE workflow_board_review_runs
             SET status = 'stale',
                 verdict = 'stale',
                 stale_reason = ?3,
                 updated_at = ?4
             WHERE card_id = ?1
               AND status IN ('pending', 'running', 'completed', 'failed')
               AND COALESCE(snapshot_digest, '') != ?2",
            params![card_id, snap, reason, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "invalidate_snapshot_reviews",
            source,
        })?;
    if reviews > 0 {
        conn.execute(
            "UPDATE workflow_board_review_findings
             SET status = 'stale',
                 updated_at = ?2
             WHERE card_id = ?1 AND status = 'open'",
            params![card_id, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "invalidate_snapshot_review_findings",
            source,
        })?;
    }
    stale_approvals_for_gates(conn, card_id, &["delivery"], reason, now)?;
    tx.commit()?;
    Ok(checks + receipts + ac + reviews)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Inserts receipt, updates planned check, and syncs AC evidence in one WriteTx"
)]
fn persist_check_receipt_rows(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &RecordCheckReceiptInput<'_>,
    stored_argv_json: &str,
    owner_task_id: Option<&str>,
    snapshot_digest: &str,
    next_revision: i64,
    now: &str,
) -> Result<Option<String>, WorkflowBoardRepoError> {
    let card_id = existing.card_id.as_str();
    let check_id = input.check_id.trim();
    let receipt_id = input.receipt_id.trim();
    let status = input.status.trim();
    let output_excerpt = input
        .stderr_excerpt
        .filter(|s| !s.trim().is_empty())
        .or(input.stdout_excerpt);

    conn.execute(
        "INSERT INTO workflow_board_check_receipts (
           id, card_id, check_id, task_id, card_revision, argv_json, snapshot_digest,
           commit_sha, spec_digest, plan_digest, status, exit_code, log_ref,
           stdout_excerpt, stderr_excerpt, duration_ms, executed_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
        params![
            receipt_id,
            card_id,
            check_id,
            owner_task_id,
            next_revision,
            stored_argv_json,
            snapshot_digest,
            input.commit_sha,
            existing.spec_digest,
            existing.plan_digest,
            status,
            input.exit_code,
            input.log_ref,
            input.stdout_excerpt,
            input.stderr_excerpt,
            input.duration_ms,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_check_receipt",
        source,
    })?;

    conn.execute(
        "UPDATE workflow_board_planned_checks
         SET status = ?3,
             last_exit_code = ?4,
             last_log_ref = COALESCE(?5, last_log_ref),
             product_snapshot_digest = ?6,
             stale_reason = NULL,
             last_receipt_id = ?7,
             last_run_at = ?8,
             last_duration_ms = ?9,
             last_output_excerpt = ?10,
             updated_at = ?8
         WHERE card_id = ?1 AND check_id = ?2",
        params![
            card_id,
            check_id,
            status,
            input.exit_code,
            input.log_ref,
            snapshot_digest,
            receipt_id,
            now,
            input.duration_ms,
            output_excerpt,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_planned_check_receipt",
        source,
    })?;

    sync_automated_ac_evidence(conn, card_id, snapshot_digest, now)?;
    let blocker_reason = if status == "failed" {
        Some(format!(
            "Planned check {check_id} failed (exit {:?})",
            input.exit_code
        ))
    } else {
        None
    };
    Ok(blocker_reason)
}

/// Records a snapshot-bound planned check receipt, updating `workflow_board_planned_checks` and
/// invalidating evidence bound to older product snapshots.
pub fn record_check_receipt(
    conn: &Connection,
    input: &RecordCheckReceiptInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let _revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _receipt_id = require_non_empty(input.receipt_id, "receipt_id")?;
    let check_id = require_non_empty(input.check_id, "check_id")?;
    let status = require_non_empty(input.status, "status")?;
    let snapshot_digest = require_non_empty(input.snapshot_digest, "snapshot_digest")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(status, "passed" | "failed") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "record_check_receipt status must be `passed` or `failed`, got `{status}`"
        )));
    }
    if !legion_contracts::workflow_board::is_valid_argv_command(input.argv) {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "planned check argv must be a valid argv-only command".to_owned(),
        ));
    }

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    if matches!(existing.status.as_str(), "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot record check receipt while card status is `{}`",
            existing.status
        )));
    }

    let planned: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT argv_json, owner_task_id FROM workflow_board_planned_checks
             WHERE card_id = ?1 AND check_id = ?2",
            params![card_id, check_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_planned_check_for_receipt",
            source,
        })?;
    let Some((stored_argv_json, owner_task_id)) = planned else {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "planned check `{check_id}` does not exist on card `{card_id}`"
        )));
    };
    let stored_argv: Vec<String> = serde_json::from_str(&stored_argv_json).unwrap_or_default();
    if stored_argv != input.argv {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "planned check `{check_id}` argv mismatch against approved plan"
        )));
    }

    if existing.product_snapshot_digest.as_deref() != Some(snapshot_digest) {
        invalidate_snapshot_bound_evidence(
            conn,
            card_id,
            snapshot_digest,
            "product_snapshot_changed",
            now,
        )?;
    }

    let next_revision = existing.revision + 1;
    let blocker_reason = persist_check_receipt_rows(
        conn,
        &existing,
        input,
        &stored_argv_json,
        owner_task_id.as_deref(),
        snapshot_digest,
        next_revision,
        now,
    )?;
    finalize_check_receipt_card(
        conn,
        &existing,
        input,
        next_revision,
        blocker_reason.as_deref(),
        now,
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn finalize_check_receipt_card(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &RecordCheckReceiptInput<'_>,
    next_revision: i64,
    blocker_reason: Option<&str>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let revision_id = input.revision_id.trim();
    let receipt_id = input.receipt_id.trim();
    let check_id = input.check_id.trim();
    let status = input.status.trim();
    let snapshot_digest = input.snapshot_digest.trim();

    recompute_card_blocker_status(
        conn,
        existing,
        next_revision,
        "run_review",
        blocker_reason,
        None,
        Some(snapshot_digest),
        now,
    )?;

    let payload_json = json!({
        "receiptId": receipt_id,
        "checkId": check_id,
        "status": status,
        "snapshotDigest": snapshot_digest,
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "check_receipt_recorded",
            summary: &format!("Check {check_id} {status}"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}
