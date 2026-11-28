#![forbid(unsafe_code)]
//! Bounded challenge (`challengeStep`) service handler and concern/digest helpers.

use std::collections::HashSet;
use std::sync::Arc;

use legion_contracts::workflow_board::{
    WorkflowBoardChallengeConcern, WorkflowBoardChallengeConcernInput,
    WorkflowBoardChallengeStepAction, WorkflowBoardChallengeStepAnswers,
    WorkflowBoardChallengeStepInput, WorkflowBoardChallengeStepResult, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m2::{
    CardPlanningDetail, ChallengeConcernInput, ChallengeStepInput,
};
use legion_persistence::workflow_board_repo::{challenge_step, get_card_planning_detail};
use legion_workflows::sha256_prefixed;
use serde_json::json;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_explore_challenge::render_next_card_json_write;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, resolve_card_workspace_root,
    validate_safe_card_id,
};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_journal::execute_journaled_writes;
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_render::parse_stored_spec_fields;
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, internal, map_repo_err, parse_expected_rev,
};

struct OwnedConcern {
    id: String,
    summary: String,
    why_matters: Option<String>,
    evidence: Option<String>,
    assumption: Option<String>,
    disposition: String,
    answer: Option<String>,
    reason: Option<String>,
    mutated_answer: Option<String>,
}

pub(crate) async fn challenge_step_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardChallengeStepInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardChallengeStepResult, WorkflowBoardRequestError> {
    if input.approved == Some(true) {
        return Err(WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InvalidInput,
            "challengeStep cannot set approved=true",
        ));
    }
    let card_id = validate_safe_card_id(input.card_id.as_str())?;
    let _card_guard = acquire_card_lock(orchestrator, card_id).await;
    let now_iso = utc_now_iso();
    let expected_rev_i64 = parse_expected_rev(input.expected_revision)?;

    let conn = orchestrator.sink().db().lock().map_err(|_| db_lock_err())?;
    let invalidated = reconcile_card_external_edits(&conn, card_id, &now_iso)?;
    let planning = get_card_planning_detail(&conn, card_id).map_err(map_repo_err)?;
    if invalidated {
        notify_project(&planning.card_detail.card.project_id);
    }
    check_revision(
        card_id,
        planning.card_detail.card.revision,
        input.expected_revision,
    )?;

    let action_str = enum_as_str(&input.action).unwrap_or_else(|| "start".to_owned());
    let owned_concerns = build_challenge_concerns(&planning, input)?;
    let manual_answers: Option<Vec<String>> = match &input.answers {
        Some(WorkflowBoardChallengeStepAnswers::Strings(vals)) => Some(vals.clone()),
        _ => None,
    };
    let unresolved_ids = input.unresolved_concern_ids.clone().unwrap_or_default();
    let interview_digest = compute_interview_digest(&planning);
    let source_refs_digest = compute_source_refs_digest(&planning);
    let synth_diff_json = build_synth_diff_json(input.action, &owned_concerns, &planning);

    let ws_root = resolve_card_workspace_root(&conn, &planning.card_detail.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;
    let writes = render_next_card_json_write(&planning, &now_iso, |_| {})?;

    let updated = persist_challenge_tx(
        &conn,
        card_id,
        &card_dir,
        &writes,
        expected_rev_i64,
        &action_str,
        &owned_concerns,
        input,
        &unresolved_ids,
        manual_answers.as_deref(),
        &planning,
        interview_digest.as_deref(),
        source_refs_digest.as_deref(),
        synth_diff_json.as_deref(),
        &now_iso,
    )?;

    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    let receipt = detail
        .challenge_receipt
        .clone()
        .ok_or_else(|| internal("Challenge receipt missing after challengeStep"))?;
    Ok(WorkflowBoardChallengeStepResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        concerns: receipt.concerns.clone(),
        challenge_receipt: receipt,
        card: detail,
    })
}

