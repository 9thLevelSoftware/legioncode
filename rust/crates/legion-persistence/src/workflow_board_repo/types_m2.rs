//! Milestone 2 persisted row and input DTOs for [`crate::workflow_board_repo`].

use super::CardDetail;

/// Row representation from `workflow_board_exploration_rounds`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorationRoundRow {
    pub id: String,
    pub card_id: String,
    pub round_index: i64,
    pub status: String,
    pub decision_ids_json: String,
    pub pending_continue_prompt: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_decisions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    pub card_id: String,
    pub decision_id: String,
    pub round_index: Option<i64>,
    pub prompt: String,
    pub recommendation: Option<String>,
    pub depends_on_json: String,
    pub last_action: Option<String>,
    pub status: String,
    pub answer: Option<String>,
    pub rationale: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_challenges`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeRow {
    pub id: String,
    pub card_id: String,
    pub round: i64,
    pub status: String,
    pub spec_digest: String,
    pub interview_digest: String,
    pub source_refs_digest: String,
    pub final_spec_digest: Option<String>,
    pub concerns_json: String,
    pub synthesis_count: i64,
    pub synthesis_diff_json: Option<String>,
    pub manual_fallback_json: Option<String>,
    pub generation_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

/// Row representation from `workflow_board_plan_designs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDesignRow {
    pub card_id: String,
    pub lenses_json: String,
    pub alternatives_json: String,
    pub selected_alternative_id: Option<String>,
    pub selection_rationale: Option<String>,
    pub comparison_revision_count: i64,
    pub strategy: String,
    pub strategy_rationale: Option<String>,
    pub strategy_metadata_json: String,
    pub granularity: String,
    pub design_digest: Option<String>,
    pub updated_at: String,
}

/// Row representation from `workflow_board_tasks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContractRow {
    pub card_id: String,
    pub task_id: String,
    pub ordinal: i64,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub parent_id: Option<String>,
    pub blocked_by_json: String,
    pub blocks_json: String,
    pub files_allowed_json: String,
    pub files_forbidden_json: String,
    pub expected_artifacts_json: String,
    pub verification_commands_json: String,
    pub testing_method: String,
    pub testing_boundary: Option<String>,
    pub acceptance_ids_json: String,
    pub blocker_reason: Option<String>,
    pub notes: Option<String>,
    pub max_files_touched: Option<i64>,
    pub contract_digest: Option<String>,
    pub active_stage_run_id: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub attempt_count: i64,
    pub last_error: Option<String>,
    pub last_diff_digest: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_planned_checks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCheckRow {
    pub card_id: String,
    pub check_id: String,
    pub owner_task_id: Option<String>,
    pub argv_json: String,
    pub label: Option<String>,
    pub status: String,
    pub last_exit_code: Option<i64>,
    pub last_log_ref: Option<String>,
    pub product_snapshot_digest: Option<String>,
    pub stale_reason: Option<String>,
    pub last_receipt_id: Option<String>,
    pub last_run_at: Option<String>,
    pub last_duration_ms: Option<i64>,
    pub last_output_excerpt: Option<String>,
    pub updated_at: String,
}

/// Row representation from `workflow_board_ac_evidence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcEvidenceRow {
    pub card_id: String,
    pub criterion_id: String,
    pub method: String,
    pub check_ids_json: String,
    pub command_argv_json: Option<String>,
    pub procedure: Option<String>,
    pub status: String,
    pub observation_note: Option<String>,
    pub evidence_ref: Option<String>,
    pub evidence_digest: Option<String>,
    pub product_snapshot_digest: Option<String>,
    pub stale_reason: Option<String>,
    pub external_source: Option<String>,
    pub observed_by: Option<String>,
    pub observed_at: Option<String>,
    pub updated_at: String,
}

/// Full hydrated M2 planning aggregate returned by M2 repository operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardPlanningDetail {
    pub card_detail: CardDetail,
    pub exploration_rounds: Vec<ExplorationRoundRow>,
    pub decisions: Vec<DecisionRow>,
    pub challenges: Vec<ChallengeRow>,
    pub plan_design: Option<PlanDesignRow>,
    pub tasks: Vec<TaskContractRow>,
    pub planned_checks: Vec<PlannedCheckRow>,
    pub ac_evidence: Vec<AcEvidenceRow>,
}

/// Single decision action entry for [`ExploreStepInput`].
#[derive(Debug, Clone)]
pub struct ExploreAnswerInput<'a> {
    pub decision_id: &'a str,
    pub action: &'a str,
    pub answer: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub recommendation: Option<&'a str>,
    pub rationale: Option<&'a str>,
}

/// Input parameters for [`super::explore_step`].
#[derive(Debug, Clone)]
pub struct ExploreStepInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub action: Option<&'a str>,
    pub continue_round: Option<bool>,
    pub answers: &'a [ExploreAnswerInput<'a>],
}

