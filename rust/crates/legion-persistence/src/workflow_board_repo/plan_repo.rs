//! Plan authoring (`update_plan`) inside strict CAS `WriteTx`.

use rusqlite::{Connection, params};
use serde_json::Value;

use super::cards::load_card_row;
use super::invalidation::{
    RevisionWrite, TaskDigestFields, compute_aggregate_tasks_digest, compute_prefixed_sha256,
    compute_task_contract_digest, insert_revision_ledger, stale_approvals_for_gates,
    stale_downstream_evidence,
};
use super::plan_readiness::extract_spec_ac_ids;
use super::plan_return::{reset_in_progress_tasks_to_todo, restore_remaining_blocker_on_return};
use super::plan_validation::{
    validate_alternatives, validate_checks_and_ac_mappings, validate_granularity, validate_lenses,
    validate_strategy, validate_tasks_and_dag,
};
use super::plan_writes::{
    replace_ac_evidence, replace_planned_checks, replace_tasks, upsert_plan_design,
};
use super::planning_queries::load_card_planning_detail_inner;
use super::types_m2::{
    AlternativeSelectionUpdate, CardPlanningDetail, PlannedCheckRow, UpdatePlanInput,
};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

pub use super::plan_approve::approve_plan;
pub use super::plan_return::return_for_revision;

fn validate_update_plan_sections(
    input: &UpdatePlanInput<'_>,
    prior_design: Option<&super::types_m2::PlanDesignRow>,
    spec_ac_ids: &[String],
    persisted_checks: &[PlannedCheckRow],
) -> Result<(), WorkflowBoardRepoError> {
    if let Some(lenses) = input.lenses {
        validate_lenses(lenses)?;
    }
    if let Some(alts) = input.alternatives {
        validate_alternatives(alts)?;
    }
    if input.revise_alternatives_feedback.is_some()
        && prior_design.is_some_and(|p| p.comparison_revision_count >= 1)
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "alternatives comparison can be revised at most once".to_owned(),
        ));
    }
    if let AlternativeSelectionUpdate::Set(selected) = input.selected_alternative_id {
        let sel_trimmed = require_non_empty(selected, "selected_alternative_id")?;
        let effective_rationale = input
            .selection_rationale
            .or_else(|| prior_design.and_then(|p| p.selection_rationale.as_deref()));
        if effective_rationale.map(str::trim).is_none_or(str::is_empty) {
            return Err(WorkflowBoardRepoError::InvalidInput(
                "selected_alternative_id requires non-empty selection_rationale".to_owned(),
            ));
        }
        let matches_alt = match input.alternatives {
            Some(alts) => alts.iter().any(|a| a.id.trim() == sel_trimmed),
            None => prior_design.is_some_and(|p| {
                serde_json::from_str::<Vec<Value>>(&p.alternatives_json)
                    .unwrap_or_default()
                    .iter()
                    .any(|v| {
                        v.get("id").and_then(Value::as_str).map(str::trim) == Some(sel_trimmed)
                    })
            }),
        };
        if !matches_alt {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "selected_alternative_id `{sel_trimmed}` does not match compared alternatives"
            )));
        }
    }
    if input.strategy.is_some()
        || input.strategy_rationale.is_some()
        || input.strategy_metadata_json.is_some()
    {
        let effective_strategy = input
            .strategy
            .or_else(|| prior_design.map(|p| p.strategy.as_str()))
            .unwrap_or("outcomes");
        let effective_rationale = input
            .strategy_rationale
            .or_else(|| prior_design.and_then(|p| p.strategy_rationale.as_deref()));
        let effective_meta = input
            .strategy_metadata_json
            .or_else(|| prior_design.map(|p| p.strategy_metadata_json.as_str()));
        validate_strategy(effective_strategy, effective_rationale, effective_meta)?;
    }
    if let Some(granularity) = input.granularity {
        validate_granularity(granularity)?;
    }
    if let Some(tasks) = input.tasks {
        validate_tasks_and_dag(tasks)?;
    }
    validate_checks_and_ac_mappings(
        input.checks,
        input.ac_mappings,
        spec_ac_ids,
        persisted_checks,
    )?;
    Ok(())
}

fn compute_effective_tasks_digest(
    next_tasks_digest: Option<String>,
    before: &CardPlanningDetail,
) -> Option<String> {
    next_tasks_digest.or_else(|| {
        if before.tasks.is_empty() {
            None
        } else {
            let digests: Vec<String> = before
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
            Some(compute_aggregate_tasks_digest(
                digests.iter().map(String::as_str),
            ))
        }
    })
}

struct PlanCardBump<'a> {
    existing: &'a super::CardRow,
    expected_revision: i64,
    next_stage: &'a str,
    next_action: Option<&'a str>,
    clear_current_task: bool,
    design_digest: &'a str,
    effective_tasks_digest: Option<&'a str>,
    next_plan_digest: &'a str,
    next_plan_json: Option<&'a str>,
    now: &'a str,
}

