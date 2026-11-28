//! M3 Execute, Recovery, Planned Check, Independent Review, and AC Observation models.

use serde::{Deserialize, Serialize};

use crate::ids::{RunId, ThreadId};

use super::m2_models::WorkflowBoardEvidenceMethod;
use super::m2_task_models::WorkflowBoardTaskStatus;
pub use super::m3_enums::*;
use super::validators::{
    deserialize_artifact_key, deserialize_artifact_key_vec, deserialize_non_empty_trimmed_vec,
    deserialize_opt_artifact_key, deserialize_opt_positive_revision,
    deserialize_opt_trimmed_non_empty, deserialize_positive_revision, deserialize_present_some,
    deserialize_present_trimmed_non_empty, deserialize_trimmed_non_empty,
    deserialize_trimmed_non_empty_vec,
};
use super::{WorkflowBoardApprovalGate, WorkflowBoardArtifactKind};

/// Per-task execution state hydrated onto `WorkflowCardDetail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardTaskExecutionState {
    /// Task identifier (`TSK-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub task_id: String,
    /// Current task lifecycle status.
    pub status: WorkflowBoardTaskStatus,
    /// Linked thread ID, if launched.
    pub thread_id: Option<ThreadId>,
    /// Linked workflow run ID, if any.
    pub run_id: Option<RunId>,
    /// Linked stage run ID, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub stage_run_id: Option<String>,
    /// Card worktree path, if known.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub worktree_path: Option<String>,
    /// Card branch name, if known.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub branch: Option<String>,
    /// Attempt counter (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub attempt: u64,
    /// Start timestamp, if started.
    pub started_at: Option<String>,
    /// End timestamp, if ended.
    pub ended_at: Option<String>,
    /// Last error message, if failed.
    pub last_error: Option<String>,
}

/// Post-turn file-contract diff validation report for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardTaskFileContractReport {
    /// Violation/report record identifier, if persisted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub id: Option<String>,
    /// Task identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub task_id: String,
    /// Thread identifier for the mutating turn.
    pub thread_id: Option<ThreadId>,
    /// Workflow run identifier, if any.
    pub run_id: Option<RunId>,
    /// Contract validation status.
    pub status: WorkflowBoardFileContractStatus,
    /// Changed files observed in the post-turn diff.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub changed_files: Vec<String>,
    /// Files allowed by the task contract.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub allowed_files: Vec<String>,
    /// Files changed outside `filesAllowed`.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub unexpected_files: Vec<String>,
    /// Files changed inside `filesForbidden`.
    #[serde(deserialize_with = "deserialize_artifact_key_vec")]
    pub forbidden_files: Vec<String>,
    /// `true` when unexpected edits were preserved intact in the worktree for review.
    pub preserved_in_worktree: bool,
    /// Blocking explanation when `violation_pending`.
    pub blocking_reason: Option<String>,
    /// Operator resolution disposition, if resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_disposition: Option<WorkflowBoardFileContractResolution>,
    /// Operator resolution note, if provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_note: Option<String>,
    /// Principal who resolved the violation.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub resolved_by: Option<String>,
    /// Resolution timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Write-journal entry surfaced for crash recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRecoveryEntry {
    /// Journal entry ID.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Target relative artifact path.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub target_path: String,
    /// Operation (`write` or `delete`).
    pub operation: WorkflowBoardWriteJournalOperation,
    /// Current journal status.
    pub status: WorkflowBoardWriteJournalStatus,
    /// Expected post-write SHA-256 digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub expected_digest: Option<String>,
    /// Previous SHA-256 digest.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub previous_digest: Option<String>,
    /// Card revision associated with this write.
    #[serde(deserialize_with = "deserialize_opt_positive_revision")]
    pub revision: Option<u64>,
    /// Last error, if any.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Update timestamp.
    pub updated_at: String,
    /// Completion timestamp.
    pub completed_at: Option<String>,
}

/// External file edit reconciliation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardExternalEditRecord {
    /// Record ID.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Relative artifact path edited externally.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub relative_path: String,
    /// Artifact kind, if recognized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<WorkflowBoardArtifactKind>,
    /// Expected digest prior to external edit.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub expected_digest: Option<String>,
    /// Observed digest on disk.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub actual_digest: Option<String>,
    /// Reconciliation status.
    pub status: WorkflowBoardExternalEditStatus,
    /// Gates affected by this external edit.
    pub affected_gates: Vec<WorkflowBoardApprovalGate>,
    /// Detection timestamp.
    pub detected_at: String,
    /// Reconciliation timestamp.
    pub reconciled_at: Option<String>,
    /// Human-readable summary.
    pub summary: String,
}