/// Seed decision row input for [`super::seed_exploration_decisions`].
#[derive(Debug, Clone)]
pub struct SeedDecisionInput<'a> {
    pub decision_id: &'a str,
    pub prompt: &'a str,
    pub recommendation: Option<&'a str>,
    pub rationale: Option<&'a str>,
    pub depends_on: &'a [&'a str],
}

/// Concern entry for [`ChallengeStepInput`].
#[derive(Debug, Clone)]
pub struct ChallengeConcernInput<'a> {
    pub id: &'a str,
    pub summary: &'a str,
    pub why_matters: Option<&'a str>,
    pub evidence: Option<&'a str>,
    pub assumption: Option<&'a str>,
    pub disposition: &'a str,
    pub answer: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub mutated_answer: Option<&'a str>,
}

/// Input parameters for [`super::challenge_step`].
#[derive(Debug, Clone)]
pub struct ChallengeStepInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub challenge_id: &'a str,
    pub expected_revision: i64,
    pub action: &'a str,
    pub concerns: &'a [ChallengeConcernInput<'a>],
    pub acknowledgement_token: Option<&'a str>,
    pub unresolved_concern_ids: &'a [&'a str],
    pub manual_answers: Option<&'a [&'a str]>,
    pub spec_digest: Option<&'a str>,
    pub interview_digest: Option<&'a str>,
    pub source_refs_digest: Option<&'a str>,
    pub synthesis_diff_json: Option<&'a str>,
    pub approved: Option<bool>,
}

/// Design lens input entry for [`UpdatePlanInput`].
#[derive(Debug, Clone)]
pub struct LensInput<'a> {
    pub lens: &'a str,
    pub applicable: Option<bool>,
    pub summary: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// Design alternative input entry for [`UpdatePlanInput`].
#[derive(Debug, Clone)]
pub struct AlternativeInput<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub summary: &'a str,
    pub tradeoffs: &'a str,
}

/// Task contract input entry for [`UpdatePlanInput`].
#[derive(Debug, Clone)]
pub struct TaskContractInput<'a> {
    pub task_id: &'a str,
    pub title: &'a str,
    pub depends_on: &'a [&'a str],
    pub blocks: &'a [&'a str],
    pub files_allowed: &'a [&'a str],
    pub files_forbidden: &'a [&'a str],
    pub expected_artifacts: &'a [&'a str],
    pub verification_commands: &'a [Vec<String>],
    pub testing_method: &'a str,
    pub testing_limitations: Option<&'a str>,
    pub status: Option<&'a str>,
}

/// Planned check input entry for [`UpdatePlanInput`].
#[derive(Debug, Clone)]
pub struct PlannedCheckInput<'a> {
    pub check_id: &'a str,
    pub owner_task_id: Option<&'a str>,
    pub argv: &'a [String],
    pub label: Option<&'a str>,
}

/// Acceptance criterion evidence mapping input entry for [`UpdatePlanInput`].
#[derive(Debug, Clone)]
pub struct AcMappingInput<'a> {
    pub criterion_id: &'a str,
    pub method: &'a str,
    pub check_ids: &'a [&'a str],
    pub command_argv: Option<&'a [String]>,
    pub procedure: Option<&'a str>,
    pub evidence_ref: Option<&'a str>,
    pub external_source: Option<&'a str>,
}

/// Selection update directive for `selected_alternative_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlternativeSelectionUpdate<'a> {
    /// Leave existing selection unchanged.
    Keep,
    /// Clear existing selection (`NULL`).
    Clear,
    /// Set selection to the specified alternative ID.
    Set(&'a str),
}

/// Input parameters for [`super::update_plan`].
#[derive(Debug, Clone)]
pub struct UpdatePlanInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub lenses: Option<&'a [LensInput<'a>]>,
    pub alternatives: Option<&'a [AlternativeInput<'a>]>,
    pub selected_alternative_id: AlternativeSelectionUpdate<'a>,
    pub selection_rationale: Option<&'a str>,
    pub revise_alternatives_feedback: Option<&'a str>,
    pub strategy: Option<&'a str>,
    pub strategy_rationale: Option<&'a str>,
    pub strategy_metadata_json: Option<&'a str>,
    pub granularity: Option<&'a str>,
    pub tasks: Option<&'a [TaskContractInput<'a>]>,
    pub checks: Option<&'a [PlannedCheckInput<'a>]>,
    pub ac_mappings: Option<&'a [AcMappingInput<'a>]>,
    pub plan_json: Option<&'a str>,
    pub plan_digest: Option<&'a str>,
    pub design_digest: Option<&'a str>,
    pub tasks_digest: Option<&'a str>,
}

/// Input parameters for [`super::approve_plan`].
#[derive(Debug, Clone)]
pub struct ApprovePlanInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub approval_id: &'a str,
    pub expected_revision: i64,
    pub expected_spec_digest: Option<&'a str>,
    pub expected_plan_digest: Option<&'a str>,
    pub approved_by: &'a str,
}

/// Input parameters for [`super::return_for_revision`].
#[derive(Debug, Clone)]
pub struct ReturnForRevisionInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub target_stage: &'a str,
    pub reason: &'a str,
}
