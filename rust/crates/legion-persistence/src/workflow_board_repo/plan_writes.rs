//! SQLite upsert/replace persistence helpers for M2 plan designs, tasks, checks, and AC evidence.

use rusqlite::{Connection, params};
use serde_json::json;

use super::WorkflowBoardRepoError;
use super::invalidation::compute_prefixed_sha256;
pub(crate) use super::plan_task_writes::replace_tasks;
use super::types_m2::{
    AcMappingInput, AlternativeInput, AlternativeSelectionUpdate, LensInput, PlanDesignRow,
    PlannedCheckInput, UpdatePlanInput,
};

fn serialize_lenses_and_alts(
    prior: Option<&PlanDesignRow>,
    input: &UpdatePlanInput<'_>,
) -> Result<(String, String), WorkflowBoardRepoError> {
    let lenses_json = match input.lenses {
        Some(lenses) => {
            let vals: Vec<_> = lenses
                .iter()
                .map(|l| {
                    json!({
                        "lens": l.lens.trim(),
                        "applicable": l.applicable,
                        "summary": l.summary.map(str::trim),
                        "reason": l.reason.map(str::trim),
                    })
                })
                .collect();
            serde_json::to_string(&vals).map_err(|source| WorkflowBoardRepoError::Json {
                operation: "serialize_lenses",
                source,
            })?
        }
        None => prior.map_or_else(|| "[]".to_owned(), |p| p.lenses_json.clone()),
    };

    let alts_json = match input.alternatives {
        Some(alts) => {
            let vals: Vec<_> = alts
                .iter()
                .map(|a: &AlternativeInput<'_>| {
                    json!({
                        "id": a.id.trim(),
                        "title": a.title.trim(),
                        "summary": a.summary.trim(),
                        "tradeoffs": a.tradeoffs.trim(),
                    })
                })
                .collect();
            serde_json::to_string(&vals).map_err(|source| WorkflowBoardRepoError::Json {
                operation: "serialize_alternatives",
                source,
            })?
        }
        None => prior.map_or_else(|| "[]".to_owned(), |p| p.alternatives_json.clone()),
    };
    Ok((lenses_json, alts_json))
}

pub(crate) fn upsert_plan_design(
    conn: &Connection,
    card_id: &str,
    prior: Option<&PlanDesignRow>,
    input: &UpdatePlanInput<'_>,
    now: &str,
) -> Result<(String, Option<String>), WorkflowBoardRepoError> {
    let (lenses_json, alts_json) = serialize_lenses_and_alts(prior, input)?;

    let kept_matches_alternatives = |id: &str| match input.alternatives {
        Some(alts) => alts.iter().any(|a| a.id.trim() == id.trim()),
        None => true,
    };
    let next_selected = match input.selected_alternative_id {
        AlternativeSelectionUpdate::Keep => prior
            .and_then(|p| p.selected_alternative_id.as_deref())
            .filter(|id| kept_matches_alternatives(id))
            .map(str::to_owned),
        AlternativeSelectionUpdate::Clear => None,
        AlternativeSelectionUpdate::Set(id) => Some(id.trim().to_owned()),
    };
    let next_rationale = if next_selected.is_none() {
        None
    } else {
        match input.selected_alternative_id {
            AlternativeSelectionUpdate::Clear => None,
            _ => input
                .selection_rationale
                .map(|s| s.trim().to_owned())
                .or_else(|| prior.and_then(|p| p.selection_rationale.clone())),
        }
    };
    let mut next_comp_rev = prior.map_or(0, |p| p.comparison_revision_count);
    if input.revise_alternatives_feedback.is_some() {
        next_comp_rev += 1;
    }

    let next_strategy = input
        .strategy
        .map(|s| s.trim().to_owned())
        .or_else(|| prior.map(|p| p.strategy.clone()))
        .unwrap_or_else(|| "outcomes".to_owned());
    let next_strategy_rationale = input
        .strategy_rationale
        .map(|s| s.trim().to_owned())
        .or_else(|| prior.and_then(|p| p.strategy_rationale.clone()));
    let next_strategy_meta = input
        .strategy_metadata_json
        .map(str::to_owned)
        .or_else(|| prior.map(|p| p.strategy_metadata_json.clone()))
        .unwrap_or_else(|| "{}".to_owned());
    let next_granularity = input
        .granularity
        .map(|g| g.trim().to_owned())
        .or_else(|| prior.map(|p| p.granularity.clone()))
        .unwrap_or_else(|| "balanced".to_owned());

    let design_payload = format!(
        "{lenses_json}\n{alts_json}\n{}\n{}\n{next_strategy}\n{next_granularity}",
        next_selected.as_deref().unwrap_or(""),
        next_rationale.as_deref().unwrap_or("")
    );
    let next_design_digest = input.design_digest.map_or_else(
        || compute_prefixed_sha256(design_payload.as_bytes()),
        str::to_owned,
    );

    conn.execute(
        "INSERT INTO workflow_board_plan_designs (
           card_id, lenses_json, alternatives_json, selected_alternative_id,
           selection_rationale, comparison_revision_count, strategy, strategy_rationale,
           strategy_metadata_json, granularity, design_digest, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(card_id) DO UPDATE SET
           lenses_json = excluded.lenses_json,
           alternatives_json = excluded.alternatives_json,
           selected_alternative_id = excluded.selected_alternative_id,
           selection_rationale = excluded.selection_rationale,
           comparison_revision_count = excluded.comparison_revision_count,
           strategy = excluded.strategy,
           strategy_rationale = excluded.strategy_rationale,
           strategy_metadata_json = excluded.strategy_metadata_json,
           granularity = excluded.granularity,
           design_digest = excluded.design_digest,
           updated_at = excluded.updated_at",
        params![
            card_id,
            lenses_json,
            alts_json,
            next_selected,
            next_rationale,
            next_comp_rev,
            next_strategy,
            next_strategy_rationale,
            next_strategy_meta,
            next_granularity,
            next_design_digest,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "upsert_plan_design",
        source,
    })?;

    let _ = None::<&LensInput<'_>>;
    Ok((next_design_digest, Some(alts_json)))
}

