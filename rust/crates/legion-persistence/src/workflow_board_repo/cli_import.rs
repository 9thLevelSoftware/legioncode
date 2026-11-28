//! Idempotent `.legion-cli` import upsert and policy acknowledgement persistence (`Migration 114`).

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::cards::{load_card_detail_inner, load_card_row};
use super::cli_import_helpers::{
    select_cli_import_by_card, select_cli_import_by_spec, upsert_imported_children,
};
use super::cli_import_policies::{
    MergedPolicyPayload, compute_case_a_policy_update, merge_case_b_policies,
};
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::types_m4::{ApplyCliImportInput, ApplyCliImportOutcome, CliImportRow};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

/// Returns the `workflow_board_cli_imports` row for `card_id`, if this card was CLI-imported.
pub fn get_cli_import_by_card(
    conn: &Connection,
    card_id: &str,
) -> Result<Option<CliImportRow>, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    select_cli_import_by_card(conn, card_id)
}

/// Looks up an existing imported card by `(project_id, source_cli_spec_id)`.
pub fn find_cli_import_by_spec(
    conn: &Connection,
    project_id: &str,
    source_cli_spec_id: &str,
) -> Result<Option<CliImportRow>, WorkflowBoardRepoError> {
    let project_id = require_non_empty(project_id, "project_id")?;
    let spec_id = require_non_empty(source_cli_spec_id, "source_cli_spec_id")?;
    select_cli_import_by_spec(conn, project_id, spec_id)
}

/// Resolves the first unused `card_id` in `workflow_board_cards` starting from `candidate`.
pub fn pick_available_card_id(
    conn: &Connection,
    candidate: &str,
) -> Result<String, WorkflowBoardRepoError> {
    let candidate = require_non_empty(candidate, "candidate_card_id")?;
    let mut attempt = candidate.to_owned();
    for suffix in 0_u32..32 {
        if suffix > 0 {
            attempt = format!("{candidate}-{suffix}");
        }
        let exists: Option<String> = conn
            .query_row(
                "SELECT card_id FROM workflow_board_cards WHERE card_id = ?1",
                params![attempt],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "check_cli_import_card_id_collision",
                source,
            })?;
        if exists.is_none() {
            return Ok(attempt);
        }
    }
    Err(WorkflowBoardRepoError::PreconditionFailed(format!(
        "Unable to allocate unique card_id from candidate `{candidate}`"
    )))
}

/// Inserts or idempotently updates a `.legion-cli` imported card with ownership preservation.
pub fn apply_cli_import_tx(
    conn: &Connection,
    input: &ApplyCliImportInput<'_>,
    now_iso: &str,
) -> Result<ApplyCliImportOutcome, WorkflowBoardRepoError> {
    let project_id = require_non_empty(input.project_id, "project_id")?;
    let spec_id = require_non_empty(input.source_cli_spec_id, "source_cli_spec_id")?;
    let cli_dir = require_non_empty(input.source_cli_dir, "source_cli_dir")?;
    let source_digest = require_non_empty(input.source_digest, "source_digest")?;
    let title = require_non_empty(input.title, "title")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    if let Some(existing) = select_cli_import_by_spec(conn, project_id, spec_id)? {
        let outcome =
            apply_existing_cli_import(conn, &existing, spec_id, source_digest, input, now)?;
        tx.commit()?;
        return Ok(outcome);
    }

    let card_id = pick_available_card_id(
        conn,
        require_non_empty(input.candidate_card_id, "candidate_card_id")?,
    )?;
    insert_new_import(
        conn,
        &card_id,
        project_id,
        spec_id,
        cli_dir,
        source_digest,
        title,
        input,
        now,
    )?;
    upsert_imported_children(conn, &card_id, 1, input, now)?;
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id: input.revision_id,
            card_id: &card_id,
            revision: 1,
            stage: input.stage,
            kind: "cli_imported",
            summary: &format!("Imported `.legion-cli` spec `{spec_id}`"),
            spec_digest: Some(input.spec_digest),
            plan_digest: input.plan_digest,
            payload_json: &json!({ "sourceSpecId": spec_id, "sourceDigest": source_digest })
                .to_string(),
            now,
        },
    )?;
    let cli_import = select_cli_import_by_card(conn, &card_id)?.ok_or_else(|| {
        WorkflowBoardRepoError::CardNotFound {
            card_id: card_id.clone(),
        }
    })?;
    let card_detail = load_card_detail_inner(conn, &card_id)?;
    tx.commit()?;
    Ok(ApplyCliImportOutcome {
        card_id,
        revision: 1,
        created: true,
        reused: false,
        ownership_preserved: false,
        policy_ack_only: false,
        cli_import,
        card_detail,
    })
}

