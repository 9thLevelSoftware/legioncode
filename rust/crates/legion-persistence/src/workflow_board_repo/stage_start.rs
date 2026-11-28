//! Stage-start optimistic CAS and reusable stage-run lookup helpers.

use rusqlite::{Connection, OptionalExtension as _, params};
use uuid::Uuid;

use super::cards::{load_card_detail_inner, load_card_row};
use super::stage_runs::{map_stage_run_row, validate_stage};
use super::{CardDetail, StageRunRow, WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Returns the next monotonic attempt number (`MAX(attempt) + 1`, starting at `1`) for `(card_id, stage)`.
pub fn next_stage_attempt(
    conn: &Connection,
    card_id: &str,
    stage: &str,
) -> Result<i64, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let stage = validate_stage(stage)?;
    let max_attempt: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(attempt), 0) FROM workflow_board_stage_runs
             WHERE card_id = ?1 AND stage = ?2",
            params![card_id, stage],
            |row| row.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "next_stage_attempt",
            source,
        })?;
    Ok(max_attempt + 1)
}

/// Finds the latest non-terminal stage run for `(card_id, stage)`, marking it `'cancelled'`
/// if its linked `workflow_runs` row was cancelled externally (`workflows.cancel`).
pub fn find_reusable_stage_run(
    conn: &Connection,
    card_id: &str,
    stage: &str,
) -> Result<Option<StageRunRow>, WorkflowBoardRepoError> {
    find_reusable_stage_run_for_task(conn, card_id, stage, None)
}

/// Finds the latest non-terminal stage run for `(card_id, stage, task_id)`, marking it `'cancelled'`
/// if its linked `workflow_runs` row was cancelled externally (`workflows.cancel`).
pub fn find_reusable_stage_run_for_task(
    conn: &Connection,
    card_id: &str,
    stage: &str,
    task_id: Option<&str>,
) -> Result<Option<StageRunRow>, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let stage = validate_stage(stage)?;
    let task_id = task_id
        .map(|t| require_non_empty(t, "task_id"))
        .transpose()?;
    let sr = match task_id {
        Some(tid) => conn
            .query_row(
                "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                        card_revision, provider_instance_id, model, model_options_json,
                        runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                        branch, thread_id, workflow_run_id, brief_digest, result_json,
                        last_error, created_at, updated_at, ended_at
                 FROM workflow_board_stage_runs
                 WHERE card_id = ?1 AND stage = ?2 AND task_id = ?3
                 ORDER BY attempt DESC, created_at DESC, id DESC
                 LIMIT 1",
                params![card_id, stage, tid],
                map_stage_run_row,
            )
            .optional(),
        None => conn
            .query_row(
                "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                        card_revision, provider_instance_id, model, model_options_json,
                        runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                        branch, thread_id, workflow_run_id, brief_digest, result_json,
                        last_error, created_at, updated_at, ended_at
                 FROM workflow_board_stage_runs
                 WHERE card_id = ?1 AND stage = ?2 AND task_id IS NULL
                 ORDER BY attempt DESC, created_at DESC, id DESC
                 LIMIT 1",
                params![card_id, stage],
                map_stage_run_row,
            )
            .optional(),
    }
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "find_reusable_stage_run",
        source,
    })?;
    let Some(sr) = sr else {
        return Ok(None);
    };

    if matches!(sr.status.as_str(), "failed" | "cancelled" | "completed") {
        return Ok(None);
    }

    if let Some(ref wf_run_id) = sr.workflow_run_id {
        let has_wf_runs: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'workflow_runs')",
                [],
                |row| row.get(0),
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "check_workflow_runs_table",
                source,
            })?;
        if has_wf_runs {
            let wf_status: Option<String> = conn
                .query_row(
                    "SELECT status FROM workflow_runs WHERE run_id = ?1",
                    params![wf_run_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|source| WorkflowBoardRepoError::Sqlite {
                    operation: "check_workflow_run_status",
                    source,
                })?;
            if wf_status.as_deref() == Some("cancelled") {
                let tx = WriteTx::begin(conn)?;
                conn.execute(
                    "UPDATE workflow_board_stage_runs
                     SET status = 'cancelled',
                         ended_at = COALESCE(ended_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                    params![sr.id],
                )
                .map_err(|source| WorkflowBoardRepoError::Sqlite {
                    operation: "mark_stage_run_cancelled",
                    source,
                })?;
                tx.commit()?;
                return Ok(None);
            }
        }
    }

    Ok(Some(sr))
}

/// Atomically increments `revision = expected_revision + 1`, updates `next_action`, and appends a `'stage_started'` revision row.
pub fn record_stage_started_cas(
    conn: &Connection,
    card_id: &str,
    expected_revision: i64,
    next_action: &str,
    actor: &str,
    summary: &str,
    now_iso: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let next_action = require_non_empty(next_action, "next_action")?;
    let actor = require_non_empty(actor, "actor")?;
    let summary = require_non_empty(summary, "summary")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if expected_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected_revision must be >= 1".to_owned(),
        ));
    }

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision,
            actual_revision: existing.revision,
        });
    }
    let next_revision = existing.revision + 1;
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 next_action = ?4,
                 updated_at = ?5
             WHERE card_id = ?1 AND revision = ?2",
            params![card_id, expected_revision, next_revision, next_action, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "record_stage_started_cas_update",
            source,
        })?;
    if updated == 0 {
        let current = load_card_row(conn, card_id)?;
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision,
            actual_revision: current.revision,
        });
    }

    let rev_id = format!("rev-{}", Uuid::new_v4());
    conn.execute(
        "INSERT INTO workflow_board_revisions (
           id, card_id, revision, stage, kind, actor_kind, summary, spec_digest,
           plan_digest, payload_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, 'stage_started', ?5, ?6, ?7, ?8, '{}', ?9)",
        params![
            rev_id,
            card_id,
            next_revision,
            existing.stage,
            actor,
            summary,
            existing.spec_digest,
            existing.plan_digest,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "record_stage_started_cas_insert_rev",
        source,
    })?;

    let detail = load_card_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}
