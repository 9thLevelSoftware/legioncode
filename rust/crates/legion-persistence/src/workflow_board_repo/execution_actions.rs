//! M3 plan/spec amendment proposals and durable external-action reconciliation.

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::cards::load_card_row;
use super::execution_checks::invalidate_snapshot_bound_evidence;
use super::execution_queries::load_card_execution_detail_inner;
use super::invalidation::{
    RevisionWrite, insert_revision_ledger_with_actor, stale_active_challenges,
    stale_approvals_for_gates, stale_downstream_evidence,
};
use super::plan_return::{reset_amended_blocked_tasks_to_todo, reset_in_progress_tasks_to_todo};
use super::types_m3::{
    CardExecutionDetail, ExternalActionRow, ProposeAmendmentInput, ReconcileInterruptedActionInput,
    RecordExternalActionInput,
};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

pub(crate) fn ensure_no_unreconciled_actions(
    conn: &Connection,
    card_id: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let interrupted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_external_actions
             WHERE card_id = ?1 AND status = 'interrupted'",
            params![card_id],
            |r| r.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_interrupted_actions_lifecycle",
            source,
        })?;
    if interrupted > 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "cannot proceed while interrupted external actions remain unreconciled".to_owned(),
        ));
    }
    Ok(())
}

fn insert_amendment_and_stale(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &ProposeAmendmentInput<'_>,
    target_stage: &str,
    reason: &str,
    proposed_by: &str,
    now: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    let card_id = existing.card_id.as_str();
    let next_revision = existing.revision + 1;
    let unexpected_json = serde_json::to_string(input.unexpected_files).map_err(|source| {
        WorkflowBoardRepoError::Json {
            operation: "serialize_amendment_unexpected_files",
            source,
        }
    })?;
    conn.execute(
        "INSERT INTO workflow_board_amendments (
           id, card_id, card_revision, target_stage, status, reason, summary,
           source_task_id, unexpected_files_json, proposed_changes_json, proposed_by,
           created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, 'proposed', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            input.amendment_id.trim(),
            card_id,
            next_revision,
            target_stage,
            reason,
            input.summary,
            input.source_task_id,
            unexpected_json,
            input.proposed_changes_json,
            proposed_by,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_amendment",
        source,
    })?;

    let reason_tag = format!("amendment_proposed:{reason}");
    if target_stage == "spec" {
        stale_approvals_for_gates(
            conn,
            card_id,
            &["spec", "plan", "delivery"],
            &reason_tag,
            now,
        )?;
        stale_active_challenges(conn, card_id, now)?;
    } else {
        stale_approvals_for_gates(conn, card_id, &["plan", "delivery"], &reason_tag, now)?;
    }
    stale_downstream_evidence(conn, card_id, &reason_tag, now)?;
    Ok(next_revision)
}

/// Records a plan/spec amendment proposal, transitions the card back to `target_stage` (`spec` or
/// `plan`), and stales affected approvals + downstream evidence while preserving full history.
pub fn propose_amendment(
    conn: &Connection,
    input: &ProposeAmendmentInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _amendment_id = require_non_empty(input.amendment_id, "amendment_id")?;
    let target_stage = require_non_empty(input.target_stage, "target_stage")?;
    let reason = require_non_empty(input.reason, "reason")?;
    let proposed_by = require_non_empty(input.proposed_by, "proposed_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(target_stage, "spec" | "plan") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "amendment target_stage must be `spec` or `plan`, got `{target_stage}`"
        )));
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
    if existing.stage == "spec" && target_stage == "plan" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(
            "cannot propose `plan` amendment while card is in `spec` stage".to_owned(),
        ));
    }
    ensure_no_unreconciled_actions(conn, card_id)?;

    let next_revision = insert_amendment_and_stale(
        conn,
        &existing,
        input,
        target_stage,
        reason,
        proposed_by,
        now,
    )?;

    let next_action = if target_stage == "spec" {
        "revise_spec"
    } else {
        "revise_plan"
    };
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             stage = ?4,
             status = 'active',
             completed_at = NULL,
             paused_from_status = NULL,
             pause_reason = NULL,
             paused_at = NULL,
             blocker_reason = NULL,
             current_task_id = NULL,
             next_action = ?5,
             updated_at = ?6
         WHERE card_id = ?1 AND revision = ?2",
        params![
            card_id,
            input.expected_revision,
            next_revision,
            target_stage,
            next_action,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_card_on_amendment",
        source,
    })?;
    reset_in_progress_tasks_to_todo(conn, card_id, now)?;
    reset_amended_blocked_tasks_to_todo(conn, card_id, input.source_task_id, now)?;

    let payload_json = json!({
        "amendmentId": input.amendment_id.trim(),
        "targetStage": target_stage,
        "reason": reason,
    })
    .to_string();
    insert_revision_ledger_with_actor(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: target_stage,
            kind: "amendment_proposed",
            summary: &format!("Proposed amendment to {target_stage}: {reason}"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
        proposed_by,
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

/// Idempotently records or retrieves an external action by `(card_id, idempotency_key)`.
pub fn record_external_action(
    conn: &Connection,
    input: &RecordExternalActionInput<'_>,
    now_iso: &str,
) -> Result<ExternalActionRow, WorkflowBoardRepoError> {
    let id = require_non_empty(input.id, "id")?;
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let action_kind = require_non_empty(input.action_kind, "action_kind")?;
    let idempotency_key = require_non_empty(input.idempotency_key, "idempotency_key")?;
    let status = require_non_empty(input.status, "status")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let _card = load_card_row(conn, card_id)?;
    conn.execute(
        "INSERT OR IGNORE INTO workflow_board_external_actions (
           id, card_id, action_kind, target_id, stage_run_id, idempotency_key,
           status, expected_snapshot_digest, observed_snapshot_digest, details_json,
           created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id,
            card_id,
            action_kind,
            input.target_id,
            input.stage_run_id,
            idempotency_key,
            status,
            input.expected_snapshot_digest,
            input.observed_snapshot_digest,
            input.details_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_external_action",
        source,
    })?;

    let row = conn
        .query_row(
            "SELECT id, card_id, action_kind, target_id, stage_run_id, idempotency_key, status,
                    expected_snapshot_digest, observed_snapshot_digest, details_json,
                    reconciliation_note, reconciled_by, reconciled_at, created_at, updated_at
             FROM workflow_board_external_actions
             WHERE card_id = ?1 AND idempotency_key = ?2",
            params![card_id, idempotency_key],
            map_external_action_row,
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_external_action",
            source,
        })?;
    tx.commit()?;
    Ok(row)
}

