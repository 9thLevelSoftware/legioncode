//! Spec update (CAS + approval invalidation) and spec approval operations.

use rusqlite::{Connection, params};

use super::cards::{load_card_detail_inner, load_card_row};
use super::invalidation::{
    compute_next_spec_state, stale_active_challenges, stale_approvals_for_gates,
    stale_downstream_evidence,
};
use super::plan_readiness::validate_spec_approval_m2_readiness;
use super::plan_return::{reset_in_progress_tasks_to_todo, restore_remaining_blocker_on_return};
use super::{
    ApproveSpecInput, CardDetail, UpdateSpecInput, WorkflowBoardRepoError, WriteTx,
    require_non_empty,
};

struct NewRevision<'a> {
    revision_id: &'a str,
    card_id: &'a str,
    revision: i64,
    stage: &'a str,
    kind: &'a str,
    summary: &'a str,
    spec_digest: Option<&'a str>,
    payload_json: &'a str,
    now: &'a str,
}

fn insert_revision_row(
    conn: &Connection,
    entry: &NewRevision<'_>,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "INSERT INTO workflow_board_revisions (
           id, card_id, revision, stage, kind, actor_kind, summary, spec_digest,
           payload_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'client', ?6, ?7, ?8, ?9)",
        params![
            entry.revision_id,
            entry.card_id,
            entry.revision,
            entry.stage,
            entry.kind,
            entry.summary,
            entry.spec_digest,
            entry.payload_json,
            entry.now
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_workflow_board_revision",
        source,
    })?;
    Ok(())
}

fn check_cas_updated(
    conn: &Connection,
    card_id: &str,
    expected_revision: i64,
    updated_rows: usize,
) -> Result<(), WorkflowBoardRepoError> {
    if updated_rows == 0 {
        let current = load_card_row(conn, card_id)?;
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision,
            actual_revision: current.revision,
        });
    }
    Ok(())
}

fn stale_active_approvals_on_spec_update(
    conn: &Connection,
    card_id: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    stale_approvals_for_gates(
        conn,
        card_id,
        &["spec", "plan", "delivery"],
        "spec_updated",
        now,
    )?;
    stale_active_challenges(conn, card_id, now)?;
    stale_downstream_evidence(conn, card_id, "spec_updated", now)?;
    Ok(())
}

/// Updates spec fields with strict `expected_revision` CAS protection.
pub fn update_spec(
    conn: &Connection,
    input: &UpdateSpecInput<'_>,
    now_iso: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if input.expected_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected_revision must be >= 1".to_owned(),
        ));
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

    let next = compute_next_spec_state(&existing, input)?;
    if !next.changed {
        let detail = load_card_detail_inner(conn, card_id)?;
        tx.commit()?;
        return Ok(detail);
    }

    let next_revision = existing.revision + 1;
    let regressing = existing.stage != "spec";
    let next_stage = if regressing {
        "spec"
    } else {
        existing.stage.as_str()
    };
    let next_action = if regressing || existing.status != "active" {
        Some("revise_spec")
    } else {
        existing.next_action.as_deref()
    };
    let updated_rows = conn
        .execute(
            "UPDATE workflow_board_cards
             SET title = ?3, brief = ?4, spec_json = ?5, spec_digest = ?6,
                 intake_goal = ?7, input_paths_json = ?8, wireframes_digest = ?9,
                 revision = ?10, stage = ?11,
                 status = 'active',
                 completed_at = NULL,
                 paused_from_status = NULL,
                 pause_reason = NULL,
                 paused_at = NULL,
                 blocker_reason = NULL,
                 current_task_id = CASE WHEN ?14 THEN NULL ELSE current_task_id END,
                 next_action = ?12, updated_at = ?13
             WHERE card_id = ?1 AND revision = ?2",
            params![
                card_id,
                input.expected_revision,
                next.title,
                next.brief,
                next.spec_json,
                next.digest,
                next.intake_goal,
                next.input_paths_json,
                next.wireframes_digest,
                next_revision,
                next_stage,
                next_action,
                now,
                regressing,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_spec_card",
            source,
        })?;
    check_cas_updated(conn, card_id, input.expected_revision, updated_rows)?;
    if regressing {
        reset_in_progress_tasks_to_todo(conn, card_id, now)?;
    }
    stale_active_approvals_on_spec_update(conn, card_id, now)?;
    restore_remaining_blocker_on_return(conn, card_id, next_stage)?;

    insert_revision_row(
        conn,
        &NewRevision {
            revision_id,
            card_id,
            revision: next_revision,
            stage: next_stage,
            kind: "spec_updated",
            summary: "Updated specification",
            spec_digest: next.digest.as_deref(),
            payload_json: &next.spec_json,
            now,
        },
    )?;

    let detail = load_card_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

