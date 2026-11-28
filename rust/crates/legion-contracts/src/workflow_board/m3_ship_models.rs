//! M3 Ship blocker, delivery receipt, amendment, and card-diff models.

use serde::{Deserialize, Serialize};

use crate::ids::WorkflowBoardApprovalId;

use super::WorkflowBoardStage;
use super::m3_models::{WorkflowBoardDeliveryReceiptStatus, WorkflowBoardDeliveryStatus};
use super::validators::{
    deserialize_artifact_key, deserialize_artifact_key_vec, deserialize_opt_artifact_key,
    deserialize_opt_trimmed_non_empty, deserialize_positive_revision, deserialize_present_some,
    deserialize_trimmed_non_empty,
};

/// Deterministic Ship gate blocker kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardShipBlockerKind {
    /// One or more tasks are not completed/skipped.
    TaskIncomplete,
    /// Open file-contract violation requires resolution.
    ContractViolation,
    /// Planned check failed.
    CheckFailed,
    /// Planned check receipt is stale.
    CheckStale,
    /// Planned check has no execution receipt.
    CheckMissing,
    /// Independent review has unresolved findings or failed.
    ReviewUnresolved,
    /// Independent review is missing or stale.
    ReviewStale,
    /// Acceptance criterion lacks current `pass/not_applicable` evidence.
    AcUnverified,
    /// External file edits require reconciliation.
    ExternalEdit,
    /// Incomplete write-journal entries require recovery.
    JournalIncomplete,
    /// Plan approval is missing or stale.
    PlanStale,
    /// Spec approval is missing or stale.
    SpecStale,
    /// Unsupported adopted CLI policy requires operator acknowledgement.
    PolicyUnacknowledged,
}

/// UI navigation pointer directing the user to where a Ship blocker can be fixed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardFixPointer {
    /// Target board stage column.
    pub stage: WorkflowBoardStage,
    /// Target sub-tab within the stage (e.g. `tasks`, `checks`, `review`, `acceptance`, `recovery`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub tab: String,
    /// Specific task/check/finding/criterion ID, if applicable.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub target_id: Option<String>,
}

/// Single Ship readiness blocker entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardShipBlocker {
    /// Stable blocker identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Blocker classification.
    pub kind: WorkflowBoardShipBlockerKind,
    /// Human-readable explanation.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub message: String,
    /// Jump-to-fix pointer.
    pub pointer: WorkflowBoardFixPointer,
    /// Whether this blocker prevents delivery approval.
    pub blocking: bool,
}

/// Human-approved Ship delivery receipt bound to artifact and product snapshot digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardDeliveryReceipt {
    /// Receipt identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Linked delivery approval receipt ID, if recorded.
    pub approval_id: Option<WorkflowBoardApprovalId>,
    /// Card revision at delivery approval (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub card_revision: u64,
    /// Product snapshot digest verified at delivery time.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub product_snapshot_digest: String,
    /// Git HEAD SHA at delivery time, if available.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub commit_sha: Option<String>,
    /// Bound spec digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub spec_digest: Option<String>,
    /// Bound plan digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub plan_digest: Option<String>,
    /// Bound tasks digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub tasks_digest: Option<String>,
    /// Bound checks digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub checks_digest: Option<String>,
    /// Linked independent review ID.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub review_id: Option<String>,
    /// JSON summary of acceptance criterion verdicts.
    pub ac_verdicts_summary: String,
    /// Human principal who approved delivery.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub approved_by: String,
    /// Approval timestamp.
    pub approved_at: String,
    /// Current validity of the recorded delivery receipt.
    pub status: WorkflowBoardDeliveryReceiptStatus,
}

/// Aggregated Ship readiness summary hydrated on `WorkflowCardDetail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardShipReadiness {
    /// Overall delivery readiness status.
    pub status: WorkflowBoardDeliveryStatus,
    /// `true` when zero blocking issues remain.
    pub ready: bool,
    /// Current product snapshot digest, if known.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub product_snapshot_digest: Option<String>,
    /// Active Ship blockers with fix pointers.
    pub blockers: Vec<WorkflowBoardShipBlocker>,
    /// Latest delivery receipt, if any.
    pub delivery_receipt: Option<WorkflowBoardDeliveryReceipt>,
}

/// Retry step target kind for `workflowBoard.retryStep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardRetryStepKind {
    /// Retry a task contract step.
    Task,
    /// Retry a planned check.
    Check,
    /// Retry independent review.
    Review,
    /// Retry the active stage.
    Stage,
}

/// Target stage for `workflowBoard.proposeAmendment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardAmendmentTargetStage {
    /// Amend back to Spec.
    Spec,
    /// Amend back to Plan.
    Plan,
}

/// Status of a recorded plan amendment proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardAmendmentStatus {
    /// Proposed and pending re-approval.
    Proposed,
    /// Applied and re-approved.
    Applied,
    /// Superseded by a newer amendment.
    Superseded,
    /// Rejected.
    Rejected,
}

/// Recorded plan amendment entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPlanAmendmentRecord {
    /// Amendment identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Card revision when proposed.
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub card_revision: u64,
    /// Target stage (`spec` or `plan`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub target_stage: Option<WorkflowBoardAmendmentTargetStage>,
    /// Amendment status.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub status: Option<WorkflowBoardAmendmentStatus>,
    /// Operator/agent reason for amendment.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub reason: String,
    /// Short summary of proposed changes.
    pub summary: Option<String>,
    /// Source task ID that triggered the amendment, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub source_task_id: Option<String>,
    /// Unexpected files motivating the amendment.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub unexpected_files: Vec<String>,
    /// Creation timestamp.
    pub created_at: String,
}

/// File change classification matching `ReviewDiffChangeType` in TS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowBoardDiffChangeType {
    /// Modified file.
    Change,
    /// Pure rename.
    RenamePure,
    /// Rename with modifications.
    RenameChanged,
    /// Newly added file.
    New,
    /// Deleted file.
    Deleted,
}

/// File ownership classification in a card diff entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardDiffFileContractStatus {
    /// File is in `filesAllowed`.
    Allowed,
    /// File is outside `filesAllowed`.
    Unexpected,
    /// File is in `filesForbidden`.
    Forbidden,
    /// No task contract filter applied.
    Unassigned,
}

/// Single file entry returned by `workflowBoard.getCardDiff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardDiffFileEntry {
    /// Workspace-relative file path.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub path: String,
    /// Previous path when renamed.
    #[serde(deserialize_with = "deserialize_opt_artifact_key")]
    pub previous_path: Option<String>,
    /// Git change classification.
    pub change_type: WorkflowBoardDiffChangeType,
    /// Added line count.
    pub additions: u64,
    /// Deleted line count.
    pub deletions: u64,
    /// Unified diff patch, if requested and available.
    pub patch: Option<String>,
    /// Ownership status against active task contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_status: Option<WorkflowBoardDiffFileContractStatus>,
}
