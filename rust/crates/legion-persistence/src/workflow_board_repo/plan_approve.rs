//! Plan approval (`approve_plan`) inside strict CAS `WriteTx`.

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;

use super::blocker_status::compute_card_blocker_status;
use super::cards::load_card_row;
use super::invalidation::{
    RevisionWrite, TaskDigestFields, compute_aggregate_tasks_digest, compute_task_contract_digest,
    insert_revision_ledger,
};
use super::plan_readiness::{extract_spec_ac_ids, validate_plan_approval_readiness};
use super::planning_queries::load_card_planning_detail_inner;
use super::types_m2::{ApprovePlanInput, CardPlanningDetail};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

fn verify_valid_spec_and_plan_digests(
    conn: &Connection,
    existing: &super::CardRow,
    input: &ApprovePlanInput<'_>,
) -> Result<(String, String), WorkflowBoardRepoError> {
    let spec_approval_digest: Option<String> = conn
        .query_row(
            "SELECT spec_digest FROM workflow_board_approvals
             WHERE card_id = ?1 AND gate = 'spec' AND status = 'valid'
             LIMIT 1",
            params![existing.card_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "check_valid_spec_approval",
            source,
        })?
        .flatten();
    let spec_digest = spec_approval_digest.ok_or_else(|| {
        WorkflowBoardRepoError::PreconditionFailed(
            "cannot approve plan without a valid spec approval".to_owned(),
        )
    })?;

    if let Some(exp_spec) = input.expected_spec_digest {
        let trimmed = require_non_empty(exp_spec, "expected_spec_digest")?;
        if trimmed != spec_digest {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "spec digest mismatch: expected `{trimmed}`, actual `{spec_digest}`"
            )));
        }
    }

    let plan_digest = existing
        .plan_digest
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            WorkflowBoardRepoError::PreconditionFailed(
                "cannot approve plan without non-empty plan_digest".to_owned(),
            )
        })?
        .to_owned();
    if let Some(exp_plan) = input.expected_plan_digest {
        let trimmed = require_non_empty(exp_plan, "expected_plan_digest")?;
        if trimmed != plan_digest {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "plan digest mismatch: expected `{trimmed}`, actual `{plan_digest}`"
            )));
        }
    }
    Ok((spec_digest, plan_digest))
}

struct PersistPlanApprovalCtx<'a> {
    existing: &'a super::CardRow,
    current: &'a CardPlanningDetail,
    spec_ac_ids: &'a [String],
    spec_digest: &'a str,
    plan_digest: &'a str,
    approval_id: &'a str,
    approved_by: &'a str,
    revision_id: &'a str,
    expected_revision: i64,
    now: &'a str,
}

fn persist_plan_approval_and_advance(
    conn: &Connection,
    ctx: &PersistPlanApprovalCtx<'_>,
) -> Result<(), WorkflowBoardRepoError> {
    let existing = ctx.existing;
    let current = ctx.current;
    let task_ids: Vec<&str> = current.tasks.iter().map(|t| t.task_id.as_str()).collect();
    let check_ids: Vec<&str> = current
        .planned_checks
        .iter()
        .map(|c| c.check_id.as_str())
        .collect();
    let task_contract_digests: Vec<String> = current
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
    let check_digests: Vec<String> = current
        .planned_checks
        .iter()
        .map(|c| format!("{}\0{}", c.check_id, c.argv_json))
        .collect();
    let checks_digest = compute_aggregate_tasks_digest(check_digests.iter().map(String::as_str));
    let metadata_json = serde_json::to_string(&json!({
        "taskIds": task_ids,
        "checkIds": check_ids,
        "acceptanceIds": ctx.spec_ac_ids,
        "checksDigest": checks_digest,
    }))
    .map_err(|source| WorkflowBoardRepoError::Json {
        operation: "serialize_plan_approval_metadata",
        source,
    })?;

    let next_revision = existing.revision + 1;
    conn.execute(
        "UPDATE workflow_board_approvals
         SET status = 'superseded',
             invalidated_at = ?2,
             invalidation_reason = 'superseded_by_new_approval'
         WHERE card_id = ?1 AND gate = 'plan' AND status = 'valid'",
        params![existing.card_id, ctx.now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "supersede_prior_plan_approval",
        source,
    })?;

    conn.execute(
        "INSERT INTO workflow_board_approvals (
           id, card_id, gate, status, revision, artifact_digest, spec_digest,
           plan_digest, tasks_digest, approved_by, approved_at, metadata_json
         ) VALUES (?1, ?2, 'plan', 'valid', ?3, ?4, ?5, ?4, ?6, ?7, ?8, ?9)",
        params![
            ctx.approval_id,
            existing.card_id,
            next_revision,
            ctx.plan_digest,
            ctx.spec_digest,
            tasks_digest,
            ctx.approved_by,
            ctx.now,
            metadata_json,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "insert_plan_approval",
        source,
    })?;

    advance_card_after_plan_approval(conn, ctx, next_revision, &tasks_digest, &metadata_json)
}

