//! Spec update and approval RPC payloads for `workflowBoard.*` methods.

use serde::{Deserialize, Serialize};

use crate::ids::WorkflowCardId;
use crate::orchestration::PatchValue;

use super::m2_models::{WorkflowBoardIntakeGoal, WorkflowBoardSpecDiff};
use super::validators::{
    deserialize_opt_artifact_key, deserialize_opt_trimmed_non_empty,
    deserialize_patch_artifact_key, deserialize_positive_revision, deserialize_present_input_paths,
    deserialize_present_some, deserialize_present_trimmed_non_empty,
    deserialize_present_trimmed_non_empty_vec,
};
use super::{
    WorkflowBoardAcceptanceCriterion, WorkflowBoardApprovalReceipt, WorkflowBoardApprovalStatus,
    WorkflowCardDetail,
};

/// Input for `workflowBoard.updateSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardUpdateSpecInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Optional updated title.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub title: Option<String>,
    /// Optional updated spec brief body.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub brief: Option<String>,
    /// Optional updated intake goal.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub intake_goal: Option<WorkflowBoardIntakeGoal>,
    /// Optional updated input paths.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_input_paths"
    )]
    pub input_paths: Option<Vec<String>>,
    /// Optional updated acceptance criteria list.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub acceptance_criteria: Option<Vec<WorkflowBoardAcceptanceCriterion>>,
    /// Optional updated constraint strings.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty_vec"
    )]
    pub constraints: Option<Vec<String>>,
    /// Optional updated source reference paths.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty_vec"
    )]
    pub source_refs: Option<Vec<String>>,
    /// Wireframe entry point; explicit `null` clears wireframes for non-UI cards.
    #[serde(
        default,
        skip_serializing_if = "PatchValue::is_absent",
        deserialize_with = "deserialize_patch_artifact_key"
    )]
    pub wireframes_index: PatchValue<String>,
}

/// Output for `workflowBoard.updateSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardUpdateSpecResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Current spec digest (`null` when empty/unrendered).
    #[serde(default, deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub spec_digest: Option<String>,
    /// Current spec approval status (`null` when never approved).
    pub spec_approval_status: Option<WorkflowBoardApprovalStatus>,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
    /// Structured spec diff when content changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<WorkflowBoardSpecDiff>,
    /// Wireframe entry key (`null` serialized explicitly when disabled).
    #[serde(default, deserialize_with = "deserialize_opt_artifact_key")]
    pub wireframes_index: Option<String>,
    /// Updated spec approval receipt, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_approval: Option<WorkflowBoardApprovalReceipt>,
    /// Updated plan approval receipt, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_approval: Option<WorkflowBoardApprovalReceipt>,
}

/// Input for `workflowBoard.approveSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApproveSpecInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Optional expected spec digest precondition.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub expected_digest: Option<String>,
}

/// Output for `workflowBoard.approveSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApproveSpecResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Newly recorded content-bound spec approval receipt.
    pub spec_approval: WorkflowBoardApprovalReceipt,
    /// Full updated card detail.
    pub card: WorkflowCardDetail,
}
