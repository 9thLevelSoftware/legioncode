//! Bounded challenge (`challenge_step`) repository inside `WriteTx` with strict CAS.

use rusqlite::{Connection, OptionalExtension as _, params};

use super::cards::load_card_row;
use super::challenge_validation::{
    ChallengeTransition, parse_prior_concerns, resolve_challenge_transition, validate_concerns,
};
use super::invalidation::{RevisionWrite, insert_revision_ledger};
use super::planning_queries::load_card_planning_detail_inner;
use super::types_m2::{CardPlanningDetail, ChallengeStepInput};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

struct PersistChallengeCtx<'a> {
    existing: &'a super::CardRow,
    input: &'a ChallengeStepInput<'a>,
    row_id: &'a str,
    round: i64,
    transition: &'a ChallengeTransition,
    concerns_json: &'a str,
    revision_id: &'a str,
    action: &'a str,
    now: &'a str,
}

fn persist_challenge_and_bump_card(
    conn: &Connection,
    ctx: &PersistChallengeCtx<'_>,
) -> Result<(), WorkflowBoardRepoError> {
    let existing = ctx.existing;
    let input = ctx.input;
    let transition = ctx.transition;
    let now = ctx.now;
    let spec_digest = input
        .spec_digest
        .or(existing.spec_digest.as_deref())
        .unwrap_or("");
    let interview_digest = input.interview_digest.unwrap_or("");
    let source_refs_digest = input.source_refs_digest.unwrap_or("");
    let clear_completed = i64::from(ctx.action == "start");

    conn.execute(
        "INSERT INTO workflow_board_challenges (
           id, card_id, round, status, spec_digest, interview_digest, source_refs_digest,
           concerns_json, synthesis_count, synthesis_diff_json, manual_fallback_json,
           created_at, updated_at, completed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13)
         ON CONFLICT(card_id, round) DO UPDATE SET
           status = excluded.status,
           spec_digest = excluded.spec_digest,
           interview_digest = CASE WHEN excluded.interview_digest != '' THEN excluded.interview_digest ELSE workflow_board_challenges.interview_digest END,
           source_refs_digest = CASE WHEN excluded.source_refs_digest != '' THEN excluded.source_refs_digest ELSE workflow_board_challenges.source_refs_digest END,
           concerns_json = excluded.concerns_json,
           synthesis_count = excluded.synthesis_count,
           synthesis_diff_json = COALESCE(excluded.synthesis_diff_json, workflow_board_challenges.synthesis_diff_json),
           manual_fallback_json = COALESCE(excluded.manual_fallback_json, workflow_board_challenges.manual_fallback_json),
           updated_at = excluded.updated_at,
           completed_at = CASE
             WHEN excluded.completed_at IS NOT NULL THEN excluded.completed_at
             WHEN ?14 = 1 THEN NULL
             ELSE workflow_board_challenges.completed_at
           END",
        params![
            ctx.row_id,
            existing.card_id,
            ctx.round,
            transition.next_status,
            spec_digest,
            interview_digest,
            source_refs_digest,
            ctx.concerns_json,
            transition.next_synth_count,
            input.synthesis_diff_json,
            transition.manual_fallback_json,
            now,
            transition.completed_at,
            clear_completed,
        ],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "upsert_challenge_row",
        source,
    })?;

    let next_revision = existing.revision + 1;
    let updated = conn
        .execute(
            "UPDATE workflow_board_cards
             SET revision = ?3,
                 updated_at = ?4
             WHERE card_id = ?1 AND revision = ?2",
            params![
                existing.card_id,
                input.expected_revision,
                next_revision,
                now
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "update_card_on_challenge_step",
            source,
        })?;
    if updated == 0 {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: existing.card_id.clone(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }

    insert_revision_ledger(
        conn,
        &RevisionWrite {
            revision_id: ctx.revision_id,
            card_id: &existing.card_id,
            revision: next_revision,
            stage: &existing.stage,
            kind: "challenge_step",
            summary: &format!("Challenge step ({})", ctx.action),
            spec_digest: existing.spec_digest.as_deref(),
            plan_digest: existing.plan_digest.as_deref(),
            payload_json: ctx.concerns_json,
            now,
        },
    )
}

