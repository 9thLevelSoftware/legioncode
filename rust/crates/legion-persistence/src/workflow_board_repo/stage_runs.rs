//! Stage run persistence and idempotent `stable_launch_key` thread-link operations.

use rusqlite::{Connection, OptionalExtension as _, Row, params};

use super::cards::load_card_row;
use super::{
    LinkStageRunThreadInput, StageRunRow, UpsertStageRunInput, UpsertStageRunOutcome,
    WorkflowBoardRepoError, WriteTx, require_non_empty,
};

pub(crate) fn map_stage_run_row(row: &Row<'_>) -> rusqlite::Result<StageRunRow> {
    Ok(StageRunRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        stage: row.get(2)?,
        task_id: row.get(3)?,
        attempt: row.get(4)?,
        stable_launch_key: row.get(5)?,
        status: row.get(6)?,
        card_revision: row.get(7)?,
        provider_instance_id: row.get(8)?,
        model: row.get(9)?,
        model_options_json: row.get(10)?,
        runtime_mode: row.get(11)?,
        interaction_mode: row.get(12)?,
        workspace_strategy_json: row.get(13)?,
        worktree_path: row.get(14)?,
        branch: row.get(15)?,
        thread_id: row.get(16)?,
        workflow_run_id: row.get(17)?,
        brief_digest: row.get(18)?,
        result_json: row.get(19)?,
        last_error: row.get(20)?,
        created_at: row.get(21)?,
        updated_at: row.get(22)?,
        ended_at: row.get(23)?,
    })
}

pub(crate) fn validate_stage(stage: &str) -> Result<&str, WorkflowBoardRepoError> {
    let trimmed = require_non_empty(stage, "stage")?;
    if !matches!(trimmed, "spec" | "plan" | "execute" | "review" | "ship") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported stage '{trimmed}'"
        )));
    }
    Ok(trimmed)
}

fn validate_stage_run_status(status: &str) -> Result<&str, WorkflowBoardRepoError> {
    let trimmed = require_non_empty(status, "status")?;
    if !matches!(
        trimmed,
        "pending" | "launching" | "running" | "paused" | "completed" | "failed" | "cancelled"
    ) {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported stage run status '{trimmed}'"
        )));
    }
    Ok(trimmed)
}

fn load_stage_run_by_id(
    conn: &Connection,
    id: &str,
) -> Result<StageRunRow, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                card_revision, provider_instance_id, model, model_options_json,
                runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                branch, thread_id, workflow_run_id, brief_digest, result_json,
                last_error, created_at, updated_at, ended_at
         FROM workflow_board_stage_runs
         WHERE id = ?1",
        params![id],
        map_stage_run_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "load_stage_run_by_id",
        source,
    })?
    .ok_or_else(|| WorkflowBoardRepoError::StageRunNotFound { id: id.to_owned() })
}

fn find_stage_run_by_launch_key(
    conn: &Connection,
    stable_launch_key: &str,
) -> Result<Option<StageRunRow>, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                card_revision, provider_instance_id, model, model_options_json,
                runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                branch, thread_id, workflow_run_id, brief_digest, result_json,
                last_error, created_at, updated_at, ended_at
         FROM workflow_board_stage_runs
         WHERE stable_launch_key = ?1",
        params![stable_launch_key],
        map_stage_run_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "find_stage_run_by_launch_key",
        source,
    })
}

/// Inserts a new `'pending'` stage run for `stable_launch_key`, or returns the existing row
/// idempotently if `(card_id, stage)` matches.
pub fn upsert_or_get_stage_run_by_launch_key(
    conn: &Connection,
    input: &UpsertStageRunInput<'_>,
    now_iso: &str,
) -> Result<UpsertStageRunOutcome, WorkflowBoardRepoError> {
    let id = require_non_empty(input.id, "id")?;
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let stage = validate_stage(input.stage)?;
    let stable_launch_key = require_non_empty(input.stable_launch_key, "stable_launch_key")?;
    let provider_instance_id =
        require_non_empty(input.provider_instance_id, "provider_instance_id")?;
    let model = require_non_empty(input.model, "model")?;
    let runtime_mode = require_non_empty(input.runtime_mode, "runtime_mode")?;
    let interaction_mode = require_non_empty(input.interaction_mode, "interaction_mode")?;
    let workspace_strategy_json =
        require_non_empty(input.workspace_strategy_json, "workspace_strategy_json")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if input.card_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "card_revision must be >= 1".to_owned(),
        ));
    }

    let tx = WriteTx::begin(conn)?;
    if let Some(existing) = find_stage_run_by_launch_key(conn, stable_launch_key)? {
        if existing.card_id != card_id
            || existing.stage != stage
            || existing.task_id.as_deref() != input.task_id
        {
            return Err(WorkflowBoardRepoError::ReuseConflict(format!(
                "stable_launch_key '{stable_launch_key}' belongs to card '{}' stage '{}' task {:?}, cannot reuse for card '{card_id}' stage '{stage}' task {:?}",
                existing.card_id, existing.stage, existing.task_id, input.task_id
            )));
        }
        tx.commit()?;
        return Ok(UpsertStageRunOutcome::Existing(existing));
    }

    let _card = load_card_row(conn, card_id)?;

    conn.execute(
        "INSERT INTO workflow_board_stage_runs (
           id, card_id, stage, task_id, attempt, stable_launch_key, status,
           card_revision, provider_instance_id, model, model_options_json,
           runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
           branch, brief_digest, created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, ?4, ?5, ?6, 'pending',
           ?7, ?8, ?9, ?10,
           ?11, ?12, ?13, ?14,
           ?15, ?16, ?17, ?17
         )",
        params![
            id,
            card_id,
            stage,
            input.task_id,
            input.attempt,
            stable_launch_key,
            input.card_revision,
            provider_instance_id,
            model,
            input.model_options_json,
            runtime_mode,
            interaction_mode,
            workspace_strategy_json,
            input.worktree_path,
            input.branch,
            input.brief_digest,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_stage_run",
        source,
    })?;

    let row = load_stage_run_by_id(conn, id)?;
    tx.commit()?;
    Ok(UpsertStageRunOutcome::Created(row))
}

