//! Read queries hydrating full M3 execution & Ship state (`CardExecutionDetail`) for a card.

use rusqlite::{Connection, Row, params};

use super::cli_import::get_cli_import_by_card;
use super::planning_queries::load_card_planning_detail_inner;
use super::ship_blockers::compute_ship_blockers_from_detail;
use super::types_m3::{
    AmendmentRow, CardExecutionDetail, CheckReceiptRow, ContractViolationRow, DeliveryReceiptRow,
    ExternalActionRow, ReviewFindingRow, ReviewRunRow,
};
use super::{WorkflowBoardRepoError, require_non_empty};

fn map_violation(row: &Row<'_>) -> rusqlite::Result<ContractViolationRow> {
    Ok(ContractViolationRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        task_id: row.get(2)?,
        stage_run_id: row.get(3)?,
        thread_id: row.get(4)?,
        workflow_run_id: row.get(5)?,
        card_revision: row.get(6)?,
        status: row.get(7)?,
        changed_files_json: row.get(8)?,
        allowed_files_json: row.get(9)?,
        unexpected_files_json: row.get(10)?,
        forbidden_files_json: row.get(11)?,
        preserved_in_worktree: row.get::<_, i64>(12)? != 0,
        blocking_reason: row.get(13)?,
        diff_digest: row.get(14)?,
        resolution_disposition: row.get(15)?,
        resolution_note: row.get(16)?,
        resolved_by: row.get(17)?,
        resolved_at: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
    })
}

fn map_check_receipt(row: &Row<'_>) -> rusqlite::Result<CheckReceiptRow> {
    Ok(CheckReceiptRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        check_id: row.get(2)?,
        criterion_id: row.get(3)?,
        task_id: row.get(4)?,
        card_revision: row.get(5)?,
        argv_json: row.get(6)?,
        snapshot_digest: row.get(7)?,
        commit_sha: row.get(8)?,
        spec_digest: row.get(9)?,
        plan_digest: row.get(10)?,
        status: row.get(11)?,
        exit_code: row.get(12)?,
        log_ref: row.get(13)?,
        stdout_excerpt: row.get(14)?,
        stderr_excerpt: row.get(15)?,
        duration_ms: row.get(16)?,
        stale_reason: row.get(17)?,
        executed_at: row.get(18)?,
        invalidated_at: row.get(19)?,
        updated_at: row.get(20)?,
    })
}

