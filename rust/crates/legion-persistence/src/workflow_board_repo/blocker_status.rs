//! Unified orthogonal blocker computation and paused-safe card status updates.

use rusqlite::{Connection, params};

use super::WorkflowBoardRepoError;
use super::types::CardRow;

pub(crate) struct CardBlockerStatus {
    pub is_blocked: bool,
    pub blocker_reason: Option<String>,
    pub default_next_action: &'static str,
    pub has_high_priority_blocker: bool,
}

fn count_rows(
    conn: &Connection,
    sql: &str,
    card_id: &str,
    operation: &'static str,
) -> Result<i64, WorkflowBoardRepoError> {
    conn.query_row(sql, params![card_id], |r| r.get(0))
        .map_err(|source| WorkflowBoardRepoError::Sqlite { operation, source })
}

/// Evaluates all orthogonal M3 blockers across violations, interrupted actions, failed checks,
/// unresolved/fix findings on active review runs, failed review runs, and failed/blocked tasks.
pub(crate) fn compute_card_blocker_status(
    conn: &Connection,
    card_id: &str,
) -> Result<CardBlockerStatus, WorkflowBoardRepoError> {
    let open_violations = count_rows(
        conn,
        "SELECT COUNT(*) FROM workflow_board_contract_violations
         WHERE card_id = ?1 AND status = 'violation_pending'",
        card_id,
        "count_open_violations_blocker",
    )?;
    let interrupted_actions = count_rows(
        conn,
        "SELECT COUNT(*) FROM workflow_board_external_actions
         WHERE card_id = ?1 AND status = 'interrupted'",
        card_id,
        "count_interrupted_actions_blocker",
    )?;
    let failed_checks = count_rows(
        conn,
        "SELECT COUNT(*) FROM workflow_board_planned_checks
         WHERE card_id = ?1 AND status = 'failed'",
        card_id,
        "count_failed_checks_blocker",
    )?;
    let unresolved_findings = count_rows(
        conn,
        "SELECT COUNT(*)
         FROM workflow_board_review_findings f
         JOIN workflow_board_review_runs r ON r.id = f.review_run_id
         WHERE f.card_id = ?1
           AND r.status NOT IN ('historical', 'stale')
           AND (f.status = 'open' OR (f.status = 'resolved' AND f.disposition = 'fix'))",
        card_id,
        "count_unresolved_findings_blocker",
    )?;
    let failed_reviews = count_rows(
        conn,
        "SELECT COUNT(*) FROM workflow_board_review_runs
         WHERE card_id = ?1 AND status = 'failed'",
        card_id,
        "count_failed_reviews_blocker",
    )?;
    let blocked_tasks = count_rows(
        conn,
        "SELECT COUNT(*) FROM workflow_board_tasks
         WHERE card_id = ?1 AND status IN ('failed', 'blocked')",
        card_id,
        "count_blocked_tasks_blocker",
    )?;

    let has_high_priority_blocker =
        open_violations > 0 || interrupted_actions > 0 || failed_checks > 0;
    let (blocker_reason, default_next_action) = if open_violations > 0 {
        (
            Some("Unresolved file-contract violation exists".to_owned()),
            "resolve_contract_violation",
        )
    } else if interrupted_actions > 0 {
        (
            Some("Interrupted external action requires reconciliation".to_owned()),
            "reconcile_external_action",
        )
    } else if failed_checks > 0 {
        (Some("Planned check failed".to_owned()), "retry_check")
    } else if unresolved_findings > 0 {
        (
            Some(format!(
                "{unresolved_findings} unresolved review finding(s)"
            )),
            "resolve_finding",
        )
    } else if failed_reviews > 0 {
        (Some("Independent review failed".to_owned()), "retry_review")
    } else if blocked_tasks > 0 {
        (
            Some("Task execution failed or blocked".to_owned()),
            "retry_task",
        )
    } else {
        (None, "continue_execute")
    };

    Ok(CardBlockerStatus {
        is_blocked: blocker_reason.is_some(),
        blocker_reason,
        default_next_action,
        has_high_priority_blocker,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Applies unified blocker status recomputation with paused/shipped guards in one UPDATE"
)]
pub(crate) fn recompute_card_blocker_status(
    conn: &Connection,
    existing: &CardRow,
    next_revision: i64,
    next_action_if_clear: &str,
    override_blocker_reason: Option<&str>,
    override_blocked_next_action: Option<&str>,
    product_snapshot_digest: Option<&str>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let blocker = compute_card_blocker_status(conn, &existing.card_id)?;
    let next_blocker_reason = if blocker.is_blocked {
        override_blocker_reason
            .map(str::to_owned)
            .or(blocker.blocker_reason)
    } else {
        None
    };

    if existing.status == "paused" {
        let paused_from = if blocker.is_blocked {
            "blocked"
        } else {
            "active"
        };
        conn.execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 paused_from_status = ?4,
                 blocker_reason = ?5,
                 product_snapshot_digest = COALESCE(?6, product_snapshot_digest),
                 updated_at = ?7
             WHERE card_id = ?1 AND revision = ?2",
            params![
                existing.card_id,
                existing.revision,
                next_revision,
                paused_from,
                next_blocker_reason,
                product_snapshot_digest,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "recompute_paused_card_blocker_status",
            source,
        })?;
        return Ok(());
    }

    let next_status = if blocker.is_blocked {
        "blocked"
    } else {
        "active"
    };
    let next_action = if blocker.is_blocked {
        override_blocked_next_action.unwrap_or(blocker.default_next_action)
    } else {
        next_action_if_clear
    };
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?3,
             status = ?4,
             blocker_reason = ?5,
             next_action = ?6,
             product_snapshot_digest = COALESCE(?7, product_snapshot_digest),
             updated_at = ?8
         WHERE card_id = ?1 AND revision = ?2",
        params![
            existing.card_id,
            existing.revision,
            next_revision,
            next_status,
            next_blocker_reason,
            next_action,
            product_snapshot_digest,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "recompute_card_blocker_status",
        source,
    })?;
    Ok(())
}
