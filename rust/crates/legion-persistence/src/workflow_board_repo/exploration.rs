//! Guided exploration (`explore_step`) repository inside `WriteTx` with strict CAS.

use rusqlite::{Connection, OptionalExtension as _, params};

use super::cards::load_card_row;
use super::invalidation::{RevisionWrite, insert_revision_ledger, maybe_stale_on_explore};
use super::plan_return::{reset_in_progress_tasks_to_todo, restore_remaining_blocker_on_return};
use super::planning_queries::load_card_planning_detail_inner;
use super::types_m2::{CardPlanningDetail, ExploreAnswerInput, ExploreStepInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

fn validate_explore_answers(
    answers: &[ExploreAnswerInput<'_>],
) -> Result<(), WorkflowBoardRepoError> {
    if answers.len() > 3 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "explore_step accepts at most 3 decisions per round".to_owned(),
        ));
    }
    for entry in answers {
        let _id = require_non_empty(entry.decision_id, "decision_id")?;
        let action = require_non_empty(entry.action, "action")?;
        if !matches!(
            action,
            "explain" | "recommend" | "edit" | "unsure" | "pause" | "resume"
        ) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsupported explore action `{action}`"
            )));
        }
        if action == "edit" && entry.answer.map(str::trim).is_none_or(str::is_empty) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "explore `edit` for `{}` requires a non-empty answer",
                entry.decision_id.trim()
            )));
        }
    }
    Ok(())
}

fn upsert_decision_row(
    conn: &Connection,
    card_id: &str,
    round_index: i64,
    entry: &ExploreAnswerInput<'_>,
    now: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let decision_id = entry.decision_id.trim();
    let action = entry.action.trim();
    let trimmed_answer = entry
        .answer
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let next_status = match action {
        "edit" => "resolved",
        "recommend" if trimmed_answer.is_some() => "resolved",
        _ => "unresolved",
    };

    let existing_status: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT status, answer FROM workflow_board_decisions
             WHERE card_id = ?1 AND decision_id = ?2",
            params![card_id, decision_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_decision_row",
            source,
        })?;

    let (final_status, final_answer) = match (action, existing_status) {
        ("resume", Some((st, ans))) => (st, trimmed_answer.or(ans)),
        _ => (next_status.to_owned(), trimmed_answer),
    };

    conn.execute(
        "INSERT INTO workflow_board_decisions (
           card_id, decision_id, round_index, prompt, recommendation,
           last_action, status, answer, rationale, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
         ON CONFLICT(card_id, decision_id) DO UPDATE SET
           round_index = excluded.round_index,
           prompt = CASE WHEN excluded.prompt != '' THEN excluded.prompt ELSE workflow_board_decisions.prompt END,
           recommendation = COALESCE(excluded.recommendation, workflow_board_decisions.recommendation),
           last_action = excluded.last_action,
           status = excluded.status,
           answer = COALESCE(excluded.answer, workflow_board_decisions.answer),
           rationale = COALESCE(excluded.rationale, workflow_board_decisions.rationale),
           updated_at = excluded.updated_at",
        params![
            card_id,
            decision_id,
            round_index,
            entry.prompt.unwrap_or(""),
            entry.recommendation,
            action,
            final_status,
            final_answer,
            entry.rationale,
            now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "upsert_decision_row",
        source,
    })?;
    Ok(())
}

fn resolve_round_index(
    latest_round: Option<&(i64, String, bool)>,
    input: &ExploreStepInput<'_>,
) -> Result<i64, WorkflowBoardRepoError> {
    match latest_round {
        None => Ok(1),
        Some((prev_idx, _, true)) if !input.answers.is_empty() => {
            if input.continue_round != Some(true) {
                return Err(WorkflowBoardRepoError::PreconditionFailed(
                    "continue_round=true is required to advance to the next exploration round"
                        .to_owned(),
                ));
            }
            Ok(prev_idx + 1)
        }
        Some((prev_idx, _, _)) => Ok(if input.continue_round == Some(true) {
            prev_idx + 1
        } else {
            *prev_idx
        }),
    }
}

struct PersistExplorationCtx<'a> {
    existing: &'a super::CardRow,
    input: &'a ExploreStepInput<'a>,
    latest_round: Option<&'a (i64, String, bool)>,
    round_index: i64,
    is_pause: bool,
    is_resume: bool,
    staled_spec: bool,
    revision_id: &'a str,
    now: &'a str,
}

fn persist_exploration_round_and_card(
    conn: &Connection,
    ctx: &PersistExplorationCtx<'_>,
) -> Result<(), WorkflowBoardRepoError> {
    let existing = ctx.existing;
    let input = ctx.input;
    let card_id = existing.card_id.as_str();
    let decision_ids: Vec<&str> = input.answers.iter().map(|a| a.decision_id.trim()).collect();
    let decision_ids_json =
        serde_json::to_string(&decision_ids).map_err(|source| WorkflowBoardRepoError::Json {
            operation: "serialize_exploration_decision_ids",
            source,
        })?;

    let advancing_new_round = ctx
        .latest_round
        .is_none_or(|(prev_idx, _, _)| ctx.round_index > *prev_idx);
    let prior_pending = !advancing_new_round && ctx.latest_round.is_some_and(|(_, _, p)| *p);
    let pending_continue = prior_pending || (!ctx.is_pause && !input.answers.is_empty());
    let round_status = if ctx.is_pause {
        "paused"
    } else if pending_continue {
        "awaiting_continue"
    } else if ctx.is_resume {
        "in_progress"
    } else {
        ctx.latest_round
            .filter(|(prev_idx, _, _)| ctx.round_index == *prev_idx)
            .map_or("in_progress", |(_, st, _)| st.as_str())
    };
    let round_id = format!("er-{card_id}-r{}", ctx.round_index);

    conn.execute(
        "INSERT INTO workflow_board_exploration_rounds (
           id, card_id, round_index, status, decision_ids_json, pending_continue_prompt,
           created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(card_id, round_index) DO UPDATE SET
           status = excluded.status,
           decision_ids_json = CASE
             WHEN excluded.decision_ids_json != '[]' THEN excluded.decision_ids_json
             ELSE workflow_board_exploration_rounds.decision_ids_json
           END,
           pending_continue_prompt = excluded.pending_continue_prompt,
           updated_at = excluded.updated_at",
        params![
            round_id,
            card_id,
            ctx.round_index,
            round_status,
            decision_ids_json,
            i64::from(pending_continue),
            ctx.now,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "upsert_exploration_round",
        source,
    })?;

    bump_card_after_exploration(conn, ctx, &decision_ids_json)
}

