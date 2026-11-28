//! Retry target reset helpers for [`super::execution_lifecycle::retry_step`].

use rusqlite::{Connection, OptionalExtension as _, params};

use super::WorkflowBoardRepoError;

pub(crate) fn reset_retry_target(
    conn: &Connection,
    card_id: &str,
    step_kind: &str,
    target_id: Option<&str>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    match step_kind {
        "task" => reset_retry_task(conn, card_id, target_id.map_or("", str::trim), now),
        "check" => reset_retry_check(conn, card_id, target_id.map_or("", str::trim), now),
        "review" => reset_retry_review(conn, card_id, now),
        "stage" => reset_retry_stage(conn, card_id, now),
        _ => Ok(()),
    }
}

fn reset_retry_task(
    conn: &Connection,
    card_id: &str,
    tid: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let task_status: Option<String> = conn
        .query_row(
            "SELECT status FROM workflow_board_tasks WHERE card_id = ?1 AND task_id = ?2",
            params![card_id, tid],
            |r| r.get(0),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_task_status_for_retry",
            source,
        })?;
    let Some(status) = task_status else {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "task `{tid}` not found"
        )));
    };
    if !matches!(status.as_str(), "failed" | "blocked") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot retry task `{tid}` with status `{status}` (must be `failed` or `blocked`)"
        )));
    }
    let open_violations: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_contract_violations
             WHERE card_id = ?1 AND task_id = ?2 AND status = 'violation_pending'",
            params![card_id, tid],
            |r| r.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_open_task_violations_for_retry",
            source,
        })?;
    if open_violations > 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "contract conflict: task `{tid}` has an open contract violation that must be resolved before retry"
        )));
    }

    conn.execute(
        "UPDATE workflow_board_tasks
         SET status = 'todo',
             active_stage_run_id = NULL,
             started_at = NULL,
             completed_at = NULL,
             blocker_reason = NULL,
             last_error = NULL,
             updated_at = ?3
         WHERE card_id = ?1 AND task_id = ?2",
        params![card_id, tid, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_task",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_cards
         SET current_task_id = NULL,
             updated_at = ?3
         WHERE card_id = ?1 AND current_task_id = ?2",
        params![card_id, tid, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_clear_current_task",
        source,
    })?;
    Ok(())
}

fn reset_retry_check(
    conn: &Connection,
    card_id: &str,
    cid: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let updated = conn
        .execute(
            "UPDATE workflow_board_planned_checks
             SET status = 'pending',
                 stale_reason = NULL,
                 updated_at = ?3
             WHERE card_id = ?1 AND check_id = ?2",
            params![card_id, cid, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "retry_step_check",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "planned check `{cid}` not found"
        )));
    }
    Ok(())
}

fn reset_retry_review(
    conn: &Connection,
    card_id: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_review_runs
         SET status = 'historical',
             updated_at = ?2
         WHERE card_id = ?1 AND status != 'historical'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_review_runs",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_review_findings
         SET status = 'historical',
             updated_at = ?2
         WHERE card_id = ?1 AND status != 'historical'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_review_findings",
        source,
    })?;
    Ok(())
}

fn reset_retry_stage(
    conn: &Connection,
    card_id: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_cards
         SET current_task_id = NULL,
             updated_at = ?2
         WHERE card_id = ?1",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_stage_clear_task",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_tasks
         SET status = 'todo',
             active_stage_run_id = NULL,
             blocker_reason = NULL,
             updated_at = ?2
         WHERE card_id = ?1 AND status = 'in_progress'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_stage_reset_tasks",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_stage_runs
         SET status = 'cancelled',
             ended_at = COALESCE(ended_at, ?2),
             updated_at = ?2
         WHERE card_id = ?1 AND status IN ('pending', 'launching', 'running', 'paused')",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "retry_step_stage_cancel_runs",
        source,
    })?;
    Ok(())
}
