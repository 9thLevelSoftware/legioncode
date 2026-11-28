//! RPC payloads for Milestone 2 Plan authoring (`updatePlan`, `approvePlan`, `returnForRevision`).

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::ids::WorkflowCardId;
use crate::orchestration::PatchValue;

use super::m2_models::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardPlanAlternative, WorkflowBoardPlanLensEntry,
    WorkflowBoardPlannedCheck, WorkflowBoardPlanningStrategy, WorkflowBoardReturnTargetStage,
    WorkflowBoardStrategyMetadata, WorkflowBoardTaskContract, WorkflowBoardTaskGranularity,
    validate_opt_alternatives, validate_opt_lenses,
};
use super::m2_validators::{
    deserialize_patch_trimmed_non_empty, validate_ac_mappings_partial,
    validate_tasks_dag_and_contracts,
};
use super::validators::{
    deserialize_opt_trimmed_non_empty, deserialize_positive_revision, deserialize_trimmed_non_empty,
};
use super::{
    WorkflowBoardApprovalReceipt, WorkflowBoardRevisionEntry, WorkflowBoardStage,
    WorkflowCardDetail,
};

/// Input for `workflowBoard.updatePlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardUpdatePlanInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Partial or full design lens entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lenses: Option<Vec<WorkflowBoardPlanLensEntry>>,
    /// Two materially distinct design alternatives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<Vec<WorkflowBoardPlanAlternative>>,
    /// Selected alternative ID (`PatchValue::Null` clears selection).
    #[serde(default, skip_serializing_if = "PatchValue::is_absent")]
    pub selected_alternative_id: PatchValue<String>,
    /// Selection rationale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_rationale: Option<String>,
    /// Feedback requesting a single alternatives revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revise_alternatives_feedback: Option<String>,
    /// Planning decomposition strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<WorkflowBoardPlanningStrategy>,
    /// Strategy rationale (required for `custom`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy_rationale: Option<String>,
    /// Strategy metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy_metadata: Option<WorkflowBoardStrategyMetadata>,
    /// Task granularity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<WorkflowBoardTaskGranularity>,
    /// Task contracts forming an acyclic DAG with exclusive `filesAllowed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<WorkflowBoardTaskContract>>,
    /// Planned argv checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<WorkflowBoardPlannedCheck>>,
    /// AC evidence mappings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac_mappings: Option<Vec<WorkflowBoardAcEvidenceMapping>>,
}

