//! M3 Ship manual acceptance observations, deterministic Ship blocker computation, and
//! client-only `approve_delivery` (`DeliveryReceipt` + `status = 'shipped'`).

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde_json::json;

use super::execution_queries::load_card_execution_detail_inner;
use super::invalidation::{RevisionWrite, compute_prefixed_sha256, insert_revision_ledger};
pub use super::ship_acceptance::record_manual_acceptance;
use super::types_m3::{ApproveDeliveryInput, CardExecutionDetail};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Approves final delivery after verifying zero Ship blockers, inserting a durable
/// `DeliveryReceipt` + `delivery` approval and transitioning the card to `status = 'shipped'`.
pub fn approve_delivery(
    conn: &Connection,
    input: &ApproveDeliveryInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let _revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _approval_id = require_non_empty(input.approval_id, "approval_id")?;
    let _delivery_receipt_id = require_non_empty(input.delivery_receipt_id, "delivery_receipt_id")?;
    let _approved_by = require_non_empty(input.approved_by, "approved_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let detail_before = load_card_execution_detail_inner(conn, card_id)?;
    let existing = &detail_before.planning.card_detail.card;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    if matches!(existing.status.as_str(), "paused" | "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot approve delivery while card status is `{}`",
            existing.status
        )));
    }
    if let Some(expected_snap) = input
        .expected_product_snapshot_digest
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && existing.product_snapshot_digest.as_deref() != Some(expected_snap)
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "digest mismatch: expected product snapshot `{expected_snap}`, actual {:?}",
            existing.product_snapshot_digest
        )));
    }
    if !detail_before.ship_blockers.is_empty() {
        let first = &detail_before.ship_blockers[0];
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "delivery blocked by {} issue(s), first: {} ({})",
            detail_before.ship_blockers.len(),
            first.kind,
            first.message
        )));
    }

    let snapshot_digest = existing
        .product_snapshot_digest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(
            || compute_prefixed_sha256(format!("{card_id}:rev:{}", existing.revision).as_bytes()),
            str::to_owned,
        );
    let checks_digest = {
        let ids = detail_before
            .planning
            .planned_checks
            .iter()
            .map(|c| {
                format!(
                    "{}:{}",
                    c.check_id,
                    c.last_receipt_id.as_deref().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        compute_prefixed_sha256(ids.as_bytes())
    };
    let mut ac_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for ac in &detail_before.planning.ac_evidence {
        *ac_counts.entry(ac.status.as_str()).or_insert(0) += 1;
    }
    let ac_verdicts_summary_json =
        serde_json::to_string(&ac_counts).unwrap_or_else(|_| "{}".into());

    insert_delivery_approval_and_receipt(
        conn,
        &detail_before,
        input,
        &snapshot_digest,
        &checks_digest,
        &ac_verdicts_summary_json,
        now,
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn insert_delivery_approval_and_receipt(
    conn: &Connection,
    detail_before: &CardExecutionDetail,
    input: &ApproveDeliveryInput<'_>,
    snapshot_digest: &str,
    checks_digest: &str,
    ac_verdicts_summary_json: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let existing = &detail_before.planning.card_detail.card;
    let card_id = input.card_id.trim();
    let approval_id = input.approval_id.trim();
    let delivery_receipt_id = input.delivery_receipt_id.trim();
    let approved_by = input.approved_by.trim();
    let review_id = detail_before.review_runs.first().map(|r| r.id.as_str());

    conn.execute(
        "UPDATE workflow_board_approvals
         SET status = 'superseded'
         WHERE card_id = ?1 AND gate = 'delivery' AND status = 'valid'",
        params![card_id],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "supersede_delivery_approvals",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_delivery_receipts
         SET status = 'superseded',
             updated_at = ?2
         WHERE card_id = ?1 AND status IN ('shipped', 'valid')",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "supersede_delivery_receipts",
        source,
    })?;

    let next_revision = existing.revision + 1;
    conn.execute(
        "INSERT INTO workflow_board_approvals (
           id, card_id, gate, status, revision, artifact_digest, spec_digest,
           plan_digest, tasks_digest, product_snapshot_digest, approved_by, approved_at, metadata_json
         ) VALUES (?1, ?2, 'delivery', 'valid', ?3, ?4, ?5, ?6, ?7, ?4, ?8, ?9, ?10)",
        params![
            approval_id,
            card_id,
            next_revision,
            snapshot_digest,
            existing.spec_digest,
            existing.plan_digest,
            existing.tasks_digest,
            approved_by,
            now,
            json!({ "checksDigest": checks_digest, "note": input.note }).to_string(),
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_delivery_approval",
        source,
    })?;

    conn.execute(
        "INSERT INTO workflow_board_delivery_receipts (
           id, card_id, approval_id, status, card_revision, spec_digest, plan_digest,
           tasks_digest, checks_digest, product_snapshot_digest, commit_sha, diff_digest,
           review_id, ac_verdicts_summary_json, delivery_note, approved_by, approved_at,
           created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, 'shipped', ?4, ?5, ?6,
           ?7, ?8, ?9, ?10, ?11,
           ?12, ?13, ?14, ?15, ?16,
           ?16, ?16
         )",
        params![
            delivery_receipt_id,
            card_id,
            approval_id,
            next_revision,
            existing.spec_digest,
            existing.plan_digest,
            existing.tasks_digest,
            checks_digest,
            snapshot_digest,
            input.commit_sha,
            input.diff_digest,
            review_id,
            ac_verdicts_summary_json,
            input.note,
            approved_by,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_delivery_receipt",
        source,
    })?;

    finalize_shipped_card_and_ledger(conn, existing, input, snapshot_digest, now)
}

fn finalize_shipped_card_and_ledger(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &ApproveDeliveryInput<'_>,
    snapshot_digest: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let revision_id = input.revision_id.trim();
    let approval_id = input.approval_id.trim();
    let delivery_receipt_id = input.delivery_receipt_id.trim();
    let next_revision = existing.revision + 1;
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             stage = 'ship',
             status = 'shipped',
             blocker_reason = NULL,
             next_action = 'git_actions',
             product_snapshot_digest = ?4,
             completed_at = ?5,
             updated_at = ?5
         WHERE card_id = ?1 AND revision = ?2",
        params![
            card_id,
            input.expected_revision,
            next_revision,
            snapshot_digest,
            now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "ship_card_update",
        source,
    })?;

    let payload_json = json!({
        "approvalId": approval_id,
        "deliveryReceiptId": delivery_receipt_id,
        "productSnapshotDigest": snapshot_digest,
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: "ship",
            kind: "delivery_approved",
            summary: input.note.unwrap_or("Approved delivery"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}