fn apply_existing_cli_import(
    conn: &Connection,
    existing: &CliImportRow,
    spec_id: &str,
    source_digest: &str,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Result<ApplyCliImportOutcome, WorkflowBoardRepoError> {
    let card = load_card_row(conn, &existing.card_id)?;
    if card.revision > existing.last_imported_revision {
        conn.execute(
            "UPDATE workflow_board_cli_imports SET last_synced_at = ?2 WHERE card_id = ?1",
            params![existing.card_id, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "touch_cli_import_synced_at",
            source,
        })?;
        let cli_import =
            select_cli_import_by_card(conn, &existing.card_id)?.unwrap_or_else(|| existing.clone());
        let card_detail = load_card_detail_inner(conn, &existing.card_id)?;
        return Ok(ApplyCliImportOutcome {
            card_id: card.card_id,
            revision: card.revision,
            created: false,
            reused: true,
            ownership_preserved: true,
            policy_ack_only: false,
            cli_import,
            card_detail,
        });
    }
    if existing.source_digest == source_digest {
        return apply_existing_case_a(conn, existing, &card, input, now);
    }
    apply_existing_case_b(conn, existing, &card, spec_id, source_digest, input, now)
}

fn apply_existing_case_a(
    conn: &Connection,
    existing: &CliImportRow,
    card: &super::types::CardRow,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Result<ApplyCliImportOutcome, WorkflowBoardRepoError> {
    let Some(ack) = compute_case_a_policy_update(existing, input, now) else {
        let cli_import = existing.clone();
        let card_detail = load_card_detail_inner(conn, &card.card_id)?;
        return Ok(ApplyCliImportOutcome {
            card_id: card.card_id.clone(),
            revision: card.revision,
            created: false,
            reused: true,
            ownership_preserved: false,
            policy_ack_only: false,
            cli_import,
            card_detail,
        });
    };
    let next_rev = card.revision + 1;
    let next_action = if ack.all_blocking_acked && input.next_action == Some("acknowledge_policies")
    {
        if card.current_task_id.is_some() {
            Some("resolve_blocked_task")
        } else {
            Some("start_execute")
        }
    } else {
        input.next_action.or(card.next_action.as_deref())
    };
    apply_case_a_policy_update(
        conn,
        &card.card_id,
        &card.stage,
        next_rev,
        next_action,
        card,
        input,
        &ack,
        now,
    )?;
    let cli_import = select_cli_import_by_card(conn, &card.card_id)?.ok_or_else(|| {
        WorkflowBoardRepoError::CardNotFound {
            card_id: card.card_id.clone(),
        }
    })?;
    let card_detail = load_card_detail_inner(conn, &card.card_id)?;
    Ok(ApplyCliImportOutcome {
        card_id: card.card_id.clone(),
        revision: next_rev,
        created: false,
        reused: false,
        ownership_preserved: false,
        policy_ack_only: true,
        cli_import,
        card_detail,
    })
}

fn apply_existing_case_b(
    conn: &Connection,
    existing: &CliImportRow,
    card: &super::types::CardRow,
    spec_id: &str,
    source_digest: &str,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Result<ApplyCliImportOutcome, WorkflowBoardRepoError> {
    let next_rev = card.revision + 1;
    let merged = merge_case_b_policies(existing, input, now);
    update_existing_import(conn, &card.card_id, next_rev, input, &merged, now)?;
    upsert_imported_children(conn, &card.card_id, next_rev, input, now)?;
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id: input.revision_id,
            card_id: &card.card_id,
            revision: next_rev,
            stage: input.stage,
            kind: "cli_reimported",
            summary: &format!("Re-imported `.legion-cli` spec `{spec_id}`"),
            spec_digest: Some(input.spec_digest),
            plan_digest: input.plan_digest,
            payload_json: &json!({ "sourceSpecId": spec_id, "sourceDigest": source_digest })
                .to_string(),
            now,
        },
    )?;
    let cli_import = select_cli_import_by_card(conn, &card.card_id)?.ok_or_else(|| {
        WorkflowBoardRepoError::CardNotFound {
            card_id: card.card_id.clone(),
        }
    })?;
    let card_detail = load_card_detail_inner(conn, &card.card_id)?;
    Ok(ApplyCliImportOutcome {
        card_id: card.card_id.clone(),
        revision: next_rev,
        created: false,
        reused: false,
        ownership_preserved: false,
        policy_ack_only: false,
        cli_import,
        card_detail,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Applies Case A policy acknowledgement update atomically across card, cli_import, and ledger"
)]
fn apply_case_a_policy_update(
    conn: &Connection,
    card_id: &str,
    stage: &str,
    next_rev: i64,
    next_action: Option<&str>,
    card: &super::types::CardRow,
    input: &ApplyCliImportInput<'_>,
    ack: &MergedPolicyPayload,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_cards
         SET revision = ?2,
             next_action = ?3,
             import_provenance_json = ?4,
             updated_at = ?5
         WHERE card_id = ?1",
        params![
            card_id,
            next_rev,
            next_action,
            ack.import_provenance_json,
            now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_card_on_case_a_policy_ack",
        source,
    })?;
    conn.execute(
        "UPDATE workflow_board_cli_imports
         SET last_imported_revision = ?2,
             unsupported_policies_json = ?3,
             acknowledged_policies_json = ?4,
             policies_acknowledged_at = ?5,
             import_provenance_json = ?6,
             last_synced_at = ?7
         WHERE card_id = ?1",
        params![
            card_id,
            next_rev,
            ack.unsupported_policies_json,
            ack.acknowledged_policies_json,
            ack.policies_acknowledged_at,
            ack.import_provenance_json,
            now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_cli_import_on_case_a_policy_ack",
        source,
    })?;
    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id: input.revision_id,
            card_id,
            revision: next_rev,
            stage,
            kind: "cli_policies_acknowledged",
            summary: "Acknowledged unsupported adopted CLI policies via CLI import",
            spec_digest: card.spec_digest.as_deref(),
            plan_digest: card.plan_digest.as_deref(),
            payload_json: &ack.acknowledged_policies_json,
            now,
        },
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Inserts initial imported card and workflow_board_cli_imports row atomically"
)]
fn insert_new_import(
    conn: &Connection,
    card_id: &str,
    project_id: &str,
    spec_id: &str,
    cli_dir: &str,
    source_digest: &str,
    title: &str,
    input: &ApplyCliImportInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "INSERT INTO workflow_board_cards (
           card_id, project_id, title, stage, status, revision, brief,
           intake_goal, input_paths_json, source_type, assistance_mode,
           workspace_strategy_json, worktree_path, branch, model_selection_json,
           runtime_mode, interaction_mode, spec_digest, design_digest, plan_digest,
           tasks_digest, current_task_id, blocker_reason, next_action,
           spec_json, plan_json, import_provenance_json, created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, ?4, ?5, 1, ?6,
           'change', '[]', 'codebase', 'balanced',
           ?7, ?8, ?9, ?10,
           ?11, ?12, ?13, ?14, ?15,
           ?16, ?17, ?18, ?19,
           ?20, ?21, ?22, ?23, ?23
         )",
        params![
            card_id,
            project_id,
            title,
            input.stage,
            input.status,
            input.brief,
            input.workspace_strategy_json,
            input.worktree_path,
            input.branch,
            input.model_selection_json,
            input.runtime_mode,
            input.interaction_mode,
            input.spec_digest,
            input.design_digest,
            input.plan_digest,
            input.tasks_digest,
            input.current_task_id,
            input.blocker_reason,
            input.next_action,
            input.spec_json,
            input.plan_json,
            input.import_provenance_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_cli_imported_card",
        source,
    })?;

    conn.execute(
        "INSERT INTO workflow_board_cli_imports (
           card_id, project_id, source_cli_spec_id, source_cli_dir, source_digest,
           last_imported_revision, ownership_owner, unsupported_policies_json,
           acknowledged_policies_json, policies_acknowledged_at, assurance_policy_json,
           import_provenance_json, imported_at, last_synced_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 1, 'legioncode', ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            card_id,
            project_id,
            spec_id,
            cli_dir,
            source_digest,
            input.unsupported_policies_json,
            input.acknowledged_policies_json,
            input.policies_acknowledged_at,
            input.assurance_policy_json,
            input.import_provenance_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_cli_import_row",
        source,
    })?;
    Ok(())
}