fn bump_card_after_exploration(
    conn: &Connection,
    ctx: &PersistExplorationCtx<'_>,
    decision_ids_json: &str,
) -> Result<(), WorkflowBoardRepoError> {
    let existing = ctx.existing;
    let card_id = existing.card_id.as_str();
    let next_revision = existing.revision + 1;
    let regressing = existing.stage != "spec" && (ctx.staled_spec || !ctx.input.answers.is_empty());
    let clear_pause_and_blocker = regressing || (ctx.is_resume && existing.status == "paused");
    let next_stage = if regressing { "spec" } else { &existing.stage };
    let (next_card_status, next_action) = if ctx.is_pause {
        ("paused", Some("resume_exploration"))
    } else if clear_pause_and_blocker {
        ("active", Some("explore_spec"))
    } else {
        (existing.status.as_str(), existing.next_action.as_deref())
    };

    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3, stage = ?4, status = ?5,
                 completed_at = CASE WHEN ?8 THEN NULL ELSE completed_at END,
                 paused_from_status = CASE WHEN ?9 THEN NULL ELSE paused_from_status END,
                 pause_reason = CASE WHEN ?9 THEN NULL ELSE pause_reason END,
                 paused_at = CASE WHEN ?9 THEN NULL ELSE paused_at END,
                 blocker_reason = CASE WHEN ?9 THEN NULL ELSE blocker_reason END,
                 current_task_id = CASE WHEN ?8 THEN NULL ELSE current_task_id END,
                 next_action = ?6, updated_at = ?7
             WHERE card_id = ?1 AND revision = ?2",
            params![
                card_id,
                ctx.input.expected_revision,
                next_revision,
                next_stage,
                next_card_status,
                next_action,
                ctx.now,
                regressing,
                clear_pause_and_blocker,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_on_explore_step",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: ctx.input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    if regressing {
        reset_in_progress_tasks_to_todo(conn, card_id, ctx.now)?;
    }
    if clear_pause_and_blocker && !ctx.is_pause {
        restore_remaining_blocker_on_return(conn, card_id, next_stage)?;
    }

    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id: ctx.revision_id,
            card_id,
            revision: next_revision,
            stage: next_stage,
            kind: "exploration_step",
            summary: if ctx.is_pause {
                "Paused guided exploration"
            } else if ctx.is_resume {
                "Resumed guided exploration"
            } else {
                "Recorded guided exploration step"
            },
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: decision_ids_json,
            now: ctx.now,
        },
    )
}

/// Persists a guided exploration step (`<= 3` decisions/round, `continue_round` gate, pause/resume)
/// inside a strict CAS `WriteTx`, without approving the spec or mutating execution tasks.
pub fn explore_step(
    conn: &Connection,
    input: &ExploreStepInput<'_>,
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
    if let Some(action) = input.action {
        let act = require_non_empty(action, "action")?;
        if !matches!(act, "step" | "pause" | "resume") {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsupported exploreStep action `{act}`"
            )));
        }
    }
    validate_explore_answers(input.answers)?;

    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }
    let regressing_edit = existing.stage != "spec"
        && input
            .answers
            .iter()
            .any(|a| !matches!(a.action.trim(), "pause" | "resume"));
    if (existing.stage != "spec" || matches!(existing.status.as_str(), "completed" | "shipped"))
        && !regressing_edit
    {
        return Err(WorkflowBoardRepoError::InvalidStateTransition(format!(
            "cannot run non-regressing exploreStep in stage `{}` with status `{}`",
            existing.stage, existing.status
        )));
    }

    let latest_round: Option<(i64, String, bool)> = conn
        .query_row(
            "SELECT round_index, status, pending_continue_prompt
             FROM workflow_board_exploration_rounds
             WHERE card_id = ?1
             ORDER BY round_index DESC
             LIMIT 1",
            params![card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_latest_exploration_round",
            source,
        })?;

    let top_action = input.action.map(str::trim);
    let is_pause =
        top_action == Some("pause") || input.answers.iter().any(|a| a.action.trim() == "pause");
    let is_resume = top_action == Some("resume") && !is_pause;
    let round_index = resolve_round_index(latest_round.as_ref(), input)?;

    for entry in input.answers {
        upsert_decision_row(conn, card_id, round_index, entry, now)?;
    }

    let staled_spec = maybe_stale_on_explore(
        conn,
        card_id,
        !input.answers.is_empty(),
        latest_round.is_none(),
        now,
    )?;

    persist_exploration_round_and_card(
        conn,
        &PersistExplorationCtx {
            existing: &existing,
            input,
            latest_round: latest_round.as_ref(),
            round_index,
            is_pause,
            is_resume,
            staled_spec,
            revision_id,
            now,
        },
    )?;

    let detail = load_card_planning_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}
