//! M2 Spec intake, diff, guided exploration, and bounded challenge domain models.

use serde::{Deserialize, Deserializer, Serialize, de};

use super::validators::{
    deserialize_artifact_key, deserialize_opt_trimmed_non_empty, deserialize_trimmed_non_empty,
    deserialize_trimmed_non_empty_vec,
};

/// Intake goal classification for a new or updated card specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardIntakeGoal {
    /// New greenfield feature.
    Greenfield,
    /// Brownfield feature or behavioral change.
    Change,
    /// Codebase audit or investigation.
    Audit,
}

/// Digest of a workspace input file consumed during card intake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardSourceInputDigest {
    /// Safe workspace-relative input path.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub path: String,
    /// SHA-256 content digest.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub digest: String,
}

/// Summary of intake decision analysis derived from brief or codebase inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardSpecIntakeAnalysis {
    /// Decisions explicitly supplied in the intake brief.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub supplied_decisions: Vec<String>,
    /// Decisions inferred from codebase context.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub inferred_decisions: Vec<String>,
    /// Decisions remaining unresolved.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub missing_decisions: Vec<String>,
    /// Optional primary source digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub source_digest: Option<String>,
}

/// Structured diff emitted when `workflowBoard.updateSpec` changes spec content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardSpecDiff {
    /// Previous spec digest before the update, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub previous_digest: Option<String>,
    /// Resulting spec digest.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub next_digest: String,
    /// Names of changed spec fields.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub changed_fields: Vec<String>,
    /// Unified diff text.
    pub unified_diff: String,
}

/// Operator action on an individual exploration decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardExploreAction {
    /// Request an explanation of tradeoffs.
    Explain,
    /// Request or accept a recommendation.
    Recommend,
    /// Provide a custom operator answer.
    Edit,
    /// Mark decision as unsure (remains unresolved).
    Unsure,
    /// Pause exploration without resolving.
    Pause,
    /// Resume paused exploration.
    Resume,
}

/// Top-level action for `workflowBoard.exploreStep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardExploreStepAction {
    /// Process decision answers for the active round.
    Step,
    /// Pause exploration.
    Pause,
    /// Resume paused exploration.
    Resume,
}

/// Resolution status of a guided exploration decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardExplorationDecisionStatus {
    /// Decision still requires operator resolution.
    Unresolved,
    /// Decision has been resolved by the operator.
    Resolved,
}

/// Persisted guided exploration decision record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardExplorationDecision {
    /// Decision identifier (e.g. `DEC-1`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Decision prompt text.
    pub prompt: String,
    /// Current resolution status.
    pub status: WorkflowBoardExplorationDecisionStatus,
    /// Last operator action taken, if any.
    pub last_action: Option<WorkflowBoardExploreAction>,
    /// Recorded operator answer, if resolved.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub answer: Option<String>,
    /// Optional explanation returned during exploration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// Optional recommendation returned during exploration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
}

/// Resumable guided exploration state for a card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardExplorationState {
    /// Current exploration round index (`0` before first round).
    pub round_index: u64,
    /// Whether advancing to the next round requires explicit `continueRound: true`.
    pub pending_round_prompt: bool,
    /// Whether exploration is currently paused.
    pub paused: bool,
    /// IDs of unresolved decisions.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub unresolved_ids: Vec<String>,
    /// Decision records.
    pub decisions: Vec<WorkflowBoardExplorationDecision>,
    /// Digest of exploration interview decisions, if computed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub interview_digest: Option<String>,
}

/// Action on `workflowBoard.challengeStep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowBoardChallengeStepAction {
    /// Start bounded challenge generation.
    Start,
    /// Record operator dispositions on concerns.
    Resolve,
    /// Run single bounded synthesis pass.
    Synthesize,
    /// Complete challenge step with zero unresolved concerns.
    Complete,
    /// Record 3-question `"I acknowledge"` manual fallback.
    ManualFallback,
}

/// Operator disposition for a bounded challenge concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowBoardChallengeDisposition {
    /// Concern has not been resolved yet.
    Unresolved,
    /// Operator answered the concern.
    Answer,
    /// Operator dismissed the concern with a rationale.
    Dismiss,
    /// Operator accepted the concern.
    Accept,
    /// Operator explicitly accepted the risk.
    AcceptRisk,
    /// Operator deferred the concern.
    Defer,
}

/// Lifecycle status of a bounded challenge receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardChallengeStatus {
    /// Challenge not yet started or awaiting input.
    Pending,
    /// Challenge actively in progress.
    InProgress,
    /// All concerns resolved.
    Resolved,
    /// Single synthesis pass completed.
    Synthesized,
    /// Manual 3-question fallback completed.
    ManualFallback,
    /// Challenge completed.
    Complete,
    /// Invalidated by downstream spec edits.
    Stale,
}

/// Grounded challenge concern record (`0..=3` per card).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeConcern {
    /// Concern identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Short summary of the concern.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub summary: String,
    /// Optional explanation of why the concern matters.
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
    /// Operator answer, if any.
    pub answer: Option<String>,
    /// Operator dismissal/deferral reason, if any.
    pub reason: Option<String>,
}

/// Manual fallback payload requiring exact `"I acknowledge"` and 3 answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeManualFallback {
    /// Must equal `"I acknowledge"`.
    pub acknowledgement_token: String,
    /// Exactly three non-empty answers.
    pub answers: [String; 3],
    /// Recording ISO-8601 timestamp.
    pub recorded_at: String,
}

impl<'de> Deserialize<'de> for WorkflowBoardChallengeManualFallback {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            acknowledgement_token: String,
            answers: [String; 3],
            recorded_at: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.acknowledgement_token != "I acknowledge" {
            return Err(de::Error::custom(
                "acknowledgementToken must equal \"I acknowledge\"",
            ));
        }
        let trimmed = raw
            .answers
            .into_iter()
            .map(|a| {
                let t = a.trim();
                if t.is_empty() {
                    Err(de::Error::custom("manualFallback answer cannot be empty"))
                } else {
                    Ok(t.to_owned())
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let answers: [String; 3] = trimmed
            .try_into()
            .map_err(|_| de::Error::custom("manualFallback requires exactly 3 answers"))?;
        Ok(Self {
            acknowledgement_token: raw.acknowledgement_token,
            answers,
            recorded_at: raw.recorded_at,
        })
    }
}

/// Persisted bounded challenge receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardChallengeReceipt {
    /// Current challenge status.
    pub status: WorkflowBoardChallengeStatus,
    /// Bounded list (`<= 3`) of challenge concerns.
    pub concerns: Vec<WorkflowBoardChallengeConcern>,
    /// Number of synthesis passes run (`0..=1`).
    pub synthesis_count: u64,
    /// Bound spec digest, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub spec_digest: Option<String>,
    /// Bound exploration interview digest, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub interview_digest: Option<String>,
    /// Bound source refs digest, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub source_refs_digest: Option<String>,
    /// Manual fallback answers, if used.
    pub manual_fallback: Option<WorkflowBoardChallengeManualFallback>,
    /// Optional synthesis diff summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_summary: Option<String>,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
}
