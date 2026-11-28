//! RPC payloads for Milestone 2 Spec exploration (`exploreStep`) and challenge (`challengeStep`).

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::ids::WorkflowCardId;

use super::WorkflowCardDetail;
use super::m2_models::{
    WorkflowBoardChallengeConcern, WorkflowBoardChallengeDisposition,
    WorkflowBoardChallengeReceipt, WorkflowBoardChallengeStepAction,
    WorkflowBoardExplorationDecision, WorkflowBoardExplorationState, WorkflowBoardExploreAction,
    WorkflowBoardExploreStepAction,
};
use super::validators::{
    deserialize_opt_trimmed_non_empty, deserialize_opt_trimmed_non_empty_vec,
    deserialize_positive_revision, deserialize_trimmed_non_empty,
    deserialize_trimmed_non_empty_vec,
};

/// Single exploration decision input item (`answers[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardExploreAnswerInput {
    /// Decision identifier.
    pub decision_id: String,
    /// Action (`explain`, `recommend`, `edit`, `unsure`, `pause`, `resume`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<WorkflowBoardExploreAction>,
    /// Alias for `action`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<WorkflowBoardExploreAction>,
    /// Custom answer text (required when `edit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Optional note / answer alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardExploreAnswerInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            decision_id: String,
            #[serde(default)]
            action: Option<WorkflowBoardExploreAction>,
            #[serde(default)]
            choice: Option<WorkflowBoardExploreAction>,
            #[serde(default, deserialize_with = "deserialize_opt_trimmed_non_empty")]
            answer: Option<String>,
            #[serde(default)]
            note: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let effective = raw.action.or(raw.choice).ok_or_else(|| {
            de::Error::custom("explore answer requires either `action` or `choice`")
        })?;
        if effective == WorkflowBoardExploreAction::Edit {
            let has_val = raw
                .answer
                .as_deref()
                .or(raw.note.as_deref())
                .map(str::trim)
                .is_some_and(|s| !s.is_empty());
            if !has_val {
                return Err(de::Error::custom(
                    "explore `edit` action requires non-empty `answer` or `note`",
                ));
            }
        }
        Ok(Self {
            decision_id: raw.decision_id,
            action: raw.action,
            choice: raw.choice,
            answer: raw.answer,
            note: raw.note,
        })
    }
}

/// Input for `workflowBoard.exploreStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardExploreStepInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Top-level action (`step`, `pause`, `resume`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<WorkflowBoardExploreStepAction>,
    /// Explicit opt-in to advance to the next exploration round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_round: Option<bool>,
    /// Up to 3 decision responses for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Vec<WorkflowBoardExploreAnswerInput>>,
}

impl<'de> Deserialize<'de> for WorkflowBoardExploreStepInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            #[serde(default)]
            action: Option<WorkflowBoardExploreStepAction>,
            #[serde(default)]
            continue_round: Option<bool>,
            #[serde(default)]
            answers: Option<Vec<WorkflowBoardExploreAnswerInput>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if let Some(ref answers) = raw.answers
            && answers.len() > 3
        {
            return Err(de::Error::custom(
                "exploreStep accepts at most 3 decision answers per round",
            ));
        }
        Ok(Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            action: raw.action,
            continue_round: raw.continue_round,
            answers: raw.answers,
        })
    }
}

