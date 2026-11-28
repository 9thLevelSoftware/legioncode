//! Runtime metadata synchronization helpers for `workflow_board_cards` (`worktree_path`,
//! `external_edit_state`, `product_snapshot_digest`) and `approvePlan` state preview.

use rusqlite::{Connection, OptionalExtension as _, params};

use super::blocker_status::compute_card_blocker_status;
use super::invalidation::{
    TaskDigestFields, compute_aggregate_tasks_digest, compute_task_contract_digest,
};
use super::types_m2::CardPlanningDetail;
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Backfills `workflow_board_cards.worktree_path` and `branch` when currently unset.
///
/// Returns `Ok(true)` if the row was updated, or `Ok(false)` if a worktree path was already set.
pub fn set_card_worktree_if_unset(
    conn: &Connection,
    card_id: &str,
    worktree_path: &str,
    branch: Option<&str>,
    now_iso: &str,
) -> Result<bool, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let wt = require_non_empty(worktree_path, "worktree_path")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    let clean_branch = branch.map(str::trim).filter(|s| !s.is_empty());

    let tx = WriteTx::begin(conn)?;
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET worktree_path = ?2,
                 branch = COALESCE(?3, branch),
                 updated_at = ?4
             WHERE card_id = ?1
               AND (worktree_path IS NULL OR TRIM(worktree_path) = '')",
            params![card_id, wt, clean_branch, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "set_card_worktree_if_unset",
            source,
        })?;
    tx.commit()?;
    Ok(updated > 0)
}

/// Persists `external_edit_state` and `external_edit_detail_json` onto `workflow_board_cards`.
pub fn update_card_external_edit_state(
    conn: &Connection,
    card_id: &str,
    state: &str,
    detail_json: Option<&str>,
    now_iso: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let state = require_non_empty(state, "state")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET external_edit_state = ?2,
                 external_edit_detail_json = ?3,
                 updated_at = ?4
             WHERE card_id = ?1",
            params![card_id, state, detail_json, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_external_edit_state",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::CardNotFound {
            card_id: card_id.to_owned(),
        });
    }
    tx.commit()?;
    Ok(())
}

/// Persists `product_snapshot_digest` onto `workflow_board_cards`.
pub fn update_card_product_snapshot_digest(
    conn: &Connection,
    card_id: &str,
    snapshot_digest: &str,
    now_iso: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let snap = require_non_empty(snapshot_digest, "snapshot_digest")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET product_snapshot_digest = ?2,
                 updated_at = ?3
             WHERE card_id = ?1",
            params![card_id, snap, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_product_snapshot_digest",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::CardNotFound {
            card_id: card_id.to_owned(),
        });
    }
    tx.commit()?;
    Ok(())
}

/// Preview of the post-`approve_plan` card fields (`status`, `blocker_reason`, `next_action`, `tasks_digest`)
/// so journaled `card.json` writes match SQLite atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanApprovalCardPreview {
    pub status: String,
    pub blocker_reason: Option<String>,
    pub next_action: String,
    pub tasks_digest: String,
}

/// Computes the exact `status`, `blocker_reason`, `next_action`, and `tasks_digest` that
/// [`super::approve_plan`] will persist for `planning`.
pub fn preview_plan_approval_card_state(
    conn: &Connection,
    planning: &CardPlanningDetail,
) -> Result<PlanApprovalCardPreview, WorkflowBoardRepoError> {
    let task_contract_digests: Vec<String> = planning
        .tasks
        .iter()
        .map(|t| {
            compute_task_contract_digest(&TaskDigestFields {
                task_id: &t.task_id,
                title: &t.title,
                blocked_by_json: &t.blocked_by_json,
                blocks_json: &t.blocks_json,
                files_allowed_json: &t.files_allowed_json,
                files_forbidden_json: &t.files_forbidden_json,
                expected_artifacts_json: &t.expected_artifacts_json,
                verification_commands_json: &t.verification_commands_json,
                testing_method: &t.testing_method,
            })
        })
        .collect();
    let tasks_digest =
        compute_aggregate_tasks_digest(task_contract_digests.iter().map(String::as_str));
    let existing = &planning.card_detail.card;
    let next_stage = if existing.stage == "plan" {
        "execute"
    } else {
        existing.stage.as_str()
    };
    let (status, blocker_reason, next_action) = if next_stage == "execute" {
        let blocker = compute_card_blocker_status(conn, &existing.card_id)?;
        if blocker.is_blocked {
            (
                "blocked".to_owned(),
                blocker.blocker_reason,
                blocker.default_next_action.to_owned(),
            )
        } else {
            ("active".to_owned(), None, "start_execute".to_owned())
        }
    } else {
        (
            existing.status.clone(),
            existing.blocker_reason.clone(),
            "start_execute".to_owned(),
        )
    };
    Ok(PlanApprovalCardPreview {
        status,
        blocker_reason,
        next_action,
        tasks_digest,
    })
}

/// Returns `true` when `card_id` currently has a blocker relevant to `target_stage` (mirroring
/// `restore_remaining_blocker_on_return`: all orthogonal blockers for `execute`, interrupted
/// external actions for `spec`/`plan`).
pub fn card_has_remaining_blocker(
    conn: &Connection,
    card_id: &str,
    target_stage: &str,
) -> Result<bool, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM workflow_board_cards WHERE card_id = ?1 LIMIT 1",
            params![card_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "check_card_exists_for_blocker",
            source,
        })?;
    if exists.is_none() {
        return Err(WorkflowBoardRepoError::CardNotFound {
            card_id: card_id.to_owned(),
        });
    }
    if target_stage == "execute" {
        Ok(compute_card_blocker_status(conn, card_id)?.is_blocked)
    } else {
        let interrupted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workflow_board_external_actions
                 WHERE card_id = ?1 AND status = 'interrupted'",
                params![card_id],
                |r| r.get(0),
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "count_interrupted_actions_for_blocker",
                source,
            })?;
        Ok(interrupted > 0)
    }
}
