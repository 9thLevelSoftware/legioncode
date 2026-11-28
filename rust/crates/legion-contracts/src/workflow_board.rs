//! Typed Project Workflow Board domain contracts mirroring `packages/contracts/src/workflowBoard.ts`.

mod error;
mod m1_records;
pub mod m2_models;
mod m2_plan_models;
mod m2_plan_validators;
pub mod m2_rpc;
mod m2_rpc_plan;
mod m2_rpc_spec;
mod m2_spec_models;
mod m2_task_models;
pub mod m2_validators;
mod m3_enums;
pub mod m3_models;
pub mod m3_rpc;
mod m3_rpc_ship;
pub mod m3_ship_models;
mod m3_validators;
pub mod m4_models;
pub mod m4_rpc;
pub mod rpc;
mod rpc_spec;
mod validators;

use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, ThreadId, WorkflowBoardApprovalId, WorkflowCardId};
use crate::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use crate::policy::RuntimeMode;

pub use m1_records::*;
pub use m2_models::*;
pub use m2_validators::{
    is_credential_file_path, is_valid_argv_command, is_valid_input_path,
    is_valid_non_git_artifact_key,
};
pub use m3_models::*;
pub use m3_ship_models::*;
pub use m3_validators::normalize_review_finding_severity;
pub use m4_models::*;
pub use rpc::*;
pub use validators::is_valid_artifact_key;
use validators::{
    deserialize_artifact_key_vec, deserialize_opt_artifact_key, deserialize_opt_positive_revision,
    deserialize_opt_trimmed_non_empty, deserialize_positive_revision, deserialize_present_some,
    deserialize_present_trimmed_non_empty_vec, deserialize_trimmed_non_empty,
    deserialize_trimmed_non_empty_vec,
};

/// Structured specification state for a workflow board card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardSpecDocument {
    /// Card title.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub title: String,
    /// Brief markdown or text.
    pub brief: String,
    /// Intake source type.
    pub source_type: WorkflowBoardSpecSourceType,
    /// Spec assistance mode.
    pub assistance_mode: WorkflowBoardAssistanceMode,
    /// Defined acceptance criteria.
    pub acceptance_criteria: Vec<WorkflowBoardAcceptanceCriterion>,
    /// Explicit non-functional or scope constraints.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub constraints: Vec<String>,
    /// Referenced workspace paths.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub source_refs: Vec<String>,
    /// Optional wireframe index key (`null` for non-UI features).
    #[serde(deserialize_with = "deserialize_opt_artifact_key")]
    pub wireframes_index: Option<String>,
    /// SHA-256 digest of the rendered `spec.md` deliverable, if computed.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub spec_digest: Option<String>,
    /// Last spec update ISO-8601 timestamp.
    pub updated_at: String,
    /// Optional intake goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_goal: Option<WorkflowBoardIntakeGoal>,
    /// Optional workspace-relative brief file path.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_artifact_key"
    )]
    pub brief_path: Option<String>,
    /// Optional brief SHA-256 digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub brief_digest: Option<String>,
    /// Optional primary source digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub source_digest: Option<String>,
    /// Digests of ingested workspace input paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_input_digests: Vec<WorkflowBoardSourceInputDigest>,
    /// Intake decision analysis summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake_analysis: Option<WorkflowBoardSpecIntakeAnalysis>,
    /// Optional wireframe digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub wireframes_digest: Option<String>,
}

/// Revision- and digest-bound approval receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApprovalReceipt {
    /// Receipt identifier.
    pub id: WorkflowBoardApprovalId,
    /// Gate approved by this receipt.
    pub gate: WorkflowBoardApprovalGate,
    /// Current receipt status.
    pub status: WorkflowBoardApprovalStatus,
    /// Card revision at approval time (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Primary artifact SHA-256 digest bound by this approval.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub artifact_digest: String,
    /// Spec digest bound by this approval, if applicable.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub spec_digest: Option<String>,
    /// Wireframe digest bound by this approval, if present.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub wireframes_digest: Option<String>,
    /// Approval ISO-8601 timestamp.
    pub approved_at: String,
    /// Principal that approved (`"client"` or subject identifier).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub approved_by: String,
    /// Plan digest bound by Plan approval, if applicable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub plan_digest: Option<String>,
    /// Task contracts digest bound by Plan approval, if applicable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub tasks_digest: Option<String>,
    /// Planned checks digest bound by Plan approval, if applicable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub checks_digest: Option<String>,
    /// Bound acceptance criterion IDs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty_vec"
    )]
    pub acceptance_criteria_ids: Option<Vec<String>>,
    /// Product snapshot digest bound by this approval, if applicable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub product_snapshot_digest: Option<String>,
}

