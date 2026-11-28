//! M3 Execute task completion, post-turn file-contract violation recording, and snapshot invalidation.

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::blocker_status::compute_card_blocker_status;
use super::cards::load_card_row;
use super::execution_checks::invalidate_snapshot_bound_evidence;
use super::execution_queries::load_card_execution_detail_inner;
use super::execution_violations::insert_contract_violation;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m3::{CardExecutionDetail, CompleteTaskInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Records task completion/failure or post-turn file-contract violation (`preserved_in_worktree = 1`).
pub fn complete_task(
    conn: &Connection,
    input: &CompleteTaskInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let _revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let task_id = require_non_empty(input.task_id, "task_id")?;
    let outcome = require_non_empty(input.outcome_status, "outcome_status")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(outcome, "completed" | "failed" | "blocked") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported task outcome_status `{outcome}`"
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
    if existing.stage != "execute" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot complete task in stage `{}` (must be `execute`)",
            existing.stage
        )));
    }
    if !matches!(existing.status.as_str(), "active" | "blocked") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot complete task while card status is `{}`",
            existing.status
        )));
    }
    let (task_status, allowed_json): (String, String) = conn
        .query_row(
            "SELECT status, files_allowed_json FROM workflow_board_tasks WHERE card_id = ?1 AND task_id = ?2",
            params![card_id, task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_task_allowed_files",
            source,
        })?
        .ok_or_else(|| {
            WorkflowBoardRepoError::PreconditionFailed(format!("task `{task_id}` not found"))
        })?;
    if task_status != "in_progress" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot complete task `{task_id}` with status `{task_status}` (must be `in_progress`)"
        )));
    }

    let has_violation = !input.unexpected_files.is_empty() || !input.forbidden_files.is_empty();
    let final_task_status = if has_violation { "blocked" } else { outcome };
    let blocker_msg = if has_violation {
        Some(format!(
            "Task {task_id} touched {} unexpected / {} forbidden file(s); edits preserved in worktree",
            input.unexpected_files.len(),
            input.forbidden_files.len()
        ))
    } else if outcome == "failed" || outcome == "blocked" {
        Some(
            input
                .error_message
                .unwrap_or("Task execution failed")
                .to_owned(),
        )
    } else {
        None
    };

    if has_violation {
        insert_contract_violation(
            conn,
            input,
            &allowed_json,
            blocker_msg.as_deref(),
            existing.revision + 1,
            now,
        )?;
    }
    persist_task_completion_rows(
        conn,
        &existing,
        input,
        final_task_status,
        blocker_msg.as_deref(),
        now,
    )?;
    finalize_task_completion_card(
        conn,
        &existing,
        input,
        final_task_status,
        blocker_msg.as_deref(),
        has_violation,
        now,
    )?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn persist_task_completion_rows(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &CompleteTaskInput<'_>,
    final_task_status: &str,
    blocker_msg: Option<&str>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let task_id = input.task_id.trim();
    conn.execute(
        "UPDATE workflow_board_tasks
         SET status = ?3,
             completed_at = CASE WHEN ?3 = 'completed' THEN ?4 ELSE completed_at END,
             blocker_reason = ?5,
             last_error = ?6,
             last_diff_digest = COALESCE(?7, last_diff_digest),
             updated_at = ?4
         WHERE card_id = ?1 AND task_id = ?2",
        params![
            card_id,
            task_id,
            final_task_status,
            now,
            blocker_msg,
            input.error_message,
            input.diff_digest
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "complete_task_update",
        source,
    })?;

    if let Some(new_snap) = input
        .product_snapshot_digest
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && existing.product_snapshot_digest.as_deref() != Some(new_snap)
    {
        invalidate_snapshot_bound_evidence(
            conn,
            card_id,
            new_snap,
            "product_snapshot_changed",
            now,
        )?;
    }
    Ok(())
}

fn finalize_task_completion_card(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &CompleteTaskInput<'_>,
    final_task_status: &str,
    blocker_msg: Option<&str>,
    has_violation: bool,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let task_id = input.task_id.trim();
    let revision_id = input.revision_id.trim();
    let next_revision = existing.revision + 1;
    let (next_card_status, effective_blocker, next_action) = if final_task_status == "completed" {
        let blocker = compute_card_blocker_status(conn, card_id)?;
        if blocker.is_blocked {
            (
                "blocked",
                blocker.blocker_reason,
                blocker.default_next_action,
            )
        } else {
            ("active", None, "run_checks_or_next_task")
        }
    } else if has_violation {
        (
            "blocked",
            blocker_msg.map(str::to_owned),
            "resolve_contract_violation",
        )
    } else {
        ("blocked", blocker_msg.map(str::to_owned), "retry_step")
    };
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             status = ?4,
             current_task_id = CASE WHEN ?9 THEN NULL ELSE current_task_id END,
             blocker_reason = ?5,
             next_action = ?6,
             product_snapshot_digest = COALESCE(?7, product_snapshot_digest),
             updated_at = ?8
         WHERE card_id = ?1 AND revision = ?2",
        params![
            card_id,
            input.expected_revision,
            next_revision,
            next_card_status,
            effective_blocker,
            next_action,
            input.product_snapshot_digest,
            now,
            final_task_status == "completed",
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "complete_task_update_card",
        source,
    })?;

    let kind = if has_violation {
        "contract_violation_detected"
    } else {
        "task_completed"
    };
    let payload_json = json!({
        "taskId": task_id,
        "outcome": final_task_status,
        "hasViolation": has_violation,
    })
    .to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: "execute",
            kind,
            summary: &format!("Task {task_id} -> {final_task_status}"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )
}
