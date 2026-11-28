//! M3 card lifecycle controls (`pause_card`, `resume_card`, `retry_step`).

use rusqlite::{Connection, params};
use serde_json::json;

use super::blocker_status::{compute_card_blocker_status, recompute_card_blocker_status};
use super::cards::load_card_row;
use super::execution_actions::ensure_no_unreconciled_actions;
use super::execution_queries::load_card_execution_detail_inner;
use super::execution_retry::reset_retry_target;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m3::{CardExecutionDetail, PauseCardInput, ResumeCardInput, RetryStepInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Pauses an active/blocked card, marking running stage runs paused and in-progress actions interrupted.
pub fn pause_card(
    conn: &Connection,
    input: &PauseCardInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    if existing.status == "paused" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(
            "card is already paused".to_owned(),
        ));
    }
    if matches!(existing.status.as_str(), "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot pause card with status `{}`",
            existing.status
        )));
    }

    let paused_from = if existing.status == "blocked" {
        "blocked"
    } else {
        "active"
    };
    let next_revision = existing.revision + 1;
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             status = 'paused',
             paused_from_status = ?4,
             pause_reason = ?5,
             paused_at = ?6,
             next_action = 'resume_card',
             updated_at = ?6
         WHERE card_id = ?1 AND revision = ?2",
        params![
            card_id,
            input.expected_revision,
            next_revision,
            paused_from,
            input.reason,
            now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "pause_card_update",
        source,
    })?;

    conn.execute(
        "UPDATE workflow_board_external_actions
         SET status = 'interrupted',
             updated_at = ?2
         WHERE card_id = ?1 AND status = 'in_progress'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "interrupt_actions_on_pause",
        source,
    })?;

    conn.execute(
        "UPDATE workflow_board_stage_runs
         SET status = 'paused',
             updated_at = ?2
         WHERE card_id = ?1 AND status IN ('pending', 'launching', 'running')",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "pause_stage_runs",
        source,
    })?;

    let payload_json =
        json!({ "pausedFromStatus": paused_from, "reason": input.reason }).to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "card_paused",
            summary: input.reason.unwrap_or("Paused card"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

/// Resumes a paused card after verifying interrupted external actions and incomplete write-journal
/// entries have been reconciled.
pub fn resume_card(
    conn: &Connection,
    input: &ResumeCardInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    if existing.status != "paused" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot resume card with status `{}` (must be `paused`)",
            existing.status
        )));
    }
    ensure_no_unreconciled_actions(conn, card_id)?;

    let incomplete_journal: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_write_journal
             WHERE card_id = ?1 AND status NOT IN ('committed', 'rolled_back')",
            params![card_id],
            |r| r.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_incomplete_journal_on_resume",
            source,
        })?;
    if incomplete_journal > 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "incomplete write-journal entries must be reconciled before resume".to_owned(),
        ));
    }

    let blocker = compute_card_blocker_status(conn, card_id)?;
    let restored = if blocker.is_blocked {
        "blocked"
    } else {
        "active"
    };
    let next_revision = existing.revision + 1;
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             status = ?4,
             paused_from_status = NULL,
             pause_reason = NULL,
             paused_at = NULL,
             blocker_reason = ?5,
             next_action = CASE WHEN ?4 = 'blocked' THEN ?6 ELSE 'continue_stage' END,
             updated_at = ?7
         WHERE card_id = ?1 AND revision = ?2",
        params![
            card_id,
            input.expected_revision,
            next_revision,
            restored,
            blocker.blocker_reason,
            blocker.default_next_action,
            now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "resume_card_update",
        source,
    })?;

    let payload_json = json!({ "restoredStatus": restored, "note": input.note }).to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "card_resumed",
            summary: input.note.unwrap_or("Resumed card"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

/// Retries a failed/blocked task, check, review, or stage after verifying no interrupted
/// external actions are pending reconciliation.
pub fn retry_step(
    conn: &Connection,
    input: &RetryStepInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let step_kind = require_non_empty(input.step_kind, "step_kind")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(step_kind, "task" | "check" | "review" | "stage") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported step_kind `{step_kind}`"
        )));
    }
    if matches!(step_kind, "task" | "check")
        && input.target_id.map(str::trim).is_none_or(str::is_empty)
    {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "target_id is required for step_kind `{step_kind}`"
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
    if matches!(existing.status.as_str(), "paused" | "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot retry step while card status is `{}`",
            existing.status
        )));
    }
    ensure_no_unreconciled_actions(conn, card_id)?;
    reset_retry_target(conn, card_id, step_kind, input.target_id, now)?;

    let next_revision = existing.revision + 1;
    recompute_card_blocker_status(
        conn,
        &existing,
        next_revision,
        &format!("retry_{step_kind}"),
        None,
        None,
        None,
        now,
    )?;

    let payload_json = json!({
        "stepKind": step_kind,
        "targetId": input.target_id,
        "reason": input.reason,
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "step_retried",
            summary: &format!("Retried {step_kind}"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}