/// Aggregated write-journal and external-edit recovery state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRecoveryState {
    /// Count of incomplete write-journal entries.
    pub incomplete_journal_count: u64,
    /// Incomplete write-journal entries.
    pub pending_journal_entries: Vec<WorkflowBoardRecoveryEntry>,
    /// External file edit records.
    pub external_edits: Vec<WorkflowBoardExternalEditRecord>,
    /// `true` when external edits or interrupted actions require reconciliation before running.
    pub requires_reconciliation: bool,
    /// Timestamp of last recovery check, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recovered_at: Option<String>,
}

/// Snapshot-bound planned check execution receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPlannedCheckReceipt {
    /// Receipt row ID, if persisted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub id: Option<String>,
    /// Planned check ID.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub check_id: String,
    /// Linked acceptance criterion ID, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub criterion_id: Option<String>,
    /// Owner task ID, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub owner_task_id: Option<String>,
    /// Argv-only command tokens.
    #[serde(deserialize_with = "deserialize_non_empty_trimmed_vec")]
    pub argv: Vec<String>,
    /// Optional human description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Check receipt status.
    pub status: WorkflowBoardCheckStatus,
    /// Process exit code, if executed.
    pub exit_code: Option<i64>,
    /// Excerpt of standard output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_excerpt: Option<String>,
    /// Excerpt of standard error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_excerpt: Option<String>,
    /// Log artifact/file reference, if saved.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub log_ref: Option<String>,
    /// Product snapshot digest tested by this run.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub product_snapshot_digest: Option<String>,
    /// HEAD commit SHA tested, if available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub commit_sha: Option<String>,
    /// Bound spec digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub spec_digest: Option<String>,
    /// Bound plan digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub plan_digest: Option<String>,
    /// Reason if marked stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
    /// Execution timestamp.
    pub executed_at: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Structured finding produced during an independent fresh-thread review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardReviewFinding {
    /// Finding identifier.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Finding severity.
    pub severity: WorkflowBoardReviewSeverity,
    /// Lifecycle status (`open | resolved | stale | historical`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub status: Option<WorkflowBoardReviewFindingStatus>,
    /// Linked acceptance criterion ID, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub criterion_id: Option<String>,
    /// Affected file path, if any.
    #[serde(deserialize_with = "deserialize_opt_artifact_key")]
    pub file: Option<String>,
    /// Affected 1-based line number, if any.
    #[serde(deserialize_with = "deserialize_opt_positive_revision")]
    pub line: Option<u64>,
    /// Finding description.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub message: String,
    /// Human disposition.
    pub disposition: WorkflowBoardFindingDisposition,
    /// Resolution note/rationale.
    pub resolution_note: Option<String>,
    /// Human principal who resolved the finding.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub resolved_by: Option<String>,
    /// Resolution timestamp.
    pub resolved_at: Option<String>,
}

/// Independent fresh-thread review report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardReviewReport {
    /// Review run ID.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub review_id: String,
    /// Linked stage run ID, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub stage_run_id: Option<String>,
    /// Fresh thread ID used for this review.
    pub thread_id: Option<ThreadId>,
    /// Review run status.
    pub status: WorkflowBoardReviewStatus,
    /// Review verdict.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub verdict: Option<WorkflowBoardReviewVerdict>,
    /// Summary of review outcome.
    pub summary: Option<String>,
    /// Product snapshot digest reviewed.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub product_snapshot_digest: Option<String>,
    /// Reason if marked stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
    /// Structured findings.
    pub findings: Vec<WorkflowBoardReviewFinding>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Completion timestamp.
    pub completed_at: Option<String>,
}

/// Hydrated acceptance-criterion observation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardAcObservationRecord {
    /// Acceptance criterion ID (`AC-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub criterion_id: String,
    /// Evidence method (`automated | manual | external`).
    pub method: WorkflowBoardEvidenceMethod,
    /// Current AC evidence status.
    pub status: WorkflowBoardAcEvidenceStatus,
    /// Linked planned check IDs.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub check_ids: Vec<String>,
    /// Verification procedure description.
    pub procedure: Option<String>,
    /// Operator observation note.
    pub observation_note: Option<String>,
    /// External/log evidence reference.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub evidence_ref: Option<String>,
    /// Digest of attached evidence artifact.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub evidence_digest: Option<String>,
    /// Product snapshot digest bound to this observation.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub product_snapshot_digest: Option<String>,
    /// External evidence source identifier.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub external_source: Option<String>,
    /// Reason if marked stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
    /// Principal who observed the criterion.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub observed_by: Option<String>,
    /// Observation timestamp.
    pub observed_at: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}
