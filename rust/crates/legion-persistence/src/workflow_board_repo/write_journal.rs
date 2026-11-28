//! Deliverable write-journal lifecycle for crash recovery on `.legion/workflows/<card-id>/`.

use rusqlite::{Connection, OptionalExtension as _, Row, params};

use super::cards::load_card_row;
use super::{StageWriteInput, WorkflowBoardRepoError, WriteJournalRow, WriteTx, require_non_empty};

fn map_journal_row(row: &Row<'_>) -> rusqlite::Result<WriteJournalRow> {
    Ok(WriteJournalRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        target_path: row.get(2)?,
        staging_path: row.get(3)?,
        operation: row.get(4)?,
        status: row.get(5)?,
        expected_digest: row.get(6)?,
        previous_digest: row.get(7)?,
        revision: row.get(8)?,
        last_error: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        completed_at: row.get(12)?,
    })
}

fn load_journal_row(
    conn: &Connection,
    id: &str,
) -> Result<WriteJournalRow, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT id, card_id, target_path, staging_path, operation, status,
                expected_digest, previous_digest, revision, last_error,
                created_at, updated_at, completed_at
         FROM workflow_board_write_journal
         WHERE id = ?1",
        params![id],
        map_journal_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "load_journal_row",
        source,
    })?
    .ok_or_else(|| WorkflowBoardRepoError::JournalEntryNotFound { id: id.to_owned() })
}

/// Stages a deliverable write/delete operation in `workflow_board_write_journal` (`status = 'staged'`).
pub fn stage_write(
    conn: &Connection,
    input: &StageWriteInput<'_>,
    now_iso: &str,
) -> Result<WriteJournalRow, WorkflowBoardRepoError> {
    let id = require_non_empty(input.id, "id")?;
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let target_path = require_non_empty(input.target_path, "target_path")?;
    let operation = require_non_empty(input.operation, "operation")?;
    if !matches!(operation, "write" | "delete") {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported journal operation '{operation}'"
        )));
    }
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let _card = load_card_row(conn, card_id)?;

    conn.execute(
        "INSERT INTO workflow_board_write_journal (
           id, card_id, target_path, staging_path, operation, status,
           expected_digest, previous_digest, revision, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'staged', ?6, ?7, ?8, ?9, ?9)",
        params![
            id,
            card_id,
            target_path,
            input.staging_path,
            operation,
            input.expected_digest,
            input.previous_digest,
            input.revision,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_stage_write",
        source,
    })?;

    let row = load_journal_row(conn, id)?;
    tx.commit()?;
    Ok(row)
}

/// Marks a `'pending'` or `'staged'` journal entry as `'file_written'` after disk write completes.
pub fn mark_file_written(
    conn: &Connection,
    journal_id: &str,
    now_iso: &str,
) -> Result<WriteJournalRow, WorkflowBoardRepoError> {
    let id = require_non_empty(journal_id, "journal_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let current = load_journal_row(conn, id)?;
    if !matches!(current.status.as_str(), "pending" | "staged") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot mark journal '{id}' file_written from status '{}'",
            current.status
        )));
    }

    conn.execute(
        "UPDATE workflow_board_write_journal
         SET status = 'file_written', updated_at = ?2
         WHERE id = ?1",
        params![id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "mark_file_written",
        source,
    })?;

    let row = load_journal_row(conn, id)?;
    tx.commit()?;
    Ok(row)
}

/// Marks a `'pending'`, `'staged'`, or `'file_written'` journal entry as `'committed'`.
pub fn commit_journal(
    conn: &Connection,
    journal_id: &str,
    now_iso: &str,
) -> Result<WriteJournalRow, WorkflowBoardRepoError> {
    let id = require_non_empty(journal_id, "journal_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let current = load_journal_row(conn, id)?;
    if !matches!(
        current.status.as_str(),
        "pending" | "staged" | "file_written"
    ) {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot commit journal '{id}' from status '{}'",
            current.status
        )));
    }

    conn.execute(
        "UPDATE workflow_board_write_journal
         SET status = 'committed', updated_at = ?2, completed_at = ?2
         WHERE id = ?1",
        params![id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "commit_journal",
        source,
    })?;

    let row = load_journal_row(conn, id)?;
    tx.commit()?;
    Ok(row)
}

/// Rolls back a non-committed journal entry with an optional diagnostic error message.
pub fn rollback_journal(
    conn: &Connection,
    journal_id: &str,
    error: Option<&str>,
    now_iso: &str,
) -> Result<WriteJournalRow, WorkflowBoardRepoError> {
    let id = require_non_empty(journal_id, "journal_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let current = load_journal_row(conn, id)?;
    if current.status == "committed" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot roll back already committed journal '{id}'"
        )));
    }

    conn.execute(
        "UPDATE workflow_board_write_journal
         SET status = 'rolled_back', last_error = ?2, updated_at = ?3, completed_at = ?3
         WHERE id = ?1",
        params![id, error, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "rollback_journal",
        source,
    })?;

    let row = load_journal_row(conn, id)?;
    tx.commit()?;
    Ok(row)
}

/// Lists all incomplete journal entries (`status NOT IN ('committed', 'rolled_back')`).
pub fn list_incomplete_journal_entries(
    conn: &Connection,
    card_id: Option<&str>,
) -> Result<Vec<WriteJournalRow>, WorkflowBoardRepoError> {
    if let Some(cid) = card_id {
        let cid = require_non_empty(cid, "card_id")?;
        let mut stmt = conn
            .prepare(
                "SELECT id, card_id, target_path, staging_path, operation, status,
                        expected_digest, previous_digest, revision, last_error,
                        created_at, updated_at, completed_at
                 FROM workflow_board_write_journal
                 WHERE card_id = ?1 AND status NOT IN ('committed', 'rolled_back')
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "prepare_list_incomplete_journal_by_card",
                source,
            })?;
        stmt.query_map(params![cid], map_journal_row)
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "query_list_incomplete_journal_by_card",
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "collect_incomplete_journal_by_card",
                source,
            })
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT id, card_id, target_path, staging_path, operation, status,
                        expected_digest, previous_digest, revision, last_error,
                        created_at, updated_at, completed_at
                 FROM workflow_board_write_journal
                 WHERE status NOT IN ('committed', 'rolled_back')
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "prepare_list_incomplete_journal_all",
                source,
            })?;
        stmt.query_map([], map_journal_row)
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "query_list_incomplete_journal_all",
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "collect_incomplete_journal_all",
                source,
            })
    }
}