fn map_external_action_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalActionRow> {
    Ok(ExternalActionRow {
        id: r.get(0)?,
        card_id: r.get(1)?,
        action_kind: r.get(2)?,
        target_id: r.get(3)?,
        stage_run_id: r.get(4)?,
        idempotency_key: r.get(5)?,
        status: r.get(6)?,
        expected_snapshot_digest: r.get(7)?,
        observed_snapshot_digest: r.get(8)?,
        details_json: r.get(9)?,
        reconciliation_note: r.get(10)?,
        reconciled_by: r.get(11)?,
        reconciled_at: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

fn load_external_action_row(
    conn: &Connection,
    action_id: &str,
) -> Result<ExternalActionRow, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT id, card_id, action_kind, target_id, stage_run_id, idempotency_key, status,
                expected_snapshot_digest, observed_snapshot_digest, details_json,
                reconciliation_note, reconciled_by, reconciled_at, created_at, updated_at
         FROM workflow_board_external_actions
         WHERE id = ?1",
        params![action_id],
        map_external_action_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "reload_reconciled_action",
        source,
    })?
    .ok_or_else(|| WorkflowBoardRepoError::PreconditionFailed("external action missing".to_owned()))
}

/// Reconciles an interrupted external action and stales snapshot-bound evidence if the observed
/// product snapshot changed.
pub fn reconcile_interrupted_action(
    conn: &Connection,
    input: &ReconcileInterruptedActionInput<'_>,
    now_iso: &str,
) -> Result<ExternalActionRow, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let action_id = require_non_empty(input.action_id, "action_id")?;
    let status = require_non_empty(input.status, "status")?;
    let reconciled_by = require_non_empty(input.reconciled_by, "reconciled_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(status, "reconciled" | "completed" | "failed" | "cancelled") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported reconciled status `{status}`"
        )));
    }

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    let updated = conn
        .execute(
            "UPDATE workflow_board_external_actions
             SET status = ?3,
                 observed_snapshot_digest = COALESCE(?4, observed_snapshot_digest),
                 reconciliation_note = ?5,
                 reconciled_by = ?6,
                 reconciled_at = ?7,
                 updated_at = ?7
             WHERE id = ?1 AND card_id = ?2",
            params![
                action_id,
                card_id,
                status,
                input.observed_snapshot_digest,
                input.note,
                reconciled_by,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "reconcile_external_action",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "external action `{action_id}` not found on card `{card_id}`"
        )));
    }

    if let Some(obs_snap) = input
        .observed_snapshot_digest
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && existing.product_snapshot_digest.as_deref() != Some(obs_snap)
    {
        invalidate_snapshot_bound_evidence(
            conn,
            card_id,
            obs_snap,
            "product_snapshot_changed",
            now,
        )?;
        conn.execute(
            "UPDATE workflow_board_cards
             SET product_snapshot_digest = ?2,
                 updated_at = ?3
             WHERE card_id = ?1",
            params![card_id, obs_snap, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_snapshot_on_reconcile",
            source,
        })?;
    }

    let row = load_external_action_row(conn, action_id)?;
    tx.commit()?;
    Ok(row)
}