/// Links a launched thread / workflow run / worktree metadata onto an existing stage run.
pub fn link_stage_run_thread(
    conn: &Connection,
    input: &LinkStageRunThreadInput<'_>,
    now_iso: &str,
) -> Result<StageRunRow, WorkflowBoardRepoError> {
    let id = require_non_empty(input.id, "id")?;
    let status = validate_stage_run_status(input.status)?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let updated = conn
        .execute(
            "UPDATE workflow_board_stage_runs
             SET status = ?2,
                 thread_id = COALESCE(?3, thread_id),
                 workflow_run_id = COALESCE(?4, workflow_run_id),
                 worktree_path = COALESCE(?5, worktree_path),
                 branch = COALESCE(?6, branch),
                 last_error = ?7,
                 ended_at = COALESCE(?8, ended_at),
                 updated_at = ?9
             WHERE id = ?1",
            params![
                id,
                status,
                input.thread_id,
                input.workflow_run_id,
                input.worktree_path,
                input.branch,
                input.last_error,
                input.ended_at,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "link_stage_run_thread",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::StageRunNotFound { id: id.to_owned() });
    }

    let row = load_stage_run_by_id(conn, id)?;
    tx.commit()?;
    Ok(row)
}

/// Lists all stage runs associated with `card_id` in chronological creation order.
pub fn list_stage_runs_for_card(
    conn: &Connection,
    card_id: &str,
) -> Result<Vec<StageRunRow>, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let mut stmt = conn
        .prepare(
            "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                    card_revision, provider_instance_id, model, model_options_json,
                    runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                    branch, thread_id, workflow_run_id, brief_digest, result_json,
                    last_error, created_at, updated_at, ended_at
             FROM workflow_board_stage_runs
             WHERE card_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "prepare_list_stage_runs",
            source,
        })?;
    let rows = stmt
        .query_map(params![card_id], map_stage_run_row)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "query_list_stage_runs",
            source,
        })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "collect_stage_runs",
            source,
        })
}

/// Returns the latest stage run bound to `thread_id`, prioritizing active runs over terminal ones.
pub fn get_latest_stage_run_for_thread(
    conn: &Connection,
    thread_id: &str,
) -> Result<Option<StageRunRow>, WorkflowBoardRepoError> {
    let thread_id = require_non_empty(thread_id, "thread_id")?;
    conn.query_row(
        "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                card_revision, provider_instance_id, model, model_options_json,
                runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                branch, thread_id, workflow_run_id, brief_digest, result_json,
                last_error, created_at, updated_at, ended_at
         FROM workflow_board_stage_runs
         WHERE thread_id = ?1
         ORDER BY CASE WHEN status IN ('pending', 'launching', 'running', 'paused') THEN 0 ELSE 1 END,
                  updated_at DESC, created_at DESC, id DESC
         LIMIT 1",
        params![thread_id],
        map_stage_run_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "get_latest_stage_run_for_thread",
        source,
    })
}

/// Returns the latest stage run bound to `workflow_run_id`, if any.
pub fn get_stage_run_by_workflow_run_id(
    conn: &Connection,
    workflow_run_id: &str,
) -> Result<Option<StageRunRow>, WorkflowBoardRepoError> {
    let workflow_run_id = require_non_empty(workflow_run_id, "workflow_run_id")?;
    conn.query_row(
        "SELECT id, card_id, stage, task_id, attempt, stable_launch_key, status,
                card_revision, provider_instance_id, model, model_options_json,
                runtime_mode, interaction_mode, workspace_strategy_json, worktree_path,
                branch, thread_id, workflow_run_id, brief_digest, result_json,
                last_error, created_at, updated_at, ended_at
         FROM workflow_board_stage_runs
         WHERE workflow_run_id = ?1
         ORDER BY updated_at DESC, created_at DESC, id DESC
         LIMIT 1",
        params![workflow_run_id],
        map_stage_run_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "get_stage_run_by_workflow_run_id",
        source,
    })
}
