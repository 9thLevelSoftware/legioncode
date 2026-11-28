//! Idempotent child-table upsert helpers for [`super::cli_import`].

use rusqlite::{Connection, OptionalExtension as _, Row, params};

use super::WorkflowBoardRepoError;
use super::types_m4::{
    ApplyCliImportInput, CliImportAcEvidenceSeed, CliImportApprovalSeed, CliImportDecisionSeed,
    CliImportPlannedCheckSeed, CliImportRow, CliImportTaskSeed,
};

pub(super) fn map_cli_import_row(row: &Row<'_>) -> rusqlite::Result<CliImportRow> {
    Ok(CliImportRow {
        card_id: row.get(0)?,
        project_id: row.get(1)?,
        source_cli_spec_id: row.get(2)?,
        source_cli_dir: row.get(3)?,
        source_digest: row.get(4)?,
        last_imported_revision: row.get(5)?,
        ownership_owner: row.get(6)?,
        unsupported_policies_json: row.get(7)?,
        acknowledged_policies_json: row.get(8)?,
        policies_acknowledged_at: row.get(9)?,
        assurance_policy_json: row.get(10)?,
        import_provenance_json: row.get(11)?,
        imported_at: row.get(12)?,
        last_synced_at: row.get(13)?,
    })
}

pub(super) fn select_cli_import_by_card(
    conn: &Connection,
    card_id: &str,
) -> Result<Option<CliImportRow>, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT card_id, project_id, source_cli_spec_id, source_cli_dir, source_digest,
                last_imported_revision, ownership_owner, unsupported_policies_json,
                acknowledged_policies_json, policies_acknowledged_at, assurance_policy_json,
                import_provenance_json, imported_at, last_synced_at
         FROM workflow_board_cli_imports
         WHERE card_id = ?1",
        params![card_id],
        map_cli_import_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "select_cli_import_by_card",
        source,
    })
}

pub(super) fn select_cli_import_by_spec(
    conn: &Connection,
    project_id: &str,
    source_cli_spec_id: &str,
) -> Result<Option<CliImportRow>, WorkflowBoardRepoError> {
    conn.query_row(
        "SELECT card_id, project_id, source_cli_spec_id, source_cli_dir, source_digest,
                last_imported_revision, ownership_owner, unsupported_policies_json,
                acknowledged_policies_json, policies_acknowledged_at, assurance_policy_json,
                import_provenance_json, imported_at, last_synced_at
         FROM workflow_board_cli_imports
         WHERE project_id = ?1 AND source_cli_spec_id = ?2",
        params![project_id, source_cli_spec_id],
        map_cli_import_row,
    )
    .optional()
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "select_cli_import_by_spec",
        source,
    })
}

pub(super) fn upsert_imported_children(
    conn: &Connection,
    card_id: &str,
    card_revision: i64,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    upsert_decisions(conn, card_id, input.decisions, now)?;
    upsert_tasks(conn, card_id, input.tasks, now)?;
    upsert_planned_checks(conn, card_id, input.planned_checks, now)?;
    upsert_check_receipts(conn, card_id, card_revision, input, now)?;
    upsert_ac_evidence(conn, card_id, input.ac_evidence, now)?;
    upsert_historical_approvals(conn, card_id, card_revision, input.historical_approvals)?;
    if let Some(summary) = input.historical_review_summary {
        let rev_id = format!("hist-rev-{card_id}");
        let recorded = input.historical_review_at.unwrap_or(now);
        conn.execute(
            "INSERT INTO workflow_board_review_runs (
               id, card_id, card_revision, status, verdict, summary,
               stale_reason, created_at, completed_at, updated_at
             ) VALUES (?1, ?2, ?3, 'historical', 'stale', ?4,
                       'imported_historical_cli_review', ?5, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               card_revision = excluded.card_revision,
               summary = excluded.summary,
               updated_at = excluded.updated_at",
            params![rev_id, card_id, card_revision, summary, recorded, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_historical_review_run",
            source,
        })?;
    }
    Ok(())
}