fn update_existing_import(
    conn: &Connection,
    card_id: &str,
    next_rev: i64,
    input: &ApplyCliImportInput<'_>,
    merged: &MergedPolicyPayload,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let next_action =
        if merged.all_blocking_acked && input.next_action == Some("acknowledge_policies") {
            if input.current_task_id.is_some() {
                Some("resolve_blocked_task")
            } else {
                Some("start_execute")
            }
        } else {
            input.next_action
        };
    conn.execute(
        "UPDATE workflow_board_cards
         SET title = ?2,
             stage = ?3,
             status = ?4,
             revision = ?5,
             brief = ?6,
             spec_digest = ?7,
             design_digest = ?8,
             plan_digest = ?9,
             tasks_digest = ?10,
             current_task_id = ?11,
             blocker_reason = ?12,
             next_action = ?13,
             spec_json = ?14,
             plan_json = ?15,
             import_provenance_json = ?16,
             updated_at = ?17
         WHERE card_id = ?1",
        params![
            card_id,
            input.title,
            input.stage,
            input.status,
            next_rev,
            input.brief,
            input.spec_digest,
            input.design_digest,
            input.plan_digest,
            input.tasks_digest,
            input.current_task_id,
            input.blocker_reason,
            next_action,
            input.spec_json,
            input.plan_json,
            merged.import_provenance_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_cli_imported_card",
        source,
    })?;

    conn.execute(
        "UPDATE workflow_board_cli_imports
         SET source_cli_dir = ?2,
             source_digest = ?3,
             last_imported_revision = ?4,
             unsupported_policies_json = ?5,
             acknowledged_policies_json = ?6,
             policies_acknowledged_at = ?7,
             assurance_policy_json = ?8,
             import_provenance_json = ?9,
             last_synced_at = ?10
         WHERE card_id = ?1",
        params![
            card_id,
            input.source_cli_dir,
            input.source_digest,
            next_rev,
            merged.unsupported_policies_json,
            merged.acknowledged_policies_json,
            merged.policies_acknowledged_at,
            input.assurance_policy_json,
            merged.import_provenance_json,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "update_cli_import_row",
        source,
    })?;
    Ok(())
}
