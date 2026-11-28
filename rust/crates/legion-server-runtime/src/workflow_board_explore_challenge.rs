#![forbid(unsafe_code)]
//! Guided exploration (`exploreStep`) and bounded challenge (`challengeStep`) service handlers.

use std::collections::HashMap;
use std::sync::Arc;

use legion_contracts::workflow_board::{
    WorkflowBoardExploreAction, WorkflowBoardExploreStepInput, WorkflowBoardExploreStepResult,
    WorkflowBoardRequestError, WorkflowBoardRequestErrorCode,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::types_m2::{
    CardPlanningDetail, ExploreAnswerInput, ExploreStepInput, SeedDecisionInput,
};
use legion_persistence::workflow_board_repo::{
    card_has_remaining_blocker, explore_step, get_card_planning_detail, seed_exploration_decisions,
};
use legion_workflows::sha256_prefixed;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, resolve_card_workspace_root,
    validate_safe_card_id,
};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_intake::build_exploration_seeds;
use crate::workflow_board_journal::{JournaledWriteSpec, execute_journaled_writes};
use crate::workflow_board_locks::acquire_card_lock;
use crate::workflow_board_recovery::reconcile_card_external_edits;
use crate::workflow_board_render::{parse_stored_spec_fields, render_card_json};
use crate::workflow_board_types::{
    check_revision, db_lock_err, enum_as_str, internal, map_repo_err, parse_expected_rev,
};

struct OwnedExploreAnswer {
    decision_id: String,
    action: String,
    answer: Option<String>,
    prompt: Option<String>,
    recommendation: Option<String>,
    rationale: Option<String>,
}