fn upsert_decisions(
    conn: &Connection,
    card_id: &str,
    decisions: &[CliImportDecisionSeed<'_>],
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    for d in decisions {
        conn.execute(
            "INSERT INTO workflow_board_decisions (
               card_id, decision_id, round_index, prompt, recommendation,
               depends_on_json, last_action, status, answer, rationale, created_at, updated_at
             ) VALUES (?1, ?2, NULL, ?3, NULL, '[]', NULL, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(card_id, decision_id) DO UPDATE SET
               prompt = excluded.prompt,
               status = excluded.status,
               answer = excluded.answer,
               rationale = excluded.rationale,
               updated_at = excluded.updated_at",
            params![
                card_id,
                d.decision_id,
                d.prompt,
                d.status,
                d.answer,
                d.rationale,
                now
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_cli_decision",
            source,
        })?;
    }
    Ok(())
}

fn upsert_tasks(
    conn: &Connection,
    card_id: &str,
    tasks: &[CliImportTaskSeed<'_>],
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    for t in tasks {
        conn.execute(
            "INSERT INTO workflow_board_tasks (
               card_id, task_id, ordinal, title, status, priority, parent_id,
               blocked_by_json, blocks_json, files_allowed_json, files_forbidden_json,
               expected_artifacts_json, verification_commands_json, testing_method,
               testing_boundary, acceptance_ids_json, blocker_reason, notes,
               max_files_touched, contract_digest, last_error, created_at, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, ?6, ?7,
               ?8, ?9, ?10, ?11,
               ?12, ?13, ?14,
               NULL, ?15, ?16, ?17,
               ?18, ?19, ?20, ?21, ?21
             )
             ON CONFLICT(card_id, task_id) DO UPDATE SET
               ordinal = excluded.ordinal,
               title = excluded.title,
               status = excluded.status,
               priority = excluded.priority,
               parent_id = excluded.parent_id,
               blocked_by_json = excluded.blocked_by_json,
               blocks_json = excluded.blocks_json,
               files_allowed_json = excluded.files_allowed_json,
               files_forbidden_json = excluded.files_forbidden_json,
               expected_artifacts_json = excluded.expected_artifacts_json,
               verification_commands_json = excluded.verification_commands_json,
               testing_method = excluded.testing_method,
               acceptance_ids_json = excluded.acceptance_ids_json,
               blocker_reason = excluded.blocker_reason,
               notes = excluded.notes,
               max_files_touched = excluded.max_files_touched,
               contract_digest = excluded.contract_digest,
               last_error = excluded.last_error,
               updated_at = excluded.updated_at",
            params![
                card_id,
                t.task_id,
                t.ordinal,
                t.title,
                t.status,
                t.priority,
                t.parent_id,
                t.blocked_by_json,
                t.blocks_json,
                t.files_allowed_json,
                t.files_forbidden_json,
                t.expected_artifacts_json,
                t.verification_commands_json,
                t.testing_method,
                t.acceptance_ids_json,
                t.blocker_reason,
                t.notes,
                t.max_files_touched,
                t.contract_digest,
                t.last_error,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_cli_task",
            source,
        })?;
    }
    Ok(())
}

fn upsert_planned_checks(
    conn: &Connection,
    card_id: &str,
    checks: &[CliImportPlannedCheckSeed<'_>],
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    for c in checks {
        conn.execute(
            "INSERT INTO workflow_board_planned_checks (
               card_id, check_id, owner_task_id, argv_json, label, status,
               last_exit_code, last_log_ref, stale_reason, last_receipt_id,
               last_run_at, last_output_excerpt, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'historical', ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(card_id, check_id) DO UPDATE SET
               owner_task_id = excluded.owner_task_id,
               argv_json = excluded.argv_json,
               label = excluded.label,
               status = 'historical',
               last_exit_code = excluded.last_exit_code,
               last_log_ref = excluded.last_log_ref,
               stale_reason = excluded.stale_reason,
               last_receipt_id = excluded.last_receipt_id,
               last_run_at = excluded.last_run_at,
               last_output_excerpt = excluded.last_output_excerpt,
               updated_at = excluded.updated_at",
            params![
                card_id,
                c.check_id,
                c.owner_task_id,
                c.argv_json,
                c.label,
                c.last_exit_code,
                c.last_log_ref,
                c.stale_reason,
                c.last_receipt_id,
                c.last_run_at,
                c.last_output_excerpt,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_cli_planned_check",
            source,
        })?;
    }
    Ok(())
}

fn upsert_check_receipts(
    conn: &Connection,
    card_id: &str,
    card_revision: i64,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    for r in input.check_receipts {
        let receipt_id = format!("hist-{card_id}-{}", r.id);
        conn.execute(
            "INSERT INTO workflow_board_check_receipts (
               id, card_id, check_id, criterion_id, task_id, card_revision,
               argv_json, spec_digest, plan_digest, status, exit_code, log_ref,
               stderr_excerpt, stale_reason, executed_at, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, ?6,
               ?7, ?8, ?9, 'historical', ?10, ?11,
               ?12, ?13, ?14, ?15
             )
             ON CONFLICT(id) DO UPDATE SET
               criterion_id = excluded.criterion_id,
               task_id = excluded.task_id,
               card_revision = excluded.card_revision,
               argv_json = excluded.argv_json,
               status = 'historical',
               exit_code = excluded.exit_code,
               log_ref = excluded.log_ref,
               stderr_excerpt = excluded.stderr_excerpt,
               stale_reason = excluded.stale_reason,
               executed_at = excluded.executed_at,
               updated_at = excluded.updated_at",
            params![
                receipt_id,
                card_id,
                r.check_id,
                r.criterion_id,
                r.task_id,
                card_revision,
                r.argv_json,
                input.spec_digest,
                input.plan_digest,
                r.exit_code,
                r.log_ref,
                r.stderr_excerpt,
                r.stale_reason,
                r.executed_at,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_cli_check_receipt",
            source,
        })?;
    }
    Ok(())
}

fn upsert_ac_evidence(
    conn: &Connection,
    card_id: &str,
    ac_evidence: &[CliImportAcEvidenceSeed<'_>],
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    for ac in ac_evidence {
        conn.execute(
            "INSERT INTO workflow_board_ac_evidence (
               card_id, criterion_id, method, check_ids_json, command_argv_json,
               procedure, status, observation_note, stale_reason, external_source, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5,
               ?6, 'historical', ?7, 'imported_historical_cli_evidence', ?8, ?9
             )
             ON CONFLICT(card_id, criterion_id) DO UPDATE SET
               method = excluded.method,
               check_ids_json = excluded.check_ids_json,
               command_argv_json = excluded.command_argv_json,
               procedure = excluded.procedure,
               status = 'historical',
               observation_note = excluded.observation_note,
               stale_reason = excluded.stale_reason,
               external_source = excluded.external_source,
               updated_at = excluded.updated_at",
            params![
                card_id,
                ac.criterion_id,
                ac.method,
                ac.check_ids_json,
                ac.command_argv_json,
                ac.procedure,
                ac.observation_note,
                ac.external_source,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_cli_ac_evidence",
            source,
        })?;
    }
    Ok(())
}

fn upsert_historical_approvals(
    conn: &Connection,
    card_id: &str,
    card_revision: i64,
    approvals: &[CliImportApprovalSeed<'_>],
) -> Result<(), WorkflowBoardRepoError> {
    for a in approvals {
        let approval_id = format!("hist-{}-{card_id}", a.id);
        conn.execute(
            "INSERT INTO workflow_board_approvals (
               id, card_id, gate, status, revision, artifact_digest,
               spec_digest, plan_digest, tasks_digest, approved_by, approved_at, metadata_json
             ) VALUES (?1, ?2, ?3, 'historical', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               revision = excluded.revision,
               artifact_digest = excluded.artifact_digest,
               spec_digest = excluded.spec_digest,
               plan_digest = excluded.plan_digest,
               tasks_digest = excluded.tasks_digest,
               approved_by = excluded.approved_by,
               approved_at = excluded.approved_at,
               metadata_json = excluded.metadata_json",
            params![
                approval_id,
                card_id,
                a.gate,
                card_revision,
                a.artifact_digest,
                a.spec_digest,
                a.plan_digest,
                a.tasks_digest,
                a.approved_by,
                a.approved_at,
                a.metadata_json,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "upsert_cli_historical_approval",
            source,
        })?;
    }
    Ok(())
}
