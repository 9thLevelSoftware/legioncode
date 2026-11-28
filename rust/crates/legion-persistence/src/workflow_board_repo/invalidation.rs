//! Shared downstream approval, challenge, and evidence invalidation helpers inside `WriteTx`.

use rusqlite::{Connection, OptionalExtension as _, params};

pub(crate) use super::invalidation_digests::{
    TaskDigestFields, compute_aggregate_tasks_digest, compute_prefixed_sha256,
    compute_task_contract_digest,
};
use super::{WorkflowBoardRepoError, WriteTx, require_non_empty};

pub(crate) struct RevisionWrite<'a> {
    pub revision_id: &'a str,
    pub card_id: &'a str,
    pub revision: i64,
    pub stage: &'a str,
    pub kind: &'a str,
    pub summary: &'a str,
    pub spec_digest: Option<&'a str>,
    pub plan_digest: Option<&'a str>,
    pub payload_json: &'a str,
    pub now: &'a str,
}

pub(crate) fn insert_revision_ledger(
    conn: &Connection,
    entry: &RevisionWrite<'_>,
) -> Result<(), WorkflowBoardRepoError> {
    insert_revision_ledger_with_actor(conn, entry, "client")
}

pub(crate) fn insert_revision_ledger_with_actor(
    conn: &Connection,
    entry: &RevisionWrite<'_>,
    actor_kind: &str,
) -> Result<(), WorkflowBoardRepoError> {
    conn.execute(
        "INSERT INTO workflow_board_revisions (
           id, card_id, revision, stage, kind, actor_kind, summary, spec_digest,
           plan_digest, payload_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            entry.revision_id,
            entry.card_id,
            entry.revision,
            entry.stage,
            entry.kind,
            actor_kind,
            entry.summary,
            entry.spec_digest,
            entry.plan_digest,
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

/// Marks active `'valid'` or `'historical'` approvals in `gates` as `'stale'` inside an open transaction.
pub(crate) fn stale_approvals_for_gates(
    conn: &Connection,
    card_id: &str,
    gates: &[&str],
    reason: &str,
    now: &str,
) -> Result<usize, WorkflowBoardRepoError> {
    let mut total = 0_usize;
    for gate in gates {
        let updated = conn
            .execute(
                "UPDATE workflow_board_approvals
                 SET status = 'stale',
                     invalidated_at = ?3,
                     invalidation_reason = ?4
                 WHERE card_id = ?1
                   AND gate = ?2
                   AND status IN ('valid', 'historical')",
                params![card_id, gate, now, reason],
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "stale_approvals_for_gates",
                source,
            })?;
        total += updated;
        if *gate == "delivery" {
            conn.execute(
                "UPDATE workflow_board_delivery_receipts
                 SET status = 'stale',
                     invalidated_at = ?2,
                     invalidation_reason = ?3,
                     updated_at = ?2
                 WHERE card_id = ?1
                   AND status IN ('shipped', 'valid')",
                params![card_id, now, reason],
            )
            .map_err(|source| WorkflowBoardRepoError::Sqlite {
                operation: "stale_delivery_receipts",
                source,
            })?;
        }
    }
    Ok(total)
}

/// Marks non-stale challenge records `'stale'` when upstream spec content or stage changes.
pub(crate) fn stale_active_challenges(
    conn: &Connection,
    card_id: &str,
    now: &str,
) -> Result<usize, WorkflowBoardRepoError> {
    conn.execute(
        "UPDATE workflow_board_challenges
         SET status = 'stale',
             updated_at = ?2
         WHERE card_id = ?1
           AND status != 'stale'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "stale_active_challenges",
        source,
    })
}

/// Marks evaluated planned checks and AC evidence `'stale'` while keeping `'pending'`/`'historical'` intact.
pub(crate) fn stale_downstream_evidence(
    conn: &Connection,
    card_id: &str,
    reason: &str,
    now: &str,
) -> Result<usize, WorkflowBoardRepoError> {
    let checks = conn
        .execute(
            "UPDATE workflow_board_planned_checks
             SET status = 'stale',
                 stale_reason = ?3,
                 updated_at = ?2
             WHERE card_id = ?1
               AND status IN ('passed', 'failed')",
            params![card_id, now, reason],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "stale_planned_checks",
            source,
        })?;
    let receipts = conn
        .execute(
            "UPDATE workflow_board_check_receipts
             SET status = 'stale',
                 stale_reason = ?3,
                 invalidated_at = ?2,
                 updated_at = ?2
             WHERE card_id = ?1
               AND status IN ('passed', 'failed')",
            params![card_id, now, reason],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "stale_check_receipts",
            source,
        })?;
    let ac = conn
        .execute(
            "UPDATE workflow_board_ac_evidence
             SET status = 'stale',
                 stale_reason = ?3,
                 observation_note = COALESCE(observation_note, ?3),
                 updated_at = ?2
             WHERE card_id = ?1
               AND status IN ('pass', 'fail', 'not_applicable')",
            params![card_id, now, reason],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "stale_ac_evidence",
            source,
        })?;
    let reviews = conn
        .execute(
            "UPDATE workflow_board_review_runs
             SET status = 'stale',
                 verdict = 'stale',
                 stale_reason = ?3,
                 updated_at = ?2
             WHERE card_id = ?1
               AND status IN ('pending', 'running', 'completed', 'failed')",
            params![card_id, now, reason],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "stale_review_runs",
            source,
        })?;
    conn.execute(
        "UPDATE workflow_board_review_findings
         SET status = 'stale',
             updated_at = ?2
         WHERE card_id = ?1
           AND status = 'open'",
        params![card_id, now],
    )
    .map_err(|source| WorkflowBoardRepoError::Sqlite {
        operation: "stale_review_findings",
        source,
    })?;
    let violation_next_status = if reason.starts_with("amendment_proposed") {
        "amended"
    } else {
        "stale"
    };
    let violations = conn
        .execute(
            "UPDATE workflow_board_contract_violations
             SET status = ?3,
                 updated_at = ?2
             WHERE card_id = ?1
               AND status = 'violation_pending'",
            params![card_id, now, violation_next_status],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "stale_contract_violations",
            source,
        })?;
    Ok(checks + receipts + ac + reviews + violations)
}

