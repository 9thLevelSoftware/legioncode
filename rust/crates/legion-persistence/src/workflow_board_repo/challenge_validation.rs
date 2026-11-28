//! Concern validation, verbatim human-answer preservation, and transition rules for `challenge_step`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::types_m2::{ChallengeConcernInput, ChallengeStepInput};
use super::{WorkflowBoardRepoError, require_non_empty};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredConcern {
    pub id: String,
    pub summary: String,
    #[serde(default)]
    pub why_matters: Option<String>,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub assumption: Option<String>,
    pub disposition: String,
    pub answer: Option<String>,
    pub reason: Option<String>,
}

pub(crate) struct ChallengeTransition {
    pub next_status: &'static str,
    pub next_synth_count: i64,
    pub manual_fallback_json: Option<String>,
    pub completed_at: Option<String>,
}

pub(crate) fn validate_concerns(
    concerns: &[ChallengeConcernInput<'_>],
) -> Result<Vec<StoredConcern>, WorkflowBoardRepoError> {
    if concerns.len() > 3 {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "challenge_step accepts at most 3 grounded concerns".to_owned(),
        ));
    }
    let mut seen_ids: HashSet<&str> = HashSet::with_capacity(concerns.len());
    let mut out = Vec::with_capacity(concerns.len());
    for c in concerns {
        let id_str = require_non_empty(c.id, "concern.id")?;
        if !seen_ids.insert(id_str) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "duplicate challenge concern id `{id_str}`"
            )));
        }
        let id = id_str.to_owned();
        let summary = require_non_empty(c.summary, "concern.summary")?.to_owned();
        let disp = require_non_empty(c.disposition, "concern.disposition")?;
        if !matches!(
            disp,
            "unresolved" | "answer" | "dismiss" | "accept" | "accept-risk" | "defer"
        ) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "unsupported challenge disposition `{disp}`"
            )));
        }
        if disp == "dismiss" && c.reason.map(str::trim).is_none_or(str::is_empty) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "dismissing concern `{id}` requires a non-empty reason"
            )));
        }
        if disp == "answer" && c.answer.map(str::trim).is_none_or(str::is_empty) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "answering concern `{id}` requires a non-empty answer"
            )));
        }
        if let Some(mutated) = c.mutated_answer
            && mutated.trim() != c.answer.unwrap_or("").trim()
        {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "synthesis cannot mutate verbatim human answer on `{id}`"
            )));
        }
        out.push(StoredConcern {
            id,
            summary,
            why_matters: c
                .why_matters
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            evidence: c
                .evidence
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            assumption: c
                .assumption
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            disposition: disp.to_owned(),
            answer: c
                .answer
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            reason: c
                .reason
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        });
    }
    Ok(out)
}

pub(crate) fn parse_prior_concerns(
    raw: &str,
) -> Result<Vec<StoredConcern>, WorkflowBoardRepoError> {
    serde_json::from_str(raw).map_err(|source| WorkflowBoardRepoError::Json {
        operation: "parse_prior_challenge_concerns",
        source,
    })
}

pub(crate) fn verify_human_answers_preserved(
    prior_json: Option<&str>,
    incoming: &[StoredConcern],
) -> Result<(), WorkflowBoardRepoError> {
    let Some(raw) = prior_json else {
        return Ok(());
    };
    let prior = parse_prior_concerns(raw)?;
    let prior_answers: Vec<(&str, &str)> = prior
        .iter()
        .filter_map(|c| {
            c.answer
                .as_deref()
                .map(str::trim)
                .filter(|ans| !ans.is_empty())
                .map(|ans| (c.id.as_str(), ans))
        })
        .collect();
    if prior_answers.is_empty() {
        return Ok(());
    }
    if incoming.is_empty() {
        return Err(WorkflowBoardRepoError::InvalidInput(
            "synthesis cannot omit prior concerns with recorded human answers".to_owned(),
        ));
    }
    let incoming_by_id: HashMap<&str, &StoredConcern> =
        incoming.iter().map(|c| (c.id.as_str(), c)).collect();
    for (prior_id, orig_ans) in prior_answers {
        let Some(next) = incoming_by_id.get(prior_id) else {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "synthesis cannot omit prior answered concern `{prior_id}`"
            )));
        };
        let next_ans = next
            .answer
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if next_ans != Some(orig_ans) {
            return Err(WorkflowBoardRepoError::InvalidInput(format!(
                "synthesis cannot overwrite or erase verbatim human answer on `{prior_id}`"
            )));
        }
    }
    Ok(())
}

pub(crate) fn resolve_challenge_transition(
    action: &str,
    input: &ChallengeStepInput<'_>,
    prior_synth_count: i64,
    prior_concerns_json: Option<&str>,
    incoming_concerns: &[StoredConcern],
    effective_concerns: &[StoredConcern],
    now: &str,
) -> Result<ChallengeTransition, WorkflowBoardRepoError> {
    let mut next_synth_count = prior_synth_count;
    let mut manual_fallback_json: Option<String> = None;
    let mut completed_at: Option<String> = None;
    let has_unresolved = effective_concerns
        .iter()
        .any(|c| c.disposition == "unresolved");

    let next_status = match action {
        "synthesize" => {
            if prior_synth_count >= 1 {
                return Err(WorkflowBoardRepoError::PreconditionFailed(
                    "challenge synthesis pass may run at most once".to_owned(),
                ));
            }
            verify_human_answers_preserved(prior_concerns_json, incoming_concerns)?;
            next_synth_count = 1;
            "synthesized"
        }
        "manualFallback" => {
            if input.acknowledgement_token != Some("I acknowledge") {
                return Err(WorkflowBoardRepoError::InvalidInput(
                    "manualFallback requires exact acknowledgementToken \"I acknowledge\""
                        .to_owned(),
                ));
            }
            if !input.unresolved_concern_ids.is_empty() || has_unresolved {
                return Err(WorkflowBoardRepoError::PreconditionFailed(
                    "manualFallback rejected while unresolved concerns exist".to_owned(),
                ));
            }
            let answers = input
                .manual_answers
                .filter(|a| a.len() == 3 && a.iter().all(|s| !s.trim().is_empty()))
                .ok_or_else(|| {
                    WorkflowBoardRepoError::InvalidInput(
                        "manualFallback requires exactly 3 non-empty answers".to_owned(),
                    )
                })?;
            let trimmed: Vec<&str> = answers.iter().map(|s| s.trim()).collect();
            manual_fallback_json = Some(
                serde_json::to_string(&json!({
                    "acknowledgementToken": "I acknowledge",
                    "answers": trimmed,
                    "recordedAt": now,
                }))
                .map_err(|source| WorkflowBoardRepoError::Json {
                    operation: "serialize_manual_fallback",
                    source,
                })?,
            );
            completed_at = Some(now.to_owned());
            "manual_fallback"
        }
        "complete" => {
            if has_unresolved {
                return Err(WorkflowBoardRepoError::PreconditionFailed(
                    "cannot complete challenge with unresolved concerns".to_owned(),
                ));
            }
            completed_at = Some(now.to_owned());
            "complete"
        }
        "resolve" => {
            if has_unresolved {
                "in_progress"
            } else {
                "resolved"
            }
        }
        _ => "in_progress",
    };

    Ok(ChallengeTransition {
        next_status,
        next_synth_count,
        manual_fallback_json,
        completed_at,
    })
}
