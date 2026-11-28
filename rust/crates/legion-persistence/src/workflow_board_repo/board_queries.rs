//! Project board summary listing and snapshot queries for `workflow_board_repo`.

use rusqlite::{Connection, params};

use super::{BoardSnapshot, CardSummary, WorkflowBoardRepoError, require_non_empty};

/// Lists compact card summaries for `project_id`, ordered newest-updated first.
pub fn list_project_cards(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<CardSummary>, WorkflowBoardRepoError> {
    let project_id = require_non_empty(project_id, "project_id")?;
    let mut stmt = conn
        .prepare(
            "SELECT
               c.card_id,
               c.project_id,
               c.title,
               c.stage,
               c.status,
               c.revision,
               c.current_task_id,
               c.blocker_reason,
               c.next_action,
               (SELECT a.revision FROM workflow_board_approvals a
                WHERE a.card_id = c.card_id AND a.gate = 'spec'
                ORDER BY CASE WHEN a.status = 'valid' THEN 0 ELSE 1 END,
                         a.approved_at DESC, a.id DESC
                LIMIT 1) AS spec_approved_revision,
               (SELECT a.status FROM workflow_board_approvals a
                WHERE a.card_id = c.card_id AND a.gate = 'spec'
                ORDER BY CASE WHEN a.status = 'valid' THEN 0 ELSE 1 END,
                         a.approved_at DESC, a.id DESC
                LIMIT 1) AS spec_approval_status,
               (SELECT a.revision FROM workflow_board_approvals a
                WHERE a.card_id = c.card_id AND a.gate = 'plan'
                ORDER BY CASE WHEN a.status = 'valid' THEN 0 ELSE 1 END,
                         a.approved_at DESC, a.id DESC
                LIMIT 1) AS plan_approved_revision,
               (SELECT a.status FROM workflow_board_approvals a
                WHERE a.card_id = c.card_id AND a.gate = 'plan'
                ORDER BY CASE WHEN a.status = 'valid' THEN 0 ELSE 1 END,
                         a.approved_at DESC, a.id DESC
                LIMIT 1) AS plan_approval_status,
               (SELECT sr.thread_id FROM workflow_board_stage_runs sr
                WHERE sr.card_id = c.card_id AND sr.thread_id IS NOT NULL
                ORDER BY sr.updated_at DESC, sr.id DESC
                LIMIT 1) AS active_thread_id,
               (SELECT COUNT(*) FROM workflow_board_write_journal j
                WHERE j.card_id = c.card_id
                  AND j.status NOT IN ('committed', 'rolled_back')) AS incomplete_journal_count,
               c.external_edit_state,
               c.created_at,
               c.updated_at
             FROM workflow_board_cards c
             WHERE c.project_id = ?1
             ORDER BY c.updated_at DESC, c.card_id ASC",
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "prepare_list_project_cards",
            source,
        })?;

    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(CardSummary {
                card_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                stage: row.get(3)?,
                status: row.get(4)?,
                revision: row.get(5)?,
                current_task_id: row.get(6)?,
                blocker_reason: row.get(7)?,
                next_action: row.get(8)?,
                spec_approved_revision: row.get(9)?,
                spec_approval_status: row.get(10)?,
                plan_approved_revision: row.get(11)?,
                plan_approval_status: row.get(12)?,
                active_thread_id: row.get(13)?,
                incomplete_journal_count: row.get(14)?,
                external_edit_state: row.get(15)?,
                created_at: row.get(16)?,
                updated_at: row.get(17)?,
            })
        })
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "query_list_project_cards",
            source,
        })?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "collect_project_cards",
            source,
        })
}

/// Builds a compact [`BoardSnapshot`] for `project_id`.
pub fn get_board_snapshot(
    conn: &Connection,
    project_id: &str,
    now_iso: &str,
) -> Result<BoardSnapshot, WorkflowBoardRepoError> {
    let project_id = require_non_empty(project_id, "project_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    let cards = list_project_cards(conn, project_id)?;
    let revision = cards.iter().map(|c| c.revision).max().unwrap_or(0);
    Ok(BoardSnapshot {
        project_id: project_id.to_owned(),
        revision,
        cards,
        generated_at: now.to_owned(),
    })
}