/// Output for `workflowBoard.exploreStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardExploreStepResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Current exploration round index.
    pub round_index: u64,
    /// Whether advancing requires explicit `continueRound: true`.
    pub pending_round_prompt: bool,
    /// Whether exploration is currently paused.
    pub paused: bool,
    /// Updated exploration state.
    pub exploration: WorkflowBoardExplorationState,
    /// Current decision list.
    pub decisions: Vec<WorkflowBoardExplorationDecision>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Challenge concern input entry (`concerns[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeConcernInput {
    /// Concern identifier.
    pub id: String,
    /// Concern summary.
    pub summary: String,
    /// Optional explanation of why it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_matters: Option<String>,
    /// Optional grounding evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Optional underlying assumption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumption: Option<String>,
    /// Operator disposition.
    pub disposition: WorkflowBoardChallengeDisposition,
    /// Operator answer text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Operator dismissal/deferral reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Guard field rejected if synthesis mutates verbatim human answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutated_answer: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardChallengeConcernInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            id: String,
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            summary: String,
            #[serde(default)]
            why_matters: Option<String>,
            #[serde(default)]
            evidence: Option<String>,
            #[serde(default)]
            assumption: Option<String>,
            disposition: WorkflowBoardChallengeDisposition,
            #[serde(default)]
            answer: Option<String>,
            #[serde(default)]
            reason: Option<String>,
            #[serde(default)]
            mutated_answer: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.disposition == WorkflowBoardChallengeDisposition::Dismiss
            && raw
                .reason
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
        {
            return Err(de::Error::custom(
                "dismissing a challenge concern requires a non-empty reason",
            ));
        }
        if raw.disposition == WorkflowBoardChallengeDisposition::Answer
            && raw
                .answer
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
        {
            return Err(de::Error::custom(
                "answering a challenge concern requires a non-empty answer",
            ));
        }
        if let Some(ref mutated) = raw.mutated_answer
            && mutated.trim() != raw.answer.as_deref().unwrap_or("").trim()
        {
            return Err(de::Error::custom(
                "challenge synthesis cannot mutate verbatim human answer",
            ));
        }
        Ok(Self {
            id: raw.id,
            summary: raw.summary,
            why_matters: raw.why_matters,
            evidence: raw.evidence,
            assumption: raw.assumption,
            disposition: raw.disposition,
            answer: raw.answer,
            reason: raw.reason,
            mutated_answer: raw.mutated_answer,
        })
    }
}

/// Alternate concern response shape (`answers[]` for `synthesize`/`resolve`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeConcernResponse {
    /// Target concern identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub concern_id: String,
    /// Disposition.
    pub disposition: WorkflowBoardChallengeDisposition,
    /// Response text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    /// Dismissal/deferral reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Union payload for `WorkflowBoardChallengeStepInput.answers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowBoardChallengeStepAnswers {
    /// Three strings for `manualFallback`.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    Strings(Vec<String>),
    /// Structured concern responses.
    Responses(Vec<WorkflowBoardChallengeConcernResponse>),
}

/// Input for `workflowBoard.challengeStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeStepInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Challenge step action.
    pub action: WorkflowBoardChallengeStepAction,
    /// Up to 3 challenge concerns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concerns: Option<Vec<WorkflowBoardChallengeConcernInput>>,
    /// Exact `"I acknowledge"` token for `manualFallback`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledgement_token: Option<String>,
    /// Unresolved concern IDs (must be empty on `manualFallback`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_concern_ids: Option<Vec<String>>,
    /// 3 answers on `manualFallback` or structured responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<WorkflowBoardChallengeStepAnswers>,
    /// Bypass guard (must never be `Some(true)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
}

impl<'de> Deserialize<'de> for WorkflowBoardChallengeStepInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            action: WorkflowBoardChallengeStepAction,
            #[serde(default)]
            concerns: Option<Vec<WorkflowBoardChallengeConcernInput>>,
            #[serde(default)]
            acknowledgement_token: Option<String>,
            #[serde(default, deserialize_with = "deserialize_opt_trimmed_non_empty_vec")]
            unresolved_concern_ids: Option<Vec<String>>,
            #[serde(default)]
            answers: Option<WorkflowBoardChallengeStepAnswers>,
            #[serde(default)]
            approved: Option<bool>,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.approved == Some(true) {
            return Err(de::Error::custom(
                "challengeStep cannot set approved=true; human approval requires approveSpec",
            ));
        }
        if let Some(ref concerns) = raw.concerns
            && concerns.len() > 3
        {
            return Err(de::Error::custom(
                "challengeStep accepts at most 3 grounded concerns",
            ));
        }
        if raw.action == WorkflowBoardChallengeStepAction::ManualFallback {
            if raw.acknowledgement_token.as_deref() != Some("I acknowledge") {
                return Err(de::Error::custom(
                    "manualFallback requires exact acknowledgementToken \"I acknowledge\"",
                ));
            }
            if raw
                .unresolved_concern_ids
                .as_deref()
                .is_some_and(|ids| !ids.is_empty())
            {
                return Err(de::Error::custom(
                    "manualFallback cannot proceed while unresolvedConcernIds remain",
                ));
            }
            let valid_answers = matches!(
                raw.answers,
                Some(WorkflowBoardChallengeStepAnswers::Strings(ref vals))
                    if vals.len() == 3 && vals.iter().all(|s| !s.trim().is_empty())
            );
            if !valid_answers {
                return Err(de::Error::custom(
                    "manualFallback requires exactly 3 non-empty answers",
                ));
            }
        }
        Ok(Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            action: raw.action,
            concerns: raw.concerns,
            acknowledgement_token: raw.acknowledgement_token,
            unresolved_concern_ids: raw.unresolved_concern_ids,
            answers: raw.answers,
            approved: raw.approved,
        })
    }
}

/// Output for `workflowBoard.challengeStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeStepResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Updated challenge receipt.
    pub challenge_receipt: WorkflowBoardChallengeReceipt,
    /// Current challenge concerns.
    pub concerns: Vec<WorkflowBoardChallengeConcern>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}