fn bump_card_after_plan_update(
    conn: &Connection,
    bump: &PlanCardBump<'_>,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = bump.existing.card_id.as_str();
    let next_revision = bump.existing.revision + 1;
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 stage = ?4,
                 status = 'active',
                 completed_at = NULL,
                 paused_from_status = NULL,
                 pause_reason = NULL,
                 paused_at = NULL,
                 blocker_reason = NULL,
                 current_task_id = CASE WHEN ?11 THEN NULL ELSE current_task_id END,
                 next_action = ?5,
                 plan_digest = ?6,
                 design_digest = ?7,
                 tasks_digest = COALESCE(?8, tasks_digest),
                 plan_json = ?9,
                 updated_at = ?10
             WHERE card_id = ?1 AND revision = ?2",
            params![
                card_id,
                bump.expected_revision,
                next_revision,
                bump.next_stage,
                bump.next_action,
                bump.next_plan_digest,
                bump.design_digest,
                bump.effective_tasks_digest,
                bump.next_plan_json,
                bump.now,
                bump.clear_current_task,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_on_update_plan",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: bump.expected_revision,
            actual_revision: bump.existing.revision,
        });
    }
    Ok(())
}

fn persist_plan_sections_and_bump_card(
    conn: &Connection,
    existing: &super::CardRow,
    before: &CardPlanningDetail,
    input: &UpdatePlanInput<'_>,
    design_digest: &str,
    revision_id: &str,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let card_id = existing.card_id.as_str();
    let next_tasks_digest = match input.tasks {
        Some(tasks) => Some(replace_tasks(conn, card_id, tasks, now)?),
        None => None,
    };
    if let Some(checks) = input.checks {
        replace_planned_checks(conn, card_id, checks, now)?;
    }
    if let Some(mappings) = input.ac_mappings {
        replace_ac_evidence(conn, card_id, mappings, now)?;
    }

    let effective_tasks_digest = compute_effective_tasks_digest(next_tasks_digest, before);
    let next_plan_json = input
        .plan_json
        .map(str::to_owned)
        .or_else(|| existing.plan_json.clone());
    let next_plan_digest = input.plan_digest.map_or_else(
        || {
            compute_prefixed_sha256(
                format!(
                    "{design_digest}\n{}\n{}",
                    effective_tasks_digest.as_deref().unwrap_or(""),
                    next_plan_json.as_deref().unwrap_or("")
                )
                .as_bytes(),
            )
        },
        str::to_owned,
    );

    let next_revision = existing.revision + 1;
    let regressing = matches!(existing.stage.as_str(), "execute" | "ship");
    let next_stage = if regressing {
        "plan"
    } else {
        existing.stage.as_str()
    };
    let next_action = if regressing {
        Some("revise_plan")
    } else if existing.status != "active" {
        Some(if existing.stage == "spec" {
            "revise_spec"
        } else {
            "revise_plan"
        })
    } else {
        existing.next_action.as_deref()
    };
    bump_card_after_plan_update(
        conn,
        &PlanCardBump {
            existing,
            expected_revision: input.expected_revision,
            next_stage,
            next_action,
            clear_current_task: regressing || input.tasks.is_some(),
            design_digest,
            effective_tasks_digest: effective_tasks_digest.as_deref(),
            next_plan_digest: &next_plan_digest,
            next_plan_json: next_plan_json.as_deref(),
            now,
        },
    )?;
    if regressing && input.tasks.is_none() {
        reset_in_progress_tasks_to_todo(conn, card_id, now)?;
    }

    stale_approvals_for_gates(conn, card_id, &["plan", "delivery"], "plan_updated", now)?;
    stale_downstream_evidence(conn, card_id, "plan_updated", now)?;
    restore_remaining_blocker_on_return(conn, card_id, next_stage)?;

    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id,
            card_id,
            revision: next_revision,
            stage: next_stage,
            kind: "plan_updated",
            summary: "Updated plan",
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: Some(&next_plan_digest),
            payload_json: next_plan_json.as_deref().unwrap_or("{}"),
            now,
        },
    )
}

/// Persists partial or complete Plan sections in a single CAS `WriteTx`, invalidating downstream
/// `'plan'` and `'delivery'` approvals and evaluated evidence while **preserving `'spec'` approval**.
pub fn update_plan(
    conn: &Connection,
    input: &UpdatePlanInput<'_>,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
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

    let before = load_card_planning_detail_inner(conn, card_id)?;
    let spec_ac_ids = extract_spec_ac_ids(&existing.spec_json);
    validate_update_plan_sections(
        input,
        before.plan_design.as_ref(),
        &spec_ac_ids,
        &before.planned_checks,
    )?;

    let (design_digest, _) =
        upsert_plan_design(conn, card_id, before.plan_design.as_ref(), input, now)?;
    persist_plan_sections_and_bump_card(
        conn,
        &existing,
        &before,
        input,
        &design_digest,
        revision_id,
        now,
    )?;

    let detail = load_card_planning_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}