fn build_synth_diff_json(
    action: WorkflowBoardChallengeStepAction,
    owned_concerns: &[OwnedConcern],
    planning: &CardPlanningDetail,
) -> Option<String> {
    if action != WorkflowBoardChallengeStepAction::Synthesize {
        return None;
    }
    let count = if owned_concerns.is_empty() {
        planning
            .challenges
            .first()
            .map_or(0, |c| c.concerns_json.len().min(3))
    } else {
        owned_concerns.len()
    };
    Some(
        json!({
            "diffSummary": format!("Synthesized {count} concern(s) preserving verbatim operator answers.")
        })
        .to_string(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Coordinates journaled card.json write with challenge_step inside a single transaction"
)]
fn persist_challenge_tx(
    conn: &rusqlite::Connection,
    card_id: &str,
    card_dir: &std::path::Path,
    writes: &[crate::workflow_board_journal::JournaledWriteSpec],
    expected_rev_i64: i64,
    action_str: &str,
    owned_concerns: &[OwnedConcern],
    input: &WorkflowBoardChallengeStepInput,
    unresolved_ids: &[String],
    manual_answers: Option<&[String]>,
    planning: &CardPlanningDetail,
    interview_digest: Option<&str>,
    source_refs_digest: Option<&str>,
    synth_diff_json: Option<&str>,
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRequestError> {
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    let ch_id = format!("ch-{}", Uuid::new_v4().simple());
    execute_journaled_writes(conn, card_id, card_dir, writes, now_iso, |tx| {
        let concern_refs: Vec<ChallengeConcernInput<'_>> = owned_concerns
            .iter()
            .map(|c| ChallengeConcernInput {
                id: &c.id,
                summary: &c.summary,
                why_matters: c.why_matters.as_deref(),
                evidence: c.evidence.as_deref(),
                assumption: c.assumption.as_deref(),
                disposition: &c.disposition,
                answer: c.answer.as_deref(),
                reason: c.reason.as_deref(),
                mutated_answer: c.mutated_answer.as_deref(),
            })
            .collect();
        let unres_refs: Vec<&str> = unresolved_ids.iter().map(String::as_str).collect();
        let man_refs: Option<Vec<&str>> =
            manual_answers.map(|v| v.iter().map(String::as_str).collect());
        challenge_step(
            tx,
            &ChallengeStepInput {
                card_id,
                revision_id: &rev_id,
                challenge_id: &ch_id,
                expected_revision: expected_rev_i64,
                action: action_str,
                concerns: &concern_refs,
                acknowledgement_token: input.acknowledgement_token.as_deref(),
                unresolved_concern_ids: &unres_refs,
                manual_answers: man_refs.as_deref(),
                spec_digest: planning.card_detail.card.spec_digest.as_deref(),
                interview_digest,
                source_refs_digest,
                synthesis_diff_json: synth_diff_json,
                approved: input.approved,
            },
            now_iso,
        )
        .map_err(map_repo_err)
    })
}

fn build_challenge_concerns(
    planning: &CardPlanningDetail,
    input: &WorkflowBoardChallengeStepInput,
) -> Result<Vec<OwnedConcern>, WorkflowBoardRequestError> {
    if let Some(ref concerns) = input.concerns
        && !concerns.is_empty()
    {
        return Ok(concerns.iter().map(map_concern_input).collect());
    }
    if let Some(WorkflowBoardChallengeStepAnswers::Responses(ref resps)) = input.answers {
        return merge_challenge_responses(planning, resps, input.action);
    }
    if input.action == WorkflowBoardChallengeStepAction::Start
        && input.concerns.is_none()
        && planning
            .challenges
            .first()
            .is_none_or(|c| c.status == "stale" || c.concerns_json == "[]")
    {
        let fields = parse_stored_spec_fields(&planning.card_detail.card.spec_json);
        let mut generated = Vec::new();
        if fields.acceptance_criteria.is_empty() {
            generated.push(OwnedConcern {
                id: "CON-1".to_owned(),
                summary: "Acceptance criteria list is currently empty.".to_owned(),
                why_matters: Some("Plan verification needs concrete AC-* targets.".to_owned()),
                evidence: Some("spec.md".to_owned()),
                assumption: Some("Criteria will be added before Spec approval.".to_owned()),
                disposition: "unresolved".to_owned(),
                answer: None,
                reason: None,
                mutated_answer: None,
            });
        }
        return Ok(generated);
    }
    Ok(Vec::new())
}

fn merge_challenge_responses(
    planning: &CardPlanningDetail,
    resps: &[legion_contracts::workflow_board::WorkflowBoardChallengeConcernResponse],
    action: WorkflowBoardChallengeStepAction,
) -> Result<Vec<OwnedConcern>, WorkflowBoardRequestError> {
    let mut existing: Vec<OwnedConcern> = match planning
        .challenges
        .first()
        .filter(|ch| ch.status != "stale")
    {
        Some(ch) => serde_json::from_str::<Vec<WorkflowBoardChallengeConcern>>(&ch.concerns_json)
            .map_err(|e| internal(format!("Failed to parse persisted challenge concerns: {e}")))?
            .into_iter()
            .map(|c| OwnedConcern {
                id: c.id,
                summary: c.summary,
                why_matters: c.why_matters,
                evidence: c.evidence,
                assumption: c.assumption,
                disposition: enum_as_str(&c.disposition).unwrap_or_else(|| "unresolved".to_owned()),
                answer: c.answer,
                reason: c.reason,
                mutated_answer: None,
            })
            .collect(),
        None => Vec::new(),
    };

    let mut seen_ids = HashSet::with_capacity(resps.len());
    if existing.is_empty() {
        if action != WorkflowBoardChallengeStepAction::Synthesize {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                "Cannot resolve challenge concern responses without active challenge concerns",
            ));
        }
        let mut out = Vec::with_capacity(resps.len());
        for r in resps {
            let cid = r.concern_id.trim();
            if !seen_ids.insert(cid) {
                return Err(WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InvalidInput,
                    format!("Duplicate challenge concern id `{cid}` in answers"),
                ));
            }
            let disp = enum_as_str(&r.disposition).unwrap_or_else(|| "accept".to_owned());
            let answer = r
                .response
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            out.push(OwnedConcern {
                id: cid.to_owned(),
                summary: format!("Concern {cid}"),
                why_matters: None,
                evidence: None,
                assumption: None,
                disposition: disp,
                answer,
                reason: r.reason.clone(),
                mutated_answer: None,
            });
        }
        return Ok(out);
    }

    for r in resps {
        let cid = r.concern_id.trim();
        if !seen_ids.insert(cid) {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                format!("Duplicate challenge concern id `{cid}` in answers"),
            ));
        }
        let target = existing
            .iter_mut()
            .find(|c| c.id.trim() == cid)
            .ok_or_else(|| {
                WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InvalidInput,
                    format!("Unknown challenge concern id `{cid}`"),
                )
            })?;
        target.disposition = enum_as_str(&r.disposition).unwrap_or_else(|| "accept".to_owned());
        if let Some(resp) = r
            .response
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            target.answer = Some(resp.to_owned());
        }
        if let Some(reason) = r.reason.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            target.reason = Some(reason.to_owned());
        }
    }
    Ok(existing)
}