fn advance_card_after_plan_approval(
    conn: &Connection,
    ctx: &PersistPlanApprovalCtx<'_>,
    next_revision: i64,
    tasks_digest: &str,
    metadata_json: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let existing = ctx.existing;
    let next_stage = if existing.stage == "plan" {
        "execute"
    } else {
        existing.stage.as_str()
    };
    let (next_status, next_blocker_reason, next_action) = if next_stage == "execute" {
        let blocker = compute_card_blocker_status(conn, &existing.card_id)?;
        if blocker.is_blocked {
            (
                "blocked",
                blocker.blocker_reason,
                blocker.default_next_action,
            )
        } else {
            ("active", None, "start_execute")
        }
    } else {
        (
            existing.status.as_str(),
            existing.blocker_reason.clone(),
            "start_execute",
        )
    };
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 stage = ?4,
                 status = ?5,
                 blocker_reason = ?6,
                 tasks_digest = ?7,
                 next_action = ?8,
                 updated_at = ?9
             WHERE card_id = ?1 AND revision = ?2",
            params![
                existing.card_id,
                ctx.expected_revision,
                next_revision,
                next_stage,
                next_status,
                next_blocker_reason,
                tasks_digest,
                next_action,
                ctx.now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_on_approve_plan",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: existing.card_id.clone(),
            expected_revision: ctx.expected_revision,
            actual_revision: existing.revision,
        });
    }

    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id: ctx.revision_id,
            card_id: &existing.card_id,
            revision: next_revision,
            stage: next_stage,
            kind: "plan_approved",
            summary: "Approved plan",
            spec_digest: Some(ctx.spec_digest),
            plan_digest: Some(ctx.plan_digest),
            payload_json: metadata_json,
            now: ctx.now,
        },
    )
}

/// Validates 100% Plan approval readiness and records a content-bound `'valid'` `'plan'`
/// approval receipt inside a strict CAS `WriteTx`, advancing `'plan'` -> `'execute'`.
pub fn approve_plan(
    conn: &Connection,
    input: &ApprovePlanInput<'_>,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let approval_id = require_non_empty(input.approval_id, "approval_id")?;
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
            "cannot approve plan while card status is `{}`",
            existing.status
        )));
    }

    let (spec_digest, plan_digest) = verify_valid_spec_and_plan_digests(conn, &existing, input)?;
    if existing.stage != "plan" {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot approve plan while card is in `{}` stage (expected `plan`)",
            existing.stage
        )));
    }
    let current = load_card_planning_detail_inner(conn, card_id)?;
    let spec_ac_ids = extract_spec_ac_ids(&existing.spec_json);
    validate_plan_approval_readiness(&current, &spec_ac_ids)?;

    persist_plan_approval_and_advance(
        conn,
        &PersistPlanApprovalCtx {
            existing: &existing,
            current: &current,
            spec_ac_ids: &spec_ac_ids,
            spec_digest: &spec_digest,
            plan_digest: &plan_digest,
            approval_id,
            approved_by,
            revision_id,
            expected_revision: input.expected_revision,
            now,
        },
    )?;

    let detail = load_card_planning_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}