pub(crate) struct NextSpecState {
    pub title: String,
    pub brief: String,
    pub spec_json: String,
    pub intake_goal: Option<String>,
    pub input_paths_json: Option<String>,
    pub wireframes_digest: Option<String>,
    pub digest: Option<String>,
    pub changed: bool,
}

pub(crate) fn compute_spec_digest_hex(title: &str, brief: &str, spec_json: &str) -> String {
    compute_prefixed_sha256(format!("{title}\n---\n{brief}\n---\n{spec_json}").as_bytes())
}

pub(crate) fn compute_next_spec_state(
    existing: &super::types::CardRow,
    input: &super::UpdateSpecInput<'_>,
) -> Result<NextSpecState, WorkflowBoardRepoError> {
    let title = match input.title {
        Some(t) => super::require_non_empty(t, "title")?.to_owned(),
        None => existing.title.clone(),
    };
    let brief = input
        .brief
        .map_or_else(|| existing.brief.clone(), str::to_owned);
    let spec_json = input
        .spec_json
        .map_or_else(|| existing.spec_json.clone(), str::to_owned);
    let intake_goal = match input.intake_goal {
        Some(_) => super::cards::validate_intake_goal(input.intake_goal)?,
        None => existing.intake_goal.clone(),
    };
    let input_paths_json = match input.input_paths_json {
        Some(raw) => super::cards::validate_input_paths_json(Some(raw))?,
        None => existing.input_paths_json.clone(),
    };
    let wireframes_digest = if input.clear_wireframes_digest {
        None
    } else if let Some(d) = input.wireframes_digest {
        Some(super::require_non_empty(d, "wireframes_digest")?.to_owned())
    } else {
        existing.wireframes_digest.clone()
    };
    let content_changed = title != existing.title
        || brief != existing.brief
        || spec_json != existing.spec_json
        || intake_goal != existing.intake_goal
        || input_paths_json != existing.input_paths_json
        || wireframes_digest != existing.wireframes_digest;
    let digest = match input.spec_digest {
        Some(raw) => Some(super::require_non_empty(raw, "spec_digest")?.to_owned()),
        None if content_changed || existing.spec_digest.is_none() => {
            Some(compute_spec_digest_hex(&title, &brief, &spec_json))
        }
        None => existing.spec_digest.clone(),
    };
    let changed = content_changed || digest != existing.spec_digest;
    Ok(NextSpecState {
        title,
        brief,
        spec_json,
        intake_goal,
        input_paths_json: Some(input_paths_json),
        wireframes_digest,
        digest,
        changed,
    })
}

/// Idempotently seeds unresolved exploration decisions inside the caller's transaction (`SAVEPOINT`).
pub fn seed_exploration_decisions(
    conn: &Connection,
    card_id: &str,
    seeds: &[super::types_m2::SeedDecisionInput<'_>],
    now_iso: &str,
) -> Result<(), WorkflowBoardRepoError> {
    if seeds.is_empty() {
        return Ok(());
    }
    let card_id = require_non_empty(card_id, "card_id")?;
    let now = require_non_empty(now_iso, "now_iso")?;
    let tx = WriteTx::begin(conn)?;
    for seed in seeds {
        let decision_id = require_non_empty(seed.decision_id, "seed.decision_id")?;
        let prompt = require_non_empty(seed.prompt, "seed.prompt")?;
        let depends_on_json = serde_json::to_string(seed.depends_on).map_err(|source| {
            WorkflowBoardRepoError::Json {
                operation: "serialize_seed_depends_on",
                source,
            }
        })?;
        conn.execute(
            "INSERT INTO workflow_board_decisions (
               card_id, decision_id, prompt, recommendation, depends_on_json,
               status, rationale, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'unresolved', ?6, ?7, ?7)
             ON CONFLICT(card_id, decision_id) DO NOTHING",
            params![
                card_id,
                decision_id,
                prompt,
                seed.recommendation,
                depends_on_json,
                seed.rationale,
                now,
            ],
        )
        .map_err(|source| WorkflowBoardRepoError::Sqlite {
            operation: "seed_exploration_decision",
            source,
        })?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn maybe_stale_on_explore(
    conn: &Connection,
    card_id: &str,
    has_answers: bool,
    is_first_round: bool,
    now: &str,
) -> Result<bool, WorkflowBoardRepoError> {
    let has_decisions = has_answers
        || (is_first_round
            && conn
                .query_row(
                    "SELECT 1 FROM workflow_board_decisions WHERE card_id = ?1 LIMIT 1",
                    params![card_id],
                    |_| Ok(true),
                )
                .optional()
                .map_err(|source| WorkflowBoardRepoError::Sqlite {
                    operation: "check_decisions_for_explore_invalidation",
                    source,
                })?
                .unwrap_or(false));
    if !has_decisions {
        return Ok(false);
    }
    let staled = stale_approvals_for_gates(
        conn,
        card_id,
        &["spec", "plan", "delivery"],
        "exploration_updated",
        now,
    )? > 0;
    if staled {
        stale_active_challenges(conn, card_id, now)?;
        stale_downstream_evidence(conn, card_id, "exploration_updated", now)?;
    }
    Ok(staled)
}