fn map_concern_input(c: &WorkflowBoardChallengeConcernInput) -> OwnedConcern {
    OwnedConcern {
        id: c.id.clone(),
        summary: c.summary.clone(),
        why_matters: c.why_matters.clone(),
        evidence: c.evidence.clone(),
        assumption: c.assumption.clone(),
        disposition: enum_as_str(&c.disposition).unwrap_or_else(|| "unresolved".to_owned()),
        answer: c.answer.clone(),
        reason: c.reason.clone(),
        mutated_answer: c.mutated_answer.clone(),
    }
}

fn compute_interview_digest(planning: &CardPlanningDetail) -> Option<String> {
    let payload: String = planning
        .decisions
        .iter()
        .filter(|d| d.status == "resolved")
        .map(|d| format!("{}:{}", d.decision_id, d.answer.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");
    if payload.is_empty() {
        planning.card_detail.card.spec_digest.clone()
    } else {
        Some(sha256_prefixed(payload.as_bytes()))
    }
}

fn compute_source_refs_digest(planning: &CardPlanningDetail) -> Option<String> {
    let fields = parse_stored_spec_fields(&planning.card_detail.card.spec_json);
    if fields.source_refs.is_empty() {
        return None;
    }
    Some(sha256_prefixed(fields.source_refs.join("\n").as_bytes()))
}
