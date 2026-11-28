//! M3 RPC request and response payloads for Execute, Review, Amendments, Recovery, and Ship.

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::ids::WorkflowCardId;

use super::m2_models::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardPlannedCheck, WorkflowBoardTaskContract,
};
use super::m3_models::WorkflowBoardTaskFileContractReport;
pub use super::m3_rpc_ship::*;
use super::m3_ship_models::{
    WorkflowBoardAmendmentTargetStage, WorkflowBoardDiffFileEntry,
    WorkflowBoardPlanAmendmentRecord, WorkflowBoardRetryStepKind,
};
use super::m3_validators::{
    deserialize_present_artifact_key_vec, validate_propose_amendment, validate_retry_step,
};
use super::validators::{
    deserialize_opt_trimmed_non_empty, deserialize_positive_revision, deserialize_present_some,
    deserialize_present_trimmed_non_empty, deserialize_trimmed_non_empty,
};
use super::{
    WorkflowBoardApprovalReceipt, WorkflowBoardCardStatus, WorkflowBoardStage, WorkflowCardDetail,
};

/// Input for `workflowBoard.getCardDiff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardGetCardDiffInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Optional task scope filter.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub task_id: Option<String>,
    /// Whether unified diff patches should be included.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub include_patches: Option<bool>,
}

/// Output for `workflowBoard.getCardDiff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardGetCardDiffResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Card worktree path, if resolved.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub worktree_path: Option<String>,
    /// Base git reference compared against.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub base_ref: Option<String>,
    /// Product snapshot digest at diff capture time.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub product_snapshot_digest: Option<String>,
    /// Content digest of the diff.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub diff_digest: Option<String>,
    /// Changed files in the card worktree.
    pub files: Vec<WorkflowBoardDiffFileEntry>,
    /// File-contract validation report when evaluated against a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_contract_report: Option<WorkflowBoardTaskFileContractReport>,
    /// `true` if patches or entries were truncated.
    pub truncated: bool,
    /// ISO-8601 capture timestamp.
    pub generated_at: String,
}

/// Input for `workflowBoard.pauseCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPauseCardInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Optional operator pause reason.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub reason: Option<String>,
}

/// Output for `workflowBoard.pauseCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPauseCardResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Resulting card status (`paused`).
    pub status: WorkflowBoardCardStatus,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.resumeCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardResumeCardInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Reconcile interrupted actions during resume if applicable.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub reconcile_interrupted: Option<bool>,
    /// Optional operator resume note.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub note: Option<String>,
}

/// Output for `workflowBoard.resumeCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardResumeCardResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Restored card status (`active` or `blocked`).
    pub status: WorkflowBoardCardStatus,
    /// Number of interrupted actions reconciled.
    pub reconciled_count: u64,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.retryStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRetryStepInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Step kind to retry (`task`, `check`, `review`, `stage`).
    pub step_kind: WorkflowBoardRetryStepKind,
    /// Required target task or check ID when `step_kind` is `task` or `check`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// Optional operator retry note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl<'de> Deserialize<'de> for WorkflowBoardRetryStepInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            step_kind: WorkflowBoardRetryStepKind,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            target_id: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            reason: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        validate_retry_step(raw.step_kind, raw.target_id.as_deref()).map_err(de::Error::custom)?;
        Ok(Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            step_kind: raw.step_kind,
            target_id: raw.target_id,
            reason: raw.reason,
        })
    }
}

/// Output for `workflowBoard.retryStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRetryStepResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Retried step kind.
    pub step_kind: WorkflowBoardRetryStepKind,
    /// Target task/check ID if applicable.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub target_id: Option<String>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}

/// Input for `workflowBoard.proposeAmendment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardProposeAmendmentInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    pub expected_revision: u64,
    /// Target stage (`spec` or `plan`, defaults to `plan`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_stage: Option<WorkflowBoardAmendmentTargetStage>,
    /// Reason motivating the amendment.
    pub reason: String,
    /// Optional amendment summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Optional source task ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_task_id: Option<String>,
    /// Unexpected files triggering the amendment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unexpected_files: Option<Vec<String>>,
    /// Proposed replacement/amended task contracts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<WorkflowBoardTaskContract>>,
    /// Proposed replacement/amended planned checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_checks: Option<Vec<WorkflowBoardPlannedCheck>>,
    /// Proposed replacement/amended AC evidence mappings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac_mappings: Option<Vec<WorkflowBoardAcEvidenceMapping>>,
    /// Proposed deliverable artifact keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverables: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for WorkflowBoardProposeAmendmentInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            card_id: WorkflowCardId,
            #[serde(deserialize_with = "deserialize_positive_revision")]
            expected_revision: u64,
            #[serde(default, deserialize_with = "deserialize_present_some")]
            target_stage: Option<WorkflowBoardAmendmentTargetStage>,
            #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
            reason: String,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            summary: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_trimmed_non_empty")]
            source_task_id: Option<String>,
            #[serde(default, deserialize_with = "deserialize_present_artifact_key_vec")]
            unexpected_files: Option<Vec<String>>,
            #[serde(default, deserialize_with = "deserialize_present_some")]
            tasks: Option<Vec<WorkflowBoardTaskContract>>,
            #[serde(default, deserialize_with = "deserialize_present_some")]
            planned_checks: Option<Vec<WorkflowBoardPlannedCheck>>,
            #[serde(default, deserialize_with = "deserialize_present_some")]
            ac_mappings: Option<Vec<WorkflowBoardAcEvidenceMapping>>,
            #[serde(default, deserialize_with = "deserialize_present_artifact_key_vec")]
            deliverables: Option<Vec<String>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        validate_propose_amendment(
            raw.tasks.as_deref(),
            raw.planned_checks.as_deref(),
            raw.ac_mappings.as_deref(),
            raw.deliverables.as_deref(),
        )
        .map_err(de::Error::custom)?;
        Ok(Self {
            card_id: raw.card_id,
            expected_revision: raw.expected_revision,
            target_stage: raw.target_stage,
            reason: raw.reason,
            summary: raw.summary,
            source_task_id: raw.source_task_id,
            unexpected_files: raw.unexpected_files,
            tasks: raw.tasks,
            planned_checks: raw.planned_checks,
            ac_mappings: raw.ac_mappings,
            deliverables: raw.deliverables,
        })
    }
}

/// Output for `workflowBoard.proposeAmendment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardProposeAmendmentResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Stage transitioned to (`spec` or `plan`).
    pub stage: WorkflowBoardStage,
    /// Recorded amendment entry.
    pub amendment: WorkflowBoardPlanAmendmentRecord,
    /// Updated spec approval receipt.
    pub spec_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Updated plan approval receipt.
    pub plan_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}