fn supersede_and_insert_spec_approval(
    conn: &Connection,
    input: &ApproveSpecInput<'_>,
    next_revision: i64,
    spec_digest: &str,
    wireframes_digest: Option<&str>,
    approved_by: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_approvals
         SET status = 'superseded',
             invalidated_at = ?2,
             invalidation_reason = 'superseded_by_new_approval'
         WHERE card_id = ?1
           AND gate = 'spec'
           AND status = 'valid'",
        params![input.card_id.trim(), now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "supersede_prior_spec_approval",
        source,
    })?;

    conn.execute(
        "INSERT INTO workflow_board_approvals (
           id, card_id, gate, status, revision, artifact_digest, spec_digest,
           wireframes_digest, approved_by, approved_at
         ) VALUES (?1, ?2, 'spec', 'valid', ?3, ?4, ?4, ?5, ?6, ?7)",
        params![
            input.approval_id.trim(),
            input.card_id.trim(),
            next_revision,
            spec_digest,
            wireframes_digest,
            approved_by,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_spec_approval",
        source,
    })?;
    Ok(())
}

/// Approves the current card spec with strict `expected_revision` CAS protection and digest binding.
///
/// Supersedes any prior `'valid'` `'spec'` approval, records a new `'valid'` `'spec'` approval
/// bound to `(revision, spec_digest)`, and advances `stage` from `'spec'` to `'plan'`.
pub fn approve_spec(
    conn: &Connection,
    input: &ApproveSpecInput<'_>,
    now_iso: &str,
) -> Result<CardDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let _approval_id = require_non_empty(input.approval_id, "approval_id")?;
    let approved_by = require_non_empty(input.approved_by, "approved_by")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if input.expected_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected_revision must be >= 1".to_owned(),
        ));
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
    if matches!(existing.status.as_str(), "paused" | "completed" | "shipped") {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot approve spec while card status is `{}`",
            existing.status
        )));
    }
    if existing.stage != "spec" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot approve spec while card is in `{}` stage (expected `spec`)",
            existing.stage
        )));
    }

    let spec_digest = existing
        .spec_digest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            WorkflowBoardRepoError::PreconditionFailed(
                "cannot approve spec without a non-empty spec_digest".to_owned(),
            )
        })?
        .to_owned();

    if let Some(expected_digest) = input.expected_spec_digest {
        let trimmed = require_non_empty(expected_digest, "expected_spec_digest")?;
        if trimmed != spec_digest {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "spec digest mismatch: expected '{trimmed}', actual '{spec_digest}'"
            )));
        }
    }

    let next_revision = existing.revision + 1;
    validate_spec_approval_m2_readiness(conn, card_id, &existing.spec_json)?;
    let effective_wf_digest = input
        .wireframes_digest
        .or(existing.wireframes_digest.as_deref());
    supersede_and_insert_spec_approval(
        conn,
        input,
        next_revision,
        &spec_digest,
        effective_wf_digest,
        approved_by,
        now,
    )?;

    let updated_rows = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 stage = 'plan',
                 status = 'active',
                 blocker_reason = NULL,
                 next_action = 'start_plan',
                 updated_at = ?4
             WHERE card_id = ?1 AND revision = ?2",
            params![card_id, input.expected_revision, next_revision, now],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_on_approve_spec",
            source,
        })?;
    check_cas_updated(conn, card_id, input.expected_revision, updated_rows)?;
    restore_remaining_blocker_on_return(conn, card_id, "plan")?;

    let next_stage = "plan";

    insert_revision_row(
        conn,
        &NewRevision {
            revision_id,
            card_id,
            revision: next_revision,
            stage: next_stage,
            kind: "spec_approved",
            summary: "Approved specification",
            spec_digest: Some(&spec_digest),
            payload_json: "{}",
            now,
        },
    )?;

    let detail = load_card_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}

/// Marks active `'valid'` or `'historical'` `'spec'`, `'plan'`, and `'delivery'` approvals,
/// active challenges, and evaluated evidence `'stale'` inside a write transaction.
pub fn invalidate_spec_approval(
    conn: &Connection,
    card_id: &str,
    reason: &str,
    now_iso: &str,
) -> Result<bool, WorkflowBoardRepoError> {
    let card_id = require_non_empty(card_id, "card_id")?;
    let reason = require_non_empty(reason, "reason")?;
    let now = require_non_empty(now_iso, "now_iso")?;

    let tx = WriteTx::begin(conn)?;
    let _card = load_card_row(conn, card_id)?;
    let updated =
        stale_approvals_for_gates(conn, card_id, &["spec", "plan", "delivery"], reason, now)?;
    stale_active_challenges(conn, card_id, now)?;
    stale_downstream_evidence(conn, card_id, reason, now)?;
    tx.commit()?;
    Ok(updated > 0)
}
