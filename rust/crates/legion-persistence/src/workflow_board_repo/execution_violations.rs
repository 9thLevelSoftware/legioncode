//! M3 post-turn file-contract violation persistence and resolution inside CAS `WriteTx`.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;
use uuid::Uuid;

use super::blocker_status::recompute_card_blocker_status;
use super::cards::load_card_row;
use super::execution_queries::load_card_execution_detail_inner;
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m2::TaskContractRow;
use super::types_m3::{CardExecutionDetail, CompleteTaskInput, ResolveContractViolationInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

pub(crate) fn validate_task_dag_ready(
    tasks: &[TaskContractRow],
    target_task_id: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let by_id: HashMap<&str, &TaskContractRow> =
        tasks.iter().map(|t| (t.task_id.as_str(), t)).collect();
    let Some(target) = by_id.get(target_task_id).copied() else {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "task `{target_task_id}` does not exist on card"
        )));
    };
    let mut deps: HashSet<String> = serde_json::from_str::<Vec<String>>(&target.blocked_by_json)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_owned())
        .collect();
    for other in tasks {
        if other.task_id == target_task_id {
            continue;
        }
        let blocks: Vec<String> = serde_json::from_str(&other.blocks_json).unwrap_or_default();
        if blocks.iter().any(|b| b.trim() == target_task_id) {
            deps.insert(other.task_id.clone());
        }
    }
    for dep_id in deps {
        let Some(dep) = by_id.get(dep_id.as_str()).copied() else {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "task `{target_task_id}` depends on unknown task `{dep_id}`"
            )));
        };
        if !matches!(dep.status.as_str(), "completed" | "skipped") {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "task `{target_task_id}` is blocked by incomplete dependency `{dep_id}` (`{}`)",
                dep.status
            )));
        }
    }
    Ok(())
}

pub(crate) fn insert_contract_violation(
    conn: &Connection,
    input: &CompleteTaskInput<'_>,
    allowed_json: &str,
    blocker_msg: Option<&str>,
    card_revision: i64,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let vid = input
        .violation_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("viol-{}", Uuid::new_v4()), str::to_owned);
    conn.execute(
        "INSERT INTO workflow_board_contract_violations (
           id, card_id, task_id, stage_run_id, thread_id, workflow_run_id,
           card_revision, status, changed_files_json, allowed_files_json,
           unexpected_files_json, forbidden_files_json, preserved_in_worktree,
           blocking_reason, diff_digest, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'violation_pending', ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14, ?14)",
        params![
            vid,
            input.card_id.trim(),
            input.task_id.trim(),
            input.stage_run_id,
            input.thread_id,
            input.workflow_run_id,
            card_revision,
            serde_json::to_string(input.changed_files).unwrap_or_else(|_| "[]".to_owned()),
            allowed_json,
            serde_json::to_string(input.unexpected_files).unwrap_or_else(|_| "[]".to_owned()),
            serde_json::to_string(input.forbidden_files).unwrap_or_else(|_| "[]".to_owned()),
            blocker_msg,
            input.diff_digest,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_contract_violation",
        source,
    })?;
    Ok(())
}

/// Resolves an open post-turn file-contract violation and unblocks the task/card when clear.
pub fn resolve_contract_violation(
    conn: &Connection,
    input: &ResolveContractViolationInput<'_>,
    now_iso: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let _revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _violation_id = require_non_empty(input.violation_id, "violation_id")?;
    let disposition = require_non_empty(input.disposition, "disposition")?;
    let _resolved_by = require_non_empty(input.resolved_by, "resolved_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if !matches!(
        disposition,
        "amend_plan" | "accept_scope" | "manual_reverted" | "dismiss"
    ) {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported violation disposition `{disposition}`"
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
    if matches!(existing.status.as_str(), "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot resolve contract violation while card status is `{}`",
            existing.status
        )));
    }
    apply_violation_resolution_updates(conn, &existing, input, now)?;

    let detail = load_card_execution_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn apply_violation_resolution_updates(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &ResolveContractViolationInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let violation_id = input.violation_id.trim();
    let disposition = input.disposition.trim();
    let resolved_by = input.resolved_by.trim();
    let task_id: String = conn
        .query_row(
            "SELECT task_id FROM workflow_board_contract_violations WHERE id = ?1 AND card_id = ?2",
            params![violation_id, card_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "find_contract_violation",
            source,
        })?
        .ok_or_else(|| {
            WorkflowBoardRepoError::PreconditionFailed(format!(
                "contract violation `{violation_id}` not found on card `{card_id}`"
            ))
        })?;
    let next_status = if disposition == "amend_plan" {
        "amended"
    } else {
        "resolved"
    };
    conn.execute(
        "UPDATE workflow_board_contract_violations
         SET status = ?3,
             resolution_disposition = ?4,
             resolution_note = ?5,
             resolved_by = ?6,
             resolved_at = ?7,
             updated_at = ?7
         WHERE id = ?1 AND card_id = ?2",
        params![
            violation_id,
            card_id,
            next_status,
            disposition,
            input.note,
            resolved_by,
            now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_contract_violation",
        source,
    })?;

    let resolved_task_status = if disposition == "manual_reverted" {
        "todo"
    } else {
        "completed"
    };
    conn.execute(
        "UPDATE workflow_board_tasks
         SET status = ?3,
             blocker_reason = NULL,
             updated_at = ?4
         WHERE card_id = ?1 AND task_id = ?2 AND status = 'blocked'",
        params![card_id, task_id, resolved_task_status, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "unblock_task_after_violation",
        source,
    })?;

    finalize_violation_card_and_ledger(conn, existing, input, now)
}

fn finalize_violation_card_and_ledger(
    conn: &Connection,
    existing: &super::types::CardRow,
    input: &ResolveContractViolationInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = input.card_id.trim();
    let revision_id = input.revision_id.trim();
    let violation_id = input.violation_id.trim();
    let disposition = input.disposition.trim();

    let next_revision = existing.revision + 1;
    recompute_card_blocker_status(
        conn,
        existing,
        next_revision,
        "continue_execute",
        None,
        None,
        None,
        now,
    )?;

    let payload_json =
        json!({ "violationId": violation_id, "disposition": disposition }).to_string();
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "contract_violation_resolved",
            summary: &format!("Resolved contract violation {violation_id} ({disposition})"),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: &payload_json,
            now,
        },
    )?;
    Ok(())
}
