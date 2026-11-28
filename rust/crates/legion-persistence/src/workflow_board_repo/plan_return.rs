//! Revision return (`return_for_revision`) inside strict CAS `WriteTx`.

use rusqlite::{Connection, params};
use serde_json::json;

use super::blocker_status::compute_card_blocker_status;
use super::cards::load_card_row;
use super::invalidation::{
    RevisionWrite, insert_revision_ledger, stale_active_challenges, stale_approvals_for_gates,
    stale_downstream_evidence,
};
use super::planning_queries::load_card_planning_detail_inner;
use super::types_m2::{CardPlanningDetail, ReturnForRevisionInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

fn stage_rank(stage: &str) -> Option<u8> {
    match stage {
        "spec" => Some(0),
        "plan" => Some(1),
        "execute" => Some(2),
        "ship" => Some(3),
        _ => None,
    }
}

pub(crate) fn restore_remaining_blocker_on_return(
    conn: &Connection,
    card_id: &str,
    target: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let remaining_blocker_reason: Option<String> = if target == "execute" {
        compute_card_blocker_status(conn, card_id)?.blocker_reason
    } else {
        let interrupted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workflow_board_external_actions
                 WHERE card_id = ?1 AND status = 'interrupted'",
                params![card_id],
                |r| r.get(0),
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "count_interrupted_actions_on_return",
                source,
            })?;
        (interrupted > 0).then(|| "Interrupted external action requires reconciliation".to_owned())
    };
    if let Some(blocker_reason) = remaining_blocker_reason {
        conn.execute(
            "UPDATE workflow_board_cards
             SET status = 'blocked',
                 blocker_reason = ?2
             WHERE card_id = ?1",
            params![card_id, blocker_reason],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "restore_blocker_status_on_return",
            source,
        })?;
    }
    Ok(())
}

pub(crate) fn reset_in_progress_tasks_to_todo(
    conn: &Connection,
    card_id: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_tasks
         SET status = 'todo',
             updated_at = ?2
         WHERE card_id = ?1 AND status = 'in_progress'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "reset_in_progress_tasks_to_todo",
        source,
    })?;
    Ok(())
}

pub(crate) fn reset_amended_blocked_tasks_to_todo(
    conn: &Connection,
    card_id: &str,
    source_task_id: Option<&str>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_tasks
         SET status = 'todo',
             blocker_reason = NULL,
             updated_at = ?2
         WHERE card_id = ?1
           AND status = 'blocked'
           AND (
             task_id = ?3
             OR task_id IN (
               SELECT task_id FROM workflow_board_contract_violations
               WHERE card_id = ?1 AND status = 'amended'
             )
           )",
        params![card_id, now, source_task_id.map(str::trim)],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "reset_amended_blocked_tasks_to_todo",
        source,
    })?;
    Ok(())
}

fn persist_return_and_invalidate(
    conn: &Connection,
    existing: &super::CardRow,
    target: &str,
    reason: &str,
    revision_id: &str,
    expected_revision: i64,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = existing.card_id.as_str();
    let next_revision = existing.revision + 1;
    let next_action = match target {
        "spec" => "revise_spec",
        "plan" => "revise_plan",
        _ => "revise_execute",
    };
    let clear_current_task = matches!(target, "spec" | "plan");
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 stage = ?4,
                 status = 'active',
                 completed_at = NULL,
                 paused_from_status = NULL,
                 pause_reason = NULL,
                 paused_at = NULL,
                 blocker_reason = NULL,
                 current_task_id = CASE WHEN ?7 THEN NULL ELSE current_task_id END,
                 next_action = ?5,
                 updated_at = ?6
             WHERE card_id = ?1 AND revision = ?2",
            params![
                card_id,
                expected_revision,
                next_revision,
                target,
                next_action,
                now,
                clear_current_task
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_on_return_for_revision",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision,
            actual_revision: existing.revision,
        });
    }
    if clear_current_task {
        reset_in_progress_tasks_to_todo(conn, card_id, now)?;
    }

    let reason_tag = format!("returned_for_revision:{reason}");
    match target {
        "spec" => {
            stale_approvals_for_gates(
                conn,
                card_id,
                &["spec", "plan", "delivery"],
                &reason_tag,
                now,
            )?;
            stale_active_challenges(conn, card_id, now)?;
        }
        "plan" => {
            stale_approvals_for_gates(conn, card_id, &["plan", "delivery"], &reason_tag, now)?;
        }
        _ => {
            stale_approvals_for_gates(conn, card_id, &["delivery"], &reason_tag, now)?;
        }
    }
    stale_downstream_evidence(conn, card_id, &reason_tag, now)?;
    restore_remaining_blocker_on_return(conn, card_id, target)?;

    let payload_json = serde_json::to_string(&json!({
        "fromStage": existing.stage,
        "targetStage": target,
        "reason": reason,
    }))
    .map_err(|source| WorkflowBoardRepoError::Json {
        operation: "serialize_return_for_revision_payload",
        source,
    })?;
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: target,
            kind: "returned_for_revision",
            summary: &format!("Returned to {target}: {reason}"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}

/// Returns a card to an earlier stage (`spec`, `plan`, or `execute`), preserving all history and
/// marking downstream approvals, active challenges, and evaluated evidence `'stale'`.
pub fn return_for_revision(
    conn: &Connection,
    input: &ReturnForRevisionInput<'_>,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let target = require_non_empty(input.target_stage, "target_stage")?;
    let reason = require_non_empty(input.reason, "reason")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if input.expected_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected_revision must be >= 1".to_owned(),
        ));
    }
    let Some(target_rank) = stage_rank(target) else {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "invalid target_stage `{target}`"
        )));
    };
    if target == "ship" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(
            "cannot return_for_revision to `ship`".to_owned(),
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
    let current_rank = stage_rank(&existing.stage).unwrap_or(0);
    if target_rank >= current_rank {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "target_stage `{target}` must be strictly earlier than current stage `{}`",
            existing.stage
        )));
    }

    persist_return_and_invalidate(
        conn,
        &existing,
        target,
        reason,
        revision_id,
        input.expected_revision,
        now,
    )?;

    let detail = load_card_planning_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

/// Marks active `'valid'` or `'historical'` `'plan'` and `'delivery'` approvals and evaluated
/// evidence `'stale'` while preserving any active `'spec'` approval (used on external
/// `plan.md` / `design.md` edits).
pub fn invalidate_plan_approval(
    conn: &Connection,
    card_id: &str,
    reason: &str,
    now_iso: &str,
) -> Result<bool, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let reason = require_non_empty(reason, "reason")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let _card = load_card_row(conn, card_id)?;
    let updated = stale_approvals_for_gates(conn, card_id, &["plan", "delivery"], reason, now)?;
    stale_downstream_evidence(conn, card_id, reason, now)?;
    tx.commit()?;
    Ok(updated > 0)
}