fn map_review_run(row: &Row<'_>) -> rusqlite::Result<ReviewRunRow> {
    Ok(ReviewRunRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        stage_run_id: row.get(2)?,
        thread_id: row.get(3)?,
        card_revision: row.get(4)?,
        snapshot_digest: row.get(5)?,
        status: row.get(6)?,
        verdict: row.get(7)?,
        summary: row.get(8)?,
        stale_reason: row.get(9)?,
        created_at: row.get(10)?,
        completed_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn map_review_finding(row: &Row<'_>) -> rusqlite::Result<ReviewFindingRow> {
    Ok(ReviewFindingRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        review_run_id: row.get(2)?,
        severity: row.get(3)?,
        criterion_id: row.get(4)?,
        file_path: row.get(5)?,
        line: row.get(6)?,
        message: row.get(7)?,
        status: row.get(8)?,
        disposition: row.get(9)?,
        resolution_note: row.get(10)?,
        resolved_by: row.get(11)?,
        resolved_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn map_amendment(row: &Row<'_>) -> rusqlite::Result<AmendmentRow> {
    Ok(AmendmentRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        card_revision: row.get(2)?,
        target_stage: row.get(3)?,
        status: row.get(4)?,
        reason: row.get(5)?,
        summary: row.get(6)?,
        source_task_id: row.get(7)?,
        unexpected_files_json: row.get(8)?,
        proposed_changes_json: row.get(9)?,
        proposed_by: row.get(10)?,
        resolved_by: row.get(11)?,
        resolved_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn map_external_action(row: &Row<'_>) -> rusqlite::Result<ExternalActionRow> {
    Ok(ExternalActionRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        action_kind: row.get(2)?,
        target_id: row.get(3)?,
        stage_run_id: row.get(4)?,
        idempotency_key: row.get(5)?,
        status: row.get(6)?,
        expected_snapshot_digest: row.get(7)?,
        observed_snapshot_digest: row.get(8)?,
        details_json: row.get(9)?,
        reconciliation_note: row.get(10)?,
        reconciled_by: row.get(11)?,
        reconciled_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn map_delivery_receipt(row: &Row<'_>) -> rusqlite::Result<DeliveryReceiptRow> {
    Ok(DeliveryReceiptRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        approval_id: row.get(2)?,
        status: row.get(3)?,
        card_revision: row.get(4)?,
        spec_digest: row.get(5)?,
        plan_digest: row.get(6)?,
        tasks_digest: row.get(7)?,
        checks_digest: row.get(8)?,
        product_snapshot_digest: row.get(9)?,
        commit_sha: row.get(10)?,
        diff_digest: row.get(11)?,
        review_id: row.get(12)?,
        ac_verdicts_summary_json: row.get(13)?,
        git_actions_json: row.get(14)?,
        delivery_note: row.get(15)?,
        approved_by: row.get(16)?,
        approved_at: row.get(17)?,
        invalidated_at: row.get(18)?,
        invalidation_reason: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
    })
}

fn query_vec<T>(
    conn: &Connection,
    sql: &str,
    card_id: &str,
    op: &'static str,
    mapper: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, WorkflowBoardRepoError> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: op,
            source,
        })?;
    stmt.query_map(params![card_id], mapper)
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: op,
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: op,
            source,
        })
}

pub(crate) fn load_card_execution_detail_inner(
    conn: &Connection,
    card_id: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let planning = load_card_planning_detail_inner(conn, card_id)?;
    let contract_violations = query_vec(
        conn,
        "SELECT id, card_id, task_id, stage_run_id, thread_id, workflow_run_id, card_revision,
                status, changed_files_json, allowed_files_json, unexpected_files_json,
                forbidden_files_json, preserved_in_worktree, blocking_reason, diff_digest,
                resolution_disposition, resolution_note, resolved_by, resolved_at, created_at, updated_at
         FROM workflow_board_contract_violations
         WHERE card_id = ?1
         ORDER BY created_at DESC, id DESC",
        card_id,
        "load_contract_violations",
        map_violation,
    )?;
    let check_receipts = query_vec(
        conn,
        "SELECT id, card_id, check_id, criterion_id, task_id, card_revision, argv_json,
                snapshot_digest, commit_sha, spec_digest, plan_digest, status, exit_code,
                log_ref, stdout_excerpt, stderr_excerpt, duration_ms, stale_reason,
                executed_at, invalidated_at, updated_at
         FROM workflow_board_check_receipts
         WHERE card_id = ?1
         ORDER BY COALESCE(executed_at, updated_at) DESC, id DESC",
        card_id,
        "load_check_receipts",
        map_check_receipt,
    )?;
    let review_runs = query_vec(
        conn,
        "SELECT id, card_id, stage_run_id, thread_id, card_revision, snapshot_digest, status,
                verdict, summary, stale_reason, created_at, completed_at, updated_at
         FROM workflow_board_review_runs
         WHERE card_id = ?1
         ORDER BY created_at DESC, id DESC",
        card_id,
        "load_review_runs",
        map_review_run,
    )?;
    let review_findings = query_vec(
        conn,
        "SELECT id, card_id, review_run_id, severity, criterion_id, file_path, line, message,
                status, disposition, resolution_note, resolved_by, resolved_at, created_at, updated_at
         FROM workflow_board_review_findings
         WHERE card_id = ?1
         ORDER BY created_at ASC, id ASC",
        card_id,
        "load_review_findings",
        map_review_finding,
    )?;
    let amendments = query_vec(
        conn,
        "SELECT id, card_id, card_revision, target_stage, status, reason, summary, source_task_id,
                unexpected_files_json, proposed_changes_json, proposed_by, resolved_by,
                resolved_at, created_at, updated_at
         FROM workflow_board_amendments
         WHERE card_id = ?1
         ORDER BY created_at DESC, id DESC",
        card_id,
        "load_amendments",
        map_amendment,
    )?;
    let external_actions = query_vec(
        conn,
        "SELECT id, card_id, action_kind, target_id, stage_run_id, idempotency_key, status,
                expected_snapshot_digest, observed_snapshot_digest, details_json,
                reconciliation_note, reconciled_by, reconciled_at, created_at, updated_at
         FROM workflow_board_external_actions
         WHERE card_id = ?1
         ORDER BY created_at DESC, id DESC",
        card_id,
        "load_external_actions",
        map_external_action,
    )?;
    let delivery_receipts = query_vec(
        conn,
        "SELECT id, card_id, approval_id, status, card_revision, spec_digest, plan_digest,
                tasks_digest, checks_digest, product_snapshot_digest, commit_sha, diff_digest,
                review_id, ac_verdicts_summary_json, git_actions_json, delivery_note,
                approved_by, approved_at, invalidated_at, invalidation_reason, created_at, updated_at
         FROM workflow_board_delivery_receipts
         WHERE card_id = ?1
         ORDER BY approved_at DESC, id DESC",
        card_id,
        "load_delivery_receipts",
        map_delivery_receipt,
    )?;

    let mut detail = CardExecutionDetail {
        planning,
        contract_violations,
        check_receipts,
        review_runs,
        review_findings,
        amendments,
        external_actions,
        delivery_receipts,
        ship_blockers: Vec::new(),
        cli_import: get_cli_import_by_card(conn, card_id)?,
    };
    detail.ship_blockers = compute_ship_blockers_from_detail(&detail);
    Ok(detail)
}

/// Loads full M1 + M2 + M3 execution & Ship aggregate for `card_id`.
pub fn get_card_execution_detail(
    conn: &Connection,
    card_id: &str,
) -> Result<CardExecutionDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    load_card_execution_detail_inner(conn, card_id)
}