fn resolve_challenge_round(
    conn: &Connection,
    card_id: &str,
    challenge_id: &str,
) -> Result<(String, i64, i64, Option<String>), WorkflowBoardRepoError> {
    let prior: Option<(String, i64, String, i64, String)> = conn
        .query_row(
            "SELECT id, round, status, synthesis_count, concerns_json
             FROM workflow_board_challenges
             WHERE card_id = ?1
             ORDER BY round DESC
             LIMIT 1",
            params![card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "load_prior_challenge",
            source,
        })?;

    match prior {
        Some((_id, round, status, _count, _concerns)) if status == "stale" => {
            let next_round = round + 1;
            let exists_same_id: bool = conn
                .query_row(
                    "SELECT 1 FROM workflow_board_challenges WHERE id = ?1 LIMIT 1",
                    params![challenge_id],
                    |_| Ok(true),
                )
                .optional()
                .map_err(|source| WorkflowBoardRepoError::Sqlite {
                    operation: "check_challenge_id_collision",
                    source,
                })?
                .unwrap_or(false);
            let next_id = if exists_same_id {
                format!("{challenge_id}-r{next_round}")
            } else {
                challenge_id.to_owned()
            };
            Ok((next_id, next_round, 0, None))
        }
        Some((id, round, _status, count, concerns)) => Ok((id, round, count, Some(concerns))),
        None => Ok((challenge_id.to_owned(), 1, 0, None)),
    }
}

/// Persists a bounded challenge step (`0..=3` concerns, max 1 synthesis pass, or `"I acknowledge"`
/// 3-question manual fallback) inside a strict CAS `WriteTx`, never self-approving the spec.
pub fn challenge_step(
    conn: &Connection,
    input: &ChallengeStepInput<'_>,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRepoError> {
    if input.approved == Some(true) {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "challenge_step cannot self-approve spec".to_owned(),
        ));
    }
    let card_id = require_non_empty(input.card_id, "card_id")?;
    let revision_id = require_non_empty(input.revision_id, "revision_id")?;
    let challenge_id = require_non_empty(input.challenge_id, "challenge_id")?;
    let action = require_non_empty(input.action, "action")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    if input.expected_revision < 1 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "expected_revision must be >= 1".to_owned(),
        ));
    }
    if !matches!(
        action,
        "start" | "resolve" | "synthesize" | "complete" | "manualFallback"
    ) {
        return Err(WorkflowBoardRepoError::InvalidInput(format!(
            "unsupported challenge action `{action}`"
        )));
    }

    let incoming_concerns = validate_concerns(input.concerns)?;
    let tx = WriteTx::begin(conn)?;
    let existing = load_card_row(conn, card_id)?;
    if existing.revision != input.expected_revision {
        return Err(WorkflowBoardRepoError::RevisionConflict {
            card_id: card_id.to_owned(),
            expected_revision: input.expected_revision,
            actual_revision: existing.revision,
        });
    }

    let (row_id, target_round, prior_synth_count, prior_concerns_json) =
        resolve_challenge_round(conn, card_id, challenge_id)?;

    let effective_concerns = if input.concerns.is_empty() {
        match prior_concerns_json.as_deref() {
            Some(raw) => parse_prior_concerns(raw)?,
            None => Vec::new(),
        }
    } else {
        incoming_concerns.clone()
    };

    let verification_concerns = if input.concerns.is_empty() && prior_concerns_json.is_some() {
        &effective_concerns
    } else {
        &incoming_concerns
    };
    let transition = resolve_challenge_transition(
        action,
        input,
        prior_synth_count,
        prior_concerns_json.as_deref(),
        verification_concerns,
        &effective_concerns,
        now,
    )?;

    let concerns_json = if input.concerns.is_empty() && prior_concerns_json.is_some() {
        prior_concerns_json.unwrap_or_else(|| "[]".to_owned())
    } else {
        serde_json::to_string(&effective_concerns).map_err(|source| {
            WorkflowBoardRepoError::Json {
                operation: "serialize_challenge_concerns",
                source,
            }
        })?
    };

    persist_challenge_and_bump_card(
        conn,
        &PersistChallengeCtx {
            existing: &existing,
            input,
            row_id: &row_id,
            round: target_round,
            transition: &transition,
            concerns_json: &concerns_json,
            revision_id,
            action,
            now,
        },
    )?;

    let detail = load_card_planning_detail_inner(conn, card_id)?;
    tx.commit()?;
    Ok(detail)
}
