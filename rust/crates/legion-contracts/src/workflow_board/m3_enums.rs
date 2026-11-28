//! M3 enums for Execute, Recovery, Planned Checks, Independent Review, and AC Observations.

use serde::{Deserialize, Serialize};

/// Lifecycle delivery status for Ship gate readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardDeliveryStatus {
    /// Delivery not yet evaluated or waiting on prerequisites.
    Pending,
    /// Blocked by one or more Ship blockers.
    Blocked,
    /// Zero blockers; ready for human delivery approval.
    Ready,
    /// Human-approved delivery receipt recorded.
    Shipped,
}

/// Validity of a recorded Ship delivery receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardDeliveryReceiptStatus {
    /// Human-approved delivery receipt recorded.
    Shipped,
    /// Receipt valid for the bound snapshot.
    Valid,
    /// The bound snapshot or required evidence has changed.
    Stale,
    /// A newer receipt superseded this one.
    Superseded,
    /// Receipt retained for history.
    Historical,
}

/// Post-turn file-contract diff validation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardFileContractStatus {
    /// All changed files are within `filesAllowed` and not `filesForbidden`.
    Clean,
    /// Unexpected or forbidden edits detected and preserved for operator resolution.
    ViolationPending,
    /// Violation resolved by operator disposition.
    Resolved,
    /// Plan amended to incorporate the expanded file scope.
    Amended,
    /// Report superseded or invalidated by subsequent edits.
    Stale,
}

/// Operator resolution disposition for a file-contract violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardFileContractResolution {
    /// Propose a plan amendment covering the unexpected files.
    AmendPlan,
    /// Accept the expanded file scope.
    AcceptScope,
    /// Operator manually reverted the unexpected edits.
    ManualReverted,
    /// Dismiss the violation with rationale.
    Dismiss,
}

/// Write-journal operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardWriteJournalOperation {
    /// File write operation.
    Write,
    /// File delete operation.
    Delete,
}

/// Write-journal entry status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardWriteJournalStatus {
    /// Journal entry created before staging.
    Pending,
    /// Temp file staged.
    Staged,
    /// Target file written on disk; DB receipt pending.
    FileWritten,
    /// Both file write and DB receipt committed.
    Committed,
    /// Rolled back during recovery.
    RolledBack,
    /// Failed before completion.
    Failed,
}

/// External file edit detection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardExternalEditStatus {
    /// External edit detected on disk.
    Detected,
    /// Reconciled with card approvals/evidence.
    Reconciled,
    /// Dismissed without action.
    Dismissed,
}

/// Planned check execution receipt status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardCheckStatus {
    /// Check has not yet run.
    Pending,
    /// Check is currently running.
    Running,
    /// Check passed against current snapshot.
    Passed,
    /// Check failed against current snapshot.
    Failed,
    /// Check invalidated by snapshot, spec, or plan change.
    Stale,
    /// Imported historical check receipt.
    Historical,
}

/// Severity of an independent review finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardReviewSeverity {
    /// Blocking critical defect.
    Critical,
    /// Significant defect requiring disposition.
    Major,
    /// Minor improvement or nit.
    Minor,
    /// Informational observation.
    Info,
}

/// Lifecycle status of an independent review run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardReviewStatus {
    /// Review queued.
    Pending,
    /// Review actively running in fresh thread.
    Running,
    /// Review completed.
    Completed,
    /// Review execution failed.
    Failed,
    /// Review invalidated by product snapshot or plan change.
    Stale,
    /// Imported historical review run.
    Historical,
}

/// Overall verdict of an independent review run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardReviewVerdict {
    /// Review not yet completed.
    Pending,
    /// Clean pass with zero blocking findings.
    Pass,
    /// Review produced structured findings.
    Findings,
    /// Review execution failed.
    Failed,
    /// Review invalidated.
    Stale,
}

/// Lifecycle status of an individual review finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardReviewFindingStatus {
    /// Open and awaiting human disposition.
    Open,
    /// Resolved by human disposition.
    Resolved,
    /// Invalidated by snapshot or plan change.
    Stale,
    /// Imported historical finding.
    Historical,
}

/// Disposition recorded for a review finding (including `unresolved`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardFindingDisposition {
    /// Not yet triaged by a human operator.
    Unresolved,
    /// Request a code/doc fix before shipping.
    Fix,
    /// Accept the documented risk with reason.
    AcceptRisk,
    /// Dismiss the finding as inapplicable/false positive with reason.
    Dismiss,
    /// Defer to follow-up work.
    Defer,
}

/// Human disposition input values accepted by `workflowBoard.resolveFinding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardFindingResolveDisposition {
    /// Request a fix.
    Fix,
    /// Accept risk with reason.
    AcceptRisk,
    /// Dismiss with reason.
    Dismiss,
    /// Defer.
    Defer,
}

/// Persisted AC evidence status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBoardAcEvidenceStatus {
    /// Awaiting verification.
    Pending,
    /// Verified passing against current snapshot.
    Pass,
    /// Verified failing against current snapshot.
    Fail,
    /// Explicitly marked not applicable with rationale.
    NotApplicable,
    /// Invalidated by snapshot, spec, or plan change.
    Stale,
    /// Imported historical evidence.
    Historical,
}

/// Manual acceptance verdict accepted by `workflowBoard.recordManualAcceptance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkflowBoardManualAcceptanceVerdict {
    /// Criterion verified passing.
    #[serde(rename = "pass")]
    Pass,
    /// Criterion failed manual verification.
    #[serde(rename = "fail")]
    Fail,
    /// Criterion not applicable (requires note; maps to SQLite `not_applicable`).
    #[serde(rename = "not-applicable")]
    NotApplicable,
}
