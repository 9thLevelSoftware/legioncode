//! M3 Execute task claim/start inside a strict CAS `WriteTx`.

use rusqlite::{Connection, params};
use serde_json::json;

use super::cli_import_helpers::select_cli_import_by_card;
use super::execution_queries::load_card_execution_detail_inner;
pub use super::execution_task_complete::complete_task;
use super::execution_violations::validate_task_dag_ready;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::planning_queries::load_card_planning_detail_inner;
use super::types::ApprovalRow;
use super::types_m2::CardPlanningDetail;
use super::types_m3::{CardExecutionDetail, ClaimOrStartTaskInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Returns the effective Plan approval row for Execute (`"valid"`, or `"historical"` only for
/// CLI-imported cards when no `"stale"` / `"superseded"` Plan approval is present).
#[must_use]
pub fn effective_execute_plan_approval(
    is_cli_imported: bool,
    approvals: &[ApprovalRow],
) -> Option<&ApprovalRow> {
    if let Some(valid) = approvals
        .iter()
        .find(|a| a.gate == "plan" && a.status == "valid")
    {
        return Some(valid);
    }
    if !is_cli_imported {
        return None;
    }
    if approvals
        .iter()
        .any(|a| a.gate == "plan" && matches!(a.status.as_str(), "stale" | "superseded"))
    {
        return None;
    }
    approvals
        .iter()
        .find(|a| a.gate == "plan" && a.status == "historical")
}

/// Validates all Execute task-claim preconditions against persisted SQLite state.
pub fn verify_task_claim_preconditions(
    conn: &Connection,
    card_id: &str,
    expected_revision: i64,
    task_id: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    let planning = load_card_planning_detail_inner(conn, card_id)?;
    let existing = &planning.card_detail.card;
    if existing.revision != expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision,
            actual_revision: existing.revision,
        });
    }
    if existing.archived_at.is_some() {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(
            "cannot start task on an archived card".to_owned(),
        ));
    }
    if existing.stage != "execute" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot start task in stage `{}` (must be `execute`)",
            existing.stage
        )));
    }
    if !matches!(existing.status.as_str(), "active" | "blocked") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot start task while card status is `{}`",
            existing.status
        )));
    }
    let is_cli_imported = select_cli_import_by_card(conn, card_id)?.is_some();
    if effective_execute_plan_approval(is_cli_imported, &planning.card_detail.approvals).is_none() {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "cannot execute tasks without a valid Plan approval".to_owned(),
        ));
    }
    let open_violations: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_contract_violations
             WHERE card_id = ?1 AND status = 'violation_pending'",
            params![card_id],
            |r| r.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_open_violations",
            source,
        })?;
    if open_violations > 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "contract conflict: unresolved file-contract violation exists".to_owned(),
        ));
    }
    let interrupted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_external_actions
             WHERE card_id = ?1 AND status = 'interrupted'",
            params![card_id],
            |r| r.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_interrupted_actions",
            source,
        })?;
    if interrupted > 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "interrupted external action must be reconciled before starting a task".to_owned(),
        ));
    }
    if let Some(active_other) = planning
        .tasks
        .iter()
        .find(|t| t.status == "in_progress" && t.task_id != task_id)
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "contract conflict: task `{}` is already in_progress (single mutating task per card)",
            active_other.task_id
        )));
    }
    validate_task_dag_ready(&planning.tasks, task_id)?;
    Ok(planning)
}

/// Claims or starts a single dependency-ready task on an Execute card inside a strict CAS `WriteTx`.
pub fn claim_or_start_task(
    conn: &Connection,
    input: &ClaimOrStartTaskInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let task_id = require_non_empty(input.task_id, "task_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if input.expected_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected_revision must be >= 1".to_owned(),
        ));
    }

    let tx = WriteTx::begin(conn)?;
    let planning =
        verify_task_claim_preconditions(conn, card_id, input.expected_revision, task_id)?;
    let existing = &planning.card_detail.card;

    let next_revision = existing.revision + 1;
    let updated_task = conn
        .execute(
            "UPDATE workflow_board_tasks
             SET status = 'in_progress',
                 active_stage_run_id = COALESCE(?3, active_stage_run_id),
                 started_at = COALESCE(started_at, ?4),
                 attempt_count = attempt_count + 1,
                 blocker_reason = NULL,
                 last_error = NULL,
                 updated_at = ?4
             WHERE card_id = ?1 AND task_id = ?2",
            params![card_id, task_id, input.stage_run_id, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "claim_task_update",
            source,
        })?;
    if updated_task == 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "task `{task_id}` not found"
        )));
    }

    let updated_card = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 status = 'active',
                 current_task_id = ?4,
                 blocker_reason = NULL,
                 next_action = 'complete_task',
                 updated_at = ?5
             WHERE card_id = ?1 AND revision = ?2",
            params![
                card_id,
                input.expected_revision,
                next_revision,
                task_id,
                now
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "claim_task_update_card",
            source,
        })?;
    if updated_card == 0 {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }

    let payload_json = json!({ "taskId": task_id, "stageRunId": input.stage_run_id }).to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: "execute",
            kind: "task_started",
            summary: &format!("Started task {task_id}"),
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