pub(crate) fn replace_planned_checks(
    conn: &Connection,
    card_id: &str,
    checks: &[PlannedCheckInput<'_>],
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "DELETE FROM workflow_board_planned_checks WHERE card_id = ?1",
        params![card_id],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "delete_prior_checks",
        source,
    })?;

    for ch in checks {
        let argv_json =
            serde_json::to_string(ch.argv).map_err(|source| WorkflowBoardRepoError::Json {
                operation: "serialize_check_argv",
                source,
            })?;
        conn.execute(
            "INSERT INTO workflow_board_planned_checks (
               card_id, check_id, owner_task_id, argv_json, label, status, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            params![
                card_id,
                ch.check_id.trim(),
                ch.owner_task_id.map(str::trim),
                argv_json,
                ch.label.map(str::trim),
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "insert_planned_check",
            source,
        })?;
    }
    Ok(())
}

pub(crate) fn replace_ac_evidence(
    conn: &Connection,
    card_id: &str,
    mappings: &[AcMappingInput<'_>],
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "DELETE FROM workflow_board_ac_evidence WHERE card_id = ?1",
        params![card_id],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "delete_prior_ac_evidence",
        source,
    })?;

    for m in mappings {
        let check_ids_json =
            serde_json::to_string(m.check_ids).map_err(|source| WorkflowBoardRepoError::Json {
                operation: "serialize_ac_check_ids",
                source,
            })?;
        let command_argv_json = m
            .command_argv
            .map(serde_json::to_string)
            .transpose()
            .map_err(|source| WorkflowBoardRepoError::Json {
                operation: "serialize_ac_command_argv",
                source,
            })?;
        conn.execute(
            "INSERT INTO workflow_board_ac_evidence (
               card_id, criterion_id, method, check_ids_json, command_argv_json,
               procedure, status, evidence_ref, external_source, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9)",
            params![
                card_id,
                m.criterion_id.trim(),
                m.method.trim(),
                check_ids_json,
                command_argv_json,
                m.procedure.map(str::trim),
                m.evidence_ref.map(str::trim),
                m.external_source.map(str::trim),
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "insert_ac_evidence",
            source,
        })?;
    }
    Ok(())
}