fn validate_update_plan_input(input: &WorkflowBoardUpdatePlanInput) -> Result<(), String> {
    validate_opt_lenses(input.lenses.as_deref())?;
    validate_opt_alternatives(input.alternatives.as_deref())?;
    if let Some(ref tasks) = input.tasks {
        validate_tasks_dag_and_contracts(tasks)?;
    }
    if let Some(ref mappings) = input.ac_mappings {
        validate_ac_mappings_partial(mappings)?;
    }
    if let PatchValue::Set(ref selected) = input.selected_alternative_id {
        if input
            .selection_rationale
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            return Err("selectedAlternativeId requires a non-empty selectionRationale".to_owned());
        }
        if let Some(ref alts) = input.alternatives
            && !alts.iter().any(|a| a.id.trim() == selected.trim())
        {
            return Err("selectedAlternativeId does not match either alternative".to_owned());
        }
    }
    match input.strategy {
        Some(WorkflowBoardPlanningStrategy::Custom)
            if input
                .strategy_rationale
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty) =>
        {
            return Err("custom planning strategy requires non-empty strategyRationale".to_owned());
        }
        Some(WorkflowBoardPlanningStrategy::RiskFirst) => {
            let valid = input.strategy_metadata.as_ref().is_some_and(|m| {
                m.uncertainty.as_deref().is_some_and(|s| !s.is_empty())
                    && m.probe_task_id.as_deref().is_some_and(|s| !s.is_empty())
            });
            if !valid {
                return Err(
                    "risk-first strategy requires strategyMetadata.uncertainty and probeTaskId"
                        .to_owned(),
                );
            }
        }
        Some(WorkflowBoardPlanningStrategy::ExpandContract) => {
            let valid = input.strategy_metadata.as_ref().is_some_and(|m| {
                m.compatibility_window
                    .as_deref()
                    .is_some_and(|s| !s.is_empty())
                    && m.transition_step.as_deref().is_some_and(|s| !s.is_empty())
                    && m.retirement_prereq
                        .as_deref()
                        .is_some_and(|s| !s.is_empty())
            });
            if !valid {
                return Err(
                    "expand-contract strategy requires compatibilityWindow, transitionStep, and retirementPrereq"
                        .to_owned(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

impl<'de> Deserialize<'de> for WorkflowBoardUpdatePlanInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            #[serde(default)]
            lenses: Option<Vec<WorkflowBoardPlanLensEntry>>,
            #[serde(default)]
            alternatives: Option<Vec<WorkflowBoardPlanAlternative>>,
            #[serde(default, deserialize_with = "deserialize_patch_trimmed_non_empty")]
            selected_alternative_id: PatchValue<String>,
            #[serde(default)]
            selection_rationale: Option<String>,
            #[serde(default, deserialize_with = "deserialize_opt_trimmed_non_empty")]
            revise_alternatives_feedback: Option<String>,
            #[serde(default)]
            strategy: Option<WorkflowBoardPlanningStrategy>,
            #[serde(default)]
            strategy_rationale: Option<String>,
            #[serde(default)]
            strategy_metadata: Option<WorkflowBoardStrategyMetadata>,
            #[serde(default)]
            granularity: Option<WorkflowBoardTaskGranularity>,
            #[serde(default)]
            tasks: Option<Vec<WorkflowBoardTaskContract>>,
            #[serde(default)]
            checks: Option<Vec<WorkflowBoardPlannedCheck>>,
            #[serde(default)]
            ac_mappings: Option<Vec<WorkflowBoardAcEvidenceMapping>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let out = Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            lenses: raw.lenses,
            alternatives: raw.alternatives,
            selected_alternative_id: raw.selected_alternative_id,
            selection_rationale: raw.selection_rationale,
            revise_alternatives_feedback: raw.revise_alternatives_feedback,
            strategy: raw.strategy,
            strategy_rationale: raw.strategy_rationale,
            strategy_metadata: raw.strategy_metadata,
            granularity: raw.granularity,
            tasks: raw.tasks,
            checks: raw.checks,
            ac_mappings: raw.ac_mappings,
        };
        validate_update_plan_input(&out).map_err(de::Error::custom)?;
        Ok(out)
    }
}

/// Output for `workflowBoard.updatePlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardUpdatePlanResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Updated plan digest, if computed.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub plan_digest: Option<String>,
    /// Current spec approval receipt (preserved on plan-only edits).
    pub spec_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Current plan approval receipt (marked `stale` when plan changes).
    pub plan_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Design lenses.
    pub lenses: Vec<WorkflowBoardPlanLensEntry>,
    /// Compared alternatives.
    pub alternatives: Vec<WorkflowBoardPlanAlternative>,
    /// Selected alternative ID, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub selected_alternative_id: Option<String>,
    /// Selection rationale, if any.
    pub selection_rationale: Option<String>,
    /// Planning strategy, if set.
    pub strategy: Option<WorkflowBoardPlanningStrategy>,
    /// Strategy metadata, if set.
    pub strategy_metadata: Option<WorkflowBoardStrategyMetadata>,
    /// Task granularity, if set.
    pub granularity: Option<WorkflowBoardTaskGranularity>,
    /// Task contracts.
    pub tasks: Vec<WorkflowBoardTaskContract>,
    /// AC evidence mappings.
    pub ac_mappings: Vec<WorkflowBoardAcEvidenceMapping>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.approvePlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApprovePlanInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Optional expected spec digest precondition.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub expected_spec_digest: Option<String>,
    /// Optional expected plan digest precondition.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub expected_plan_digest: Option<String>,
}

/// Output for `workflowBoard.approvePlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApprovePlanResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Newly recorded content-bound plan approval receipt.
    pub plan_approval: WorkflowBoardApprovalReceipt,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.returnForRevision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardReturnForRevisionInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Earlier stage to return to (`spec`, `plan`, or `execute`).
    pub target_stage: WorkflowBoardReturnTargetStage,
    /// Non-empty rationale for returning the card.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub reason: String,
}

/// Output for `workflowBoard.returnForRevision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardReturnForRevisionResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Resulting card stage.
    pub stage: WorkflowBoardStage,
    /// Updated spec approval receipt, if any.
    pub spec_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Updated plan approval receipt, if any.
    pub plan_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Preserved revision history ledger.
    pub history: Vec<WorkflowBoardRevisionEntry>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}