/// Compact card summary streamed by `workflowBoard.subscribe` without heavy markdown/diff/transcript payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCardSummary {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Owning project identifier.
    pub project_id: ProjectId,
    /// Card title.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub title: String,
    /// Current workflow stage column.
    pub stage: WorkflowBoardStage,
    /// Current card status.
    pub status: WorkflowBoardCardStatus,
    /// Monotonic card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Current active task identifier/label, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub current_task: Option<String>,
    /// Blocker description when `status == Blocked`.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub blocker_summary: Option<String>,
    /// Recommended single next action key/label.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub next_action: Option<String>,
    /// Revision of latest spec approval, if any.
    #[serde(deserialize_with = "deserialize_opt_positive_revision")]
    pub spec_approved_revision: Option<u64>,
    /// Status of spec approval, if any.
    pub spec_approval_status: Option<WorkflowBoardApprovalStatus>,
    /// Active linked thread identifier, if any.
    pub active_thread_id: Option<ThreadId>,
    /// Creation ISO-8601 timestamp.
    pub created_at: String,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
    /// Revision of latest plan approval, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_positive_revision"
    )]
    pub plan_approved_revision: Option<u64>,
    /// Status of plan approval, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_approval_status: Option<WorkflowBoardApprovalStatus>,
    /// Incomplete write-journal entry count, if tracked.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub incomplete_journal_count: Option<u64>,
    /// Whether unreconciled external edits exist.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub has_unreconciled_edits: Option<bool>,
    /// Delivery lifecycle status (`pending | blocked | ready | shipped`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<WorkflowBoardDeliveryStatus>,
}

/// Compact project board snapshot emitted by `workflowBoard.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectWorkflowBoardSnapshot {
    /// Owning project identifier.
    pub project_id: ProjectId,
    /// Aggregate project board revision (`>= 0`).
    pub revision: u64,
    /// Compact card summaries ordered for display.
    pub cards: Vec<WorkflowBoardCardSummary>,
    /// Snapshot generation ISO-8601 timestamp.
    pub generated_at: String,
}

/// Full card detail returned by `workflowBoard.getCard` and `workflowBoard.createCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCardDetail {
    /// Flattened compact summary fields.
    #[serde(flatten)]
    pub summary: WorkflowBoardCardSummary,
    /// Configured card workspace launch strategy.
    pub workspace_strategy: WorkspaceStrategy,
    /// Configured default model selection, if set.
    pub model_selection: Option<ModelSelection>,
    /// Configured default runtime mode, if set.
    pub runtime_mode: Option<RuntimeMode>,
    /// Configured default provider interaction mode, if set.
    pub interaction_mode: Option<ProviderInteractionMode>,
    /// Current spec state.
    pub spec: WorkflowBoardSpecDocument,
    /// Current or latest spec approval receipt, if any.
    pub spec_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Linked stage runs.
    pub stage_runs: Vec<WorkflowBoardStageRunLink>,
    /// Revision history ledger.
    pub revisions: Vec<WorkflowBoardRevisionEntry>,
    /// Relative artifact keys available for `workflowBoard.getArtifact`.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub artifact_keys: Vec<String>,
    /// Top-level wireframe index key (`null` serialized explicitly when disabled).
    #[serde(default, deserialize_with = "deserialize_opt_artifact_key")]
    pub wireframes_index: Option<String>,
    /// Bound wireframe bundle digest, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub wireframes_digest: Option<String>,
    /// Guided exploration state, if started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exploration: Option<WorkflowBoardExplorationState>,
    /// Bounded challenge receipt, if started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge_receipt: Option<WorkflowBoardChallengeReceipt>,
    /// Maximum challenge concerns cap (`<= 3`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub max_challenge_concerns: Option<u64>,
    /// Structured M2 plan document, if authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<WorkflowBoardPlanDocument>,
    /// Current or latest plan approval receipt, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Planning design lenses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lenses: Vec<WorkflowBoardPlanLensEntry>,
    /// Compared design alternatives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<WorkflowBoardPlanAlternative>,
    /// Alternative-comparison revision count (`0..=1`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub alternatives_revision_count: Option<u64>,
    /// Selected alternative ID, if chosen.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub selected_alternative_id: Option<String>,
    /// Selected alternative rationale, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_rationale: Option<String>,
    /// Selected planning strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<WorkflowBoardPlanningStrategy>,
    /// Custom strategy rationale, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy_rationale: Option<String>,
    /// Strategy metadata, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy_metadata: Option<WorkflowBoardStrategyMetadata>,
    /// Task granularity, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<WorkflowBoardTaskGranularity>,
    /// Task contracts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<WorkflowBoardTaskContract>,
    /// Planned checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<WorkflowBoardPlannedCheck>,
    /// AC evidence mappings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ac_mappings: Vec<WorkflowBoardAcEvidenceMapping>,
    /// Revision history alias for `revisions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<WorkflowBoardRevisionEntry>,
    /// Per-task execution bindings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_executions: Vec<WorkflowBoardTaskExecutionState>,
    /// Post-turn file-contract diff validation reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_contract_reports: Vec<WorkflowBoardTaskFileContractReport>,
    /// Snapshot-bound planned check receipts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_receipts: Vec<WorkflowBoardPlannedCheckReceipt>,
    /// Latest independent fresh-thread review report, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_report: Option<WorkflowBoardReviewReport>,
    /// Hydrated acceptance criterion observation records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ac_observations: Vec<WorkflowBoardAcObservationRecord>,
    /// Computed Ship gate readiness summary, if evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship_readiness: Option<WorkflowBoardShipReadiness>,
    /// Human-approved delivery receipt, if shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_receipt: Option<WorkflowBoardDeliveryReceipt>,
    /// Write-journal and external-edit recovery state, if hydrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_state: Option<WorkflowBoardRecoveryState>,
    /// Recorded plan/spec amendment proposals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub amendments: Vec<WorkflowBoardPlanAmendmentRecord>,
    /// Imported `.legion-cli` provenance, policy, and historical receipt metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub cli_import: Option<WorkflowBoardCliImportProvenance>,
}