pub(crate) async fn explore_step_impl(
    orchestrator: &Arc<Orchestrator>,
    input: &WorkflowBoardExploreStepInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowBoardExploreStepResult, WorkflowBoardRequestError> {
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

    let owned_answers = prepare_explore_answers(&planning, input)?;
    let action_str = input.action.and_then(|a| enum_as_str(&a));
    let ws_root = resolve_card_workspace_root(&conn, &planning.card_detail.card)?;
    let card_dir = ensure_confined_card_dir(&ws_root, card_id)?;

    let fields = parse_stored_spec_fields(&planning.card_detail.card.spec_json);
    let missing_decisions = fields
        .intake_analysis
        .as_ref()
        .map(|a| a.missing_decisions.clone())
        .unwrap_or_default();
    let is_pause_or_resume = matches!(action_str.as_deref(), Some("pause" | "resume"));
    let can_seed = planning.card_detail.card.stage == "spec"
        && !matches!(
            planning.card_detail.card.status.as_str(),
            "completed" | "shipped"
        )
        && !is_pause_or_resume;
    let seeds = if can_seed && planning.decisions.is_empty() && owned_answers.is_empty() {
        build_exploration_seeds(
            &planning.card_detail.card.title,
            planning.card_detail.card.intake_goal.as_deref(),
            &missing_decisions,
        )
    } else {
        Vec::new()
    };
    let regressing_to_spec = planning.card_detail.card.stage != "spec" && !owned_answers.is_empty();
    let resume_has_blocker = (regressing_to_spec
        || (action_str.as_deref() == Some("resume")
            && planning.card_detail.card.status == "paused"))
        && card_has_remaining_blocker(&conn, card_id, "spec").map_err(map_repo_err)?;
    let card_writes = render_next_card_json_write(&planning, &now_iso, |c| {
        if regressing_to_spec {
            "spec".clone_into(&mut c.stage);
            "active".clone_into(&mut c.status);
            c.blocker_reason = None;
            c.next_action = Some("explore_spec".to_owned());
        }
        if action_str.as_deref() == Some("pause") {
            "paused".clone_into(&mut c.status);
            c.next_action = Some("resume_exploration".to_owned());
        } else if (action_str.as_deref() == Some("resume") && c.status == "paused")
            || regressing_to_spec
        {
            if resume_has_blocker {
                "blocked".clone_into(&mut c.status);
            } else {
                "active".clone_into(&mut c.status);
                c.blocker_reason = None;
            }
            c.next_action = Some("explore_spec".to_owned());
        }
    })?;

    let updated = persist_explore_tx(
        &conn,
        card_id,
        &card_dir,
        &card_writes,
        expected_rev_i64,
        action_str.as_deref(),
        input.continue_round,
        &seeds,
        &owned_answers,
        &now_iso,
    )?;

    notify_project(&updated.card_detail.card.project_id);
    let keys = enumerate_artifact_keys(&ws_root, card_id);
    let detail = to_card_detail_from_planning(&updated, keys)?;
    let exploration = detail
        .exploration
        .clone()
        .ok_or_else(|| internal("Exploration state missing after exploreStep"))?;
    Ok(WorkflowBoardExploreStepResult {
        card_id: detail.summary.card_id.clone(),
        revision: detail.summary.revision,
        round_index: exploration.round_index,
        pending_round_prompt: exploration.pending_round_prompt,
        paused: exploration.paused,
        decisions: exploration.decisions.clone(),
        exploration,
        card: detail,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Coordinates journaled card.json write with decision seeding and explore_step inside one transaction"
)]
fn persist_explore_tx(
    conn: &rusqlite::Connection,
    card_id: &str,
    card_dir: &std::path::Path,
    card_writes: &[JournaledWriteSpec],
    expected_rev_i64: i64,
    action: Option<&str>,
    continue_round: Option<bool>,
    seeds: &[crate::workflow_board_intake::SeedDecisionData],
    owned_answers: &[OwnedExploreAnswer],
    now_iso: &str,
) -> Result<CardPlanningDetail, WorkflowBoardRequestError> {
    let rev_id = format!("rev-{}", Uuid::new_v4().simple());
    execute_journaled_writes(conn, card_id, card_dir, card_writes, now_iso, |tx| {
        let atomic_tx = tx
            .unchecked_transaction()
            .map_err(|e| internal(format!("Failed starting exploration transaction: {e}")))?;
        if !seeds.is_empty() {
            let dep_slices: Vec<Vec<&str>> = seeds
                .iter()
                .map(|s| s.depends_on.iter().map(String::as_str).collect())
                .collect();
            let seed_refs: Vec<SeedDecisionInput<'_>> = seeds
                .iter()
                .zip(dep_slices.iter())
                .map(|(s, deps)| SeedDecisionInput {
                    decision_id: &s.decision_id,
                    prompt: &s.prompt,
                    recommendation: s.recommendation.as_deref(),
                    rationale: s.rationale.as_deref(),
                    depends_on: deps.as_slice(),
                })
                .collect();
            seed_exploration_decisions(&atomic_tx, card_id, &seed_refs, now_iso)
                .map_err(map_repo_err)?;
        }
        let ans_refs: Vec<ExploreAnswerInput<'_>> = owned_answers
            .iter()
            .map(|a| ExploreAnswerInput {
                decision_id: &a.decision_id,
                action: &a.action,
                answer: a.answer.as_deref(),
                prompt: a.prompt.as_deref(),
                recommendation: a.recommendation.as_deref(),
                rationale: a.rationale.as_deref(),
            })
            .collect();
        let detail = explore_step(
            &atomic_tx,
            &ExploreStepInput {
                card_id,
                revision_id: &rev_id,
                expected_revision: expected_rev_i64,
                action,
                continue_round,
                answers: &ans_refs,
            },
            now_iso,
        )
        .map_err(map_repo_err)?;
        atomic_tx
            .commit()
            .map_err(|e| internal(format!("Failed committing exploration transaction: {e}")))?;
        Ok(detail)
    })
}

fn prepare_explore_answers(
    planning: &CardPlanningDetail,
    input: &WorkflowBoardExploreStepInput,
) -> Result<Vec<OwnedExploreAnswer>, WorkflowBoardRequestError> {
    let raw_answers = input.answers.as_deref().unwrap_or(&[]);
    let by_id: HashMap<&str, _> = planning
        .decisions
        .iter()
        .map(|d| (d.decision_id.as_str(), d))
        .collect();

    let mut out = Vec::with_capacity(raw_answers.len());
    for ans in raw_answers {
        let effective = ans.action.or(ans.choice).ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                "explore answer requires action or choice",
            )
        })?;
        let action_str = enum_as_str(&effective).unwrap_or_else(|| "unsure".to_owned());
        let existing = by_id.get(ans.decision_id.trim()).copied();
        let val = ans
            .answer
            .as_deref()
            .or(ans.note.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let recommendation = if effective == WorkflowBoardExploreAction::Recommend {
            existing
                .and_then(|d| d.recommendation.clone())
                .or_else(|| {
                    Some(format!(
                        "Recommended approach for `{}`: preserve transactional consistency and minimal surface area.",
                        ans.decision_id.trim()
                    ))
                })
        } else {
            None
        };
        let rationale = if effective == WorkflowBoardExploreAction::Explain {
            existing.and_then(|d| d.rationale.clone()).or_else(|| {
                Some(format!(
                    "Tradeoff explanation for `{}`: resolving this decision clarifies spec acceptance boundaries before Plan.",
                    ans.decision_id.trim()
                ))
            })
        } else {
            None
        };
        out.push(OwnedExploreAnswer {
            decision_id: ans.decision_id.trim().to_owned(),
            action: action_str,
            answer: val.or_else(|| recommendation.clone()),
            prompt: existing
                .map(|d| d.prompt.clone())
                .filter(|p| !p.is_empty())
                .or_else(|| Some(format!("Decision {}", ans.decision_id.trim()))),
            recommendation,
            rationale,
        });
    }
    Ok(out)
}

pub(crate) fn render_next_card_json_write(
    planning: &CardPlanningDetail,
    now_iso: &str,
    mutate: impl FnOnce(&mut legion_persistence::workflow_board_repo::types::CardRow),
) -> Result<Vec<JournaledWriteSpec>, WorkflowBoardRequestError> {
    let fields = parse_stored_spec_fields(&planning.card_detail.card.spec_json);
    let mut next_card = planning.card_detail.card.clone();
    next_card.revision += 1;
    now_iso.clone_into(&mut next_card.updated_at);
    mutate(&mut next_card);
    let card_json = render_card_json(&next_card, &fields)?;
    Ok(vec![JournaledWriteSpec {
        key: "card.json".to_owned(),
        bytes: card_json.as_bytes().to_vec(),
        expected_digest: sha256_prefixed(card_json.as_bytes()),
        revision: Some(next_card.revision),
    }])
}
