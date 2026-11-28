//! Approval readiness checks for M2 Spec and Plan gates.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::Value;

use super::WorkflowBoardRepoError;
use super::plan_validation::{ALL_SIX_LENSES, validate_strategy};
use super::types_m2::{CardPlanningDetail, PlanDesignRow};

pub(crate) fn extract_spec_ac_ids(spec_json: &str) -> Vec<String> {
    let Ok(val) = serde_json::from_str::<Value>(spec_json) else {
        return Vec::new();
    };
    val.get("acceptanceCriteria")
        .or_else(|| val.get("ac"))
        .and_then(Value::as_array)
        .map_or_default(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .or_else(|| item.as_str())
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                })
                .collect()
        })
}

/// Validates the current Spec before recording an approval.
pub fn validate_spec_approval_m2_readiness(
    conn: &Connection,
    card_id: &str,
    spec_json: &str,
) -> Result<(), WorkflowBoardRepoError> {
    if extract_spec_ac_ids(spec_json).is_empty() {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "spec approval requires at least one acceptance criterion".to_owned(),
        ));
    }
    let unresolved_decisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workflow_board_decisions
             WHERE card_id = ?1 AND status = 'unresolved'",
            params![card_id],
            |r| r.get(0),
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "count_unresolved_decisions",
            source,
        })?;
    if unresolved_decisions > 0 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "spec approval blocked while exploration decisions remain unresolved".to_owned(),
        ));
    }
    let latest_challenge_status: Option<String> = conn
        .query_row(
            "SELECT status FROM workflow_board_challenges
             WHERE card_id = ?1
             ORDER BY round DESC
             LIMIT 1",
            params![card_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_latest_challenge_status",
            source,
        })?;
    if let Some(status @ ("pending" | "in_progress" | "stale")) = latest_challenge_status.as_deref()
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
            "Spec approval blocked while challenge status is `{status}`"
        )));
    }
    Ok(())
}

fn validate_persisted_design_readiness(
    design: &PlanDesignRow,
) -> Result<(), WorkflowBoardRepoError> {
    validate_strategy(
        &design.strategy,
        design.strategy_rationale.as_deref(),
        Some(&design.strategy_metadata_json),
    )
    .map_err(|err| match err {
        WorkflowBoardRepoError::InvalidInput(msg) => {
            WorkflowBoardRepoError::PreconditionFailed(msg)
        }
        other => other,
    })?;

    let lenses: Vec<Value> = serde_json::from_str(&design.lenses_json).map_err(|source| {
        WorkflowBoardRepoError::Json {
            operation: "parse_persisted_lenses",
            source,
        }
    })?;
    let present_lenses: HashSet<String> = lenses
        .iter()
        .filter_map(|v| {
            let lens = v.get("lens").and_then(Value::as_str)?.trim().to_owned();
            let applicable = v.get("applicable").and_then(Value::as_bool)?;
            if applicable {
                let ok = v
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty());
                ok.then_some(lens)
            } else {
                let ok = v
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty());
                ok.then_some(lens)
            }
        })
        .collect();
    for required in ALL_SIX_LENSES {
        if !present_lenses.contains(*required) {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "plan approval requires completed `{required}` design lens"
            )));
        }
    }

    let alts: Vec<Value> = serde_json::from_str(&design.alternatives_json).map_err(|source| {
        WorkflowBoardRepoError::Json {
            operation: "parse_persisted_alternatives",
            source,
        }
    })?;
    if alts.len() != 2 {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "plan approval requires 2 compared design alternatives".to_owned(),
        ));
    }
    let selected = design
        .selected_alternative_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            WorkflowBoardRepoError::PreconditionFailed(
                "plan approval requires a selected alternative".to_owned(),
            )
        })?;
    if !alts
        .iter()
        .any(|a| a.get("id").and_then(Value::as_str).map(str::trim) == Some(selected))
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "selected alternative does not match compared pair".to_owned(),
        ));
    }
    if design
        .selection_rationale
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "plan approval requires non-empty selection rationale".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_plan_approval_readiness(
    current: &CardPlanningDetail,
    spec_ac_ids: &[String],
) -> Result<(), WorkflowBoardRepoError> {
    if spec_ac_ids.is_empty() {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "plan approval requires at least one acceptance criterion in the spec".to_owned(),
        ));
    }
    let design = current.plan_design.as_ref().ok_or_else(|| {
        WorkflowBoardRepoError::PreconditionFailed(
            "plan design (lenses, alternatives, strategy) is missing".to_owned(),
        )
    })?;
    validate_persisted_design_readiness(design)?;
    if current.tasks.is_empty() {
        return Err(WorkflowBoardRepoError::PreconditionFailed(
            "plan approval requires at least one task contract".to_owned(),
        ));
    }
    let persisted_check_ids: HashSet<&str> = current
        .planned_checks
        .iter()
        .map(|c| c.check_id.trim())
        .collect();
    let mut mapped_ac_ids: HashSet<String> = HashSet::new();
    for ac in &current.ac_evidence {
        mapped_ac_ids.insert(ac.criterion_id.clone());
        if ac.method.trim() == "automated" {
            let check_ids: Vec<String> =
                serde_json::from_str(&ac.check_ids_json).map_err(|source| {
                    WorkflowBoardRepoError::Json {
                        operation: "parse_ac_check_ids",
                        source,
                    }
                })?;
            if ac.command_argv_json.is_none() && check_ids.is_empty() {
                return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                    "automated AC `{}` requires command or checkIds",
                    ac.criterion_id
                )));
            }
            for cid in &check_ids {
                let trimmed = cid.trim();
                if trimmed.is_empty() || !persisted_check_ids.contains(trimmed) {
                    return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                        "automated AC `{}` references unknown planned check `{trimmed}`",
                        ac.criterion_id
                    )));
                }
            }
        }
    }
    for ac_id in spec_ac_ids {
        if !mapped_ac_ids.contains(ac_id) {
            return Err(WorkflowBoardRepoError::PreconditionFailed(format!(
                "plan approval missing AC evidence mapping for `{ac_id}`"
            )));
        }
    }
    Ok(())
}
