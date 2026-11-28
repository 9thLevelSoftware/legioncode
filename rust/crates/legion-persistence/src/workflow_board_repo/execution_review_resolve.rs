//! M3 Execute review-finding disposition resolution inside a strict CAS `WriteTx`.

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::blocker_status::{compute_card_blocker_status, recompute_card_blocker_status};
use super::cards::load_card_row;
use super::execution_queries::load_card_execution_detail_inner;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m3::{CardExecutionDetail, ResolveFindingInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Applies a human operator disposition to an open review finding inside a strict CAS `WriteTx`.
pub fn resolve_finding(
    conn: &Connection,
    input: &ResolveFindingInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let expected_review_id = input
        .review_id
        .map(|id| require_non_empty(id, "review_id"))
        .transpose()?;
    let finding_id = require_non_empty(input.finding_id, "finding_id")?;
    let disposition = require_non_empty(input.disposition, "disposition")?;
    let resolved_by = require_non_empty(input.resolved_by, "resolved_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(disposition, "fix" | "accept_risk" | "dismiss" | "defer") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported finding disposition `{disposition}`"
        )));
    }
    if matches!(disposition, "dismiss" | "accept_risk")
        && input.reason.map(str::trim).is_none_or(str::is_empty)
    {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "reason is required for `dismiss` or `accept_risk`".to_owned(),
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
            "cannot resolve finding while card status is `{}`",
            existing.status
        )));
    }

    validate_finding_for_resolve(conn, card_id, finding_id, expected_review_id)?;

    let updated = conn
        .execute(
            "UPDATE workflow_board_review_findings
             SET status = 'resolved', disposition = ?3, resolution_note = ?4,
                 resolved_by = ?5, resolved_at = ?6, updated_at = ?6
             WHERE id = ?1 AND card_id = ?2 AND status IN ('open', 'resolved')",
            params![
                finding_id,
                card_id,
                disposition,
                input.reason,
                resolved_by,
                now
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_review_finding_disposition",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "review finding `{finding_id}` not found on card `{card_id}`"
        )));
    }

    update_card_after_finding_resolution(conn, &existing, input, revision_id, now)?;
    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn validate_finding_for_resolve(
    conn: &Connection,
    card_id: &str,
    finding_id: &str,
    expected_review_id: Option<&str>,
) -> Result<(), WorkflowBoardRepoError> {
    let status_pair: Option<(String, String, String)> = conn
        .query_row(
            "SELECT f.status, r.status, f.review_run_id
             FROM workflow_board_review_findings f
             JOIN workflow_board_review_runs r ON r.id = f.review_run_id
             WHERE f.id = ?1 AND f.card_id = ?2",
            params![finding_id, card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_review_finding_for_resolve",
            source,
        })?;
    let Some((finding_status, review_status, review_run_id)) = status_pair else {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "review finding `{finding_id}` not found on card `{card_id}`"
        )));
    };
    if let Some(expected_review) = expected_review_id
        && expected_review != review_run_id
    {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "review finding `{finding_id}` belongs to review `{review_run_id}`, not `{expected_review}`"
        )));
    }
    if !matches!(finding_status.as_str(), "open" | "resolved") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot resolve review finding `{finding_id}` with status `{finding_status}`"
        )));
    }
    if matches!(review_status.as_str(), "stale" | "historical") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot resolve finding `{finding_id}` on `{review_status}` review run"
        )));
    }
    Ok(())
}

fn update_card_after_finding_resolution(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &ResolveFindingInput<'_>,
    revision_id: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let finding_id = input.finding_id.trim();
    let disposition = input.disposition.trim();
    let resolved_by = input.resolved_by.trim();
    let blocker = compute_card_blocker_status(conn, card_id)?;
    let next_revision = existing.revision + 1;
    let override_blocked_next_action = if disposition == "fix" && !blocker.has_high_priority_blocker
    {
        Some("fix_finding")
    } else {
        None
    };
    recompute_card_blocker_status(
        conn,
        existing,
        next_revision,
        "ready_for_ship",
        None,
        override_blocked_next_action,
        None,
        now,
    )?;

    let payload_json = json!({
        "findingId": finding_id,
        "disposition": disposition,
        "resolvedBy": resolved_by,
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "finding_resolved",
            summary: &format!("Resolved finding {finding_id} ({disposition})"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}
