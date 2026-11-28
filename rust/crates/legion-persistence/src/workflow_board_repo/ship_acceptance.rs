//! M3 Ship manual acceptance-criterion verification inside a strict CAS `WriteTx`.

use rusqlite::{Connection, params};
use serde_json::json;

use super::cards::load_card_row;
use super::execution_queries::load_card_execution_detail_inner;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m3::{CardExecutionDetail, RecordManualAcceptanceInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

fn map_manual_verdict_to_sqlite(verdict: &str) -> Result<&'static str, WorkflowBoardRepoError> {
    match verdict.trim() {
        "pass" => Ok("pass"),
        "fail" => Ok("fail"),
        "not-applicable" | "not_applicable" => Ok("not_applicable"),
        other => Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported manual acceptance verdict `{other}`"
        ))),
    }
}

/// Records a human acceptance-criterion observation (`pass | fail | not-applicable` -> SQLite
/// `not_applicable`) inside a strict CAS `WriteTx`.
pub fn record_manual_acceptance(
    conn: &Connection,
    input: &RecordManualAcceptanceInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let _revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _criterion_id = require_non_empty(input.criterion_id, "criterion_id")?;
    let _observed_by = require_non_empty(input.observed_by, "observed_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    let sqlite_status = map_manual_verdict_to_sqlite(input.verdict)?;
    if matches!(sqlite_status, "fail" | "not_applicable")
        && input.note.map(str::trim).is_none_or(str::is_empty)
    {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "observation note is required for `fail` or `not-applicable`".to_owned(),
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
            "cannot record manual acceptance while card status is `{}`",
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

    apply_manual_acceptance_update(conn, &existing, input, sqlite_status, now)?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn apply_manual_acceptance_update(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &RecordManualAcceptanceInput<'_>,
    sqlite_status: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let revision_id = input.revision_id.trim();
    let criterion_id = input.criterion_id.trim();
    let observed_by = input.observed_by.trim();
    let updated = conn
        .execute(
            "UPDATE workflow_board_ac_evidence
             SET status = ?3,
                 observation_note = ?4,
                 evidence_ref = COALESCE(?5, evidence_ref),
                 evidence_digest = COALESCE(?6, evidence_digest),
                 product_snapshot_digest = COALESCE(?7, product_snapshot_digest),
                 stale_reason = NULL,
                 observed_by = ?8,
                 observed_at = ?9,
                 updated_at = ?9
             WHERE card_id = ?1 AND criterion_id = ?2",
            params![
                card_id,
                criterion_id,
                sqlite_status,
                input.note,
                input.evidence_ref,
                input.evidence_digest,
                existing.product_snapshot_digest,
                observed_by,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "record_manual_acceptance_update",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "acceptance criterion `{criterion_id}` not found in AC evidence mappings"
        )));
    }

    let next_revision = existing.revision + 1;
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             updated_at = ?4
         WHERE card_id = ?1 AND revision = ?2",
        params![card_id, input.expected_revision, next_revision, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_card_on_manual_acceptance",
        source,
    })?;

    let payload_json = json!({
        "criterionId": criterion_id,
        "verdict": input.verdict,
        "observedBy": observed_by,
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "manual_acceptance_recorded",
            summary: &format!("Recorded {criterion_id} verdict ({})", input.verdict),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}
