//! Milestone 3 persisted row, blocker, and CAS input DTOs for [`crate::workflow_board_repo`].

use super::types_m2::CardPlanningDetail;
use super::types_m4::CliImportRow;

/// Row representation from `workflow_board_contract_violations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractViolationRow {
    pub id: String,
    pub card_id: String,
    pub task_id: String,
    pub stage_run_id: Option<String>,
    pub thread_id: Option<String>,
    pub workflow_run_id: Option<String>,
    pub card_revision: i64,
    pub status: String,
    pub changed_files_json: String,
    pub allowed_files_json: String,
    pub unexpected_files_json: String,
    pub forbidden_files_json: String,
    pub preserved_in_worktree: bool,
    pub blocking_reason: Option<String>,
    pub diff_digest: Option<String>,
    pub resolution_disposition: Option<String>,
    pub resolution_note: Option<String>,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_check_receipts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReceiptRow {
    pub id: String,
    pub card_id: String,
    pub check_id: String,
    pub criterion_id: Option<String>,
    pub task_id: Option<String>,
    pub card_revision: i64,
    pub argv_json: String,
    pub snapshot_digest: Option<String>,
    pub commit_sha: Option<String>,
    pub spec_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub log_ref: Option<String>,
    pub stdout_excerpt: Option<String>,
    pub stderr_excerpt: Option<String>,
    pub duration_ms: Option<i64>,
    pub stale_reason: Option<String>,
    pub executed_at: Option<String>,
    pub invalidated_at: Option<String>,
    pub updated_at: String,
}

/// Row representation from `workflow_board_review_runs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRunRow {
    pub id: String,
    pub card_id: String,
    pub stage_run_id: Option<String>,
    pub thread_id: Option<String>,
    pub card_revision: i64,
    pub snapshot_digest: Option<String>,
    pub status: String,
    pub verdict: String,
    pub summary: Option<String>,
    pub stale_reason: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub updated_at: String,
}

/// Row representation from `workflow_board_review_findings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFindingRow {
    pub id: String,
    pub card_id: String,
    pub review_run_id: String,
    pub severity: String,
    pub criterion_id: Option<String>,
    pub file_path: Option<String>,
    pub line: Option<i64>,
    pub message: String,
    pub status: String,
    pub disposition: String,
    pub resolution_note: Option<String>,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_amendments`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmendmentRow {
    pub id: String,
    pub card_id: String,
    pub card_revision: i64,
    pub target_stage: String,
    pub status: String,
    pub reason: String,
    pub summary: Option<String>,
    pub source_task_id: Option<String>,
    pub unexpected_files_json: String,
    pub proposed_changes_json: String,
    pub proposed_by: String,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_external_actions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalActionRow {
    pub id: String,
    pub card_id: String,
    pub action_kind: String,
    pub target_id: Option<String>,
    pub stage_run_id: Option<String>,
    pub idempotency_key: String,
    pub status: String,
    pub expected_snapshot_digest: Option<String>,
    pub observed_snapshot_digest: Option<String>,
    pub details_json: String,
    pub reconciliation_note: Option<String>,
    pub reconciled_by: Option<String>,
    pub reconciled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Row representation from `workflow_board_delivery_receipts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReceiptRow {
    pub id: String,
    pub card_id: String,
    pub approval_id: Option<String>,
    pub status: String,
    pub card_revision: i64,
    pub spec_digest: Option<String>,
    pub plan_digest: Option<String>,
    pub tasks_digest: Option<String>,
    pub checks_digest: Option<String>,
    pub product_snapshot_digest: String,
    pub commit_sha: Option<String>,
    pub diff_digest: Option<String>,
    pub review_id: Option<String>,
    pub ac_verdicts_summary_json: String,
    pub git_actions_json: String,
    pub delivery_note: Option<String>,
    pub approved_by: String,
    pub approved_at: String,
    pub invalidated_at: Option<String>,
    pub invalidation_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Deterministic Ship gate blocker with UI fix pointer (`stage`, `tab`, `target_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipBlocker {
    pub id: String,
    pub kind: String,
    pub message: String,
    pub stage: String,
    pub tab: String,
    pub target_id: Option<String>,
}

/// Full hydrated M1 + M2 + M3 execution & Ship aggregate for a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardExecutionDetail {
    pub planning: CardPlanningDetail,
    pub contract_violations: Vec<ContractViolationRow>,
    pub check_receipts: Vec<CheckReceiptRow>,
    pub review_runs: Vec<ReviewRunRow>,
    pub review_findings: Vec<ReviewFindingRow>,
    pub amendments: Vec<AmendmentRow>,
    pub external_actions: Vec<ExternalActionRow>,
    pub delivery_receipts: Vec<DeliveryReceiptRow>,
    pub ship_blockers: Vec<ShipBlocker>,
    pub cli_import: Option<CliImportRow>,
}

/// Input parameters for [`super::claim_or_start_task`].
#[derive(Debug, Clone)]
pub struct ClaimOrStartTaskInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub task_id: &'a str,
    pub stage_run_id: Option<&'a str>,
}

/// Input parameters for [`super::complete_task`].
#[derive(Debug, Clone)]
pub struct CompleteTaskInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub task_id: &'a str,
    pub outcome_status: &'a str,
    pub stage_run_id: Option<&'a str>,
    pub thread_id: Option<&'a str>,
    pub workflow_run_id: Option<&'a str>,
    pub diff_digest: Option<&'a str>,
    pub product_snapshot_digest: Option<&'a str>,
    pub error_message: Option<&'a str>,
    pub violation_id: Option<&'a str>,
    pub changed_files: &'a [&'a str],
    pub unexpected_files: &'a [&'a str],
    pub forbidden_files: &'a [&'a str],
}

/// Input parameters for [`super::resolve_contract_violation`].
#[derive(Debug, Clone)]
pub struct ResolveContractViolationInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub violation_id: &'a str,
    pub disposition: &'a str,
    pub note: Option<&'a str>,
    pub resolved_by: &'a str,
}

/// Input parameters for [`super::record_check_receipt`].
#[derive(Debug, Clone)]
pub struct RecordCheckReceiptInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub receipt_id: &'a str,
    pub expected_revision: i64,
    pub check_id: &'a str,
    pub argv: &'a [String],
    pub status: &'a str,
    pub exit_code: Option<i64>,
    pub log_ref: Option<&'a str>,
    pub stdout_excerpt: Option<&'a str>,
    pub stderr_excerpt: Option<&'a str>,
    pub duration_ms: Option<i64>,
    pub snapshot_digest: &'a str,
    pub commit_sha: Option<&'a str>,
}

/// Finding entry for [`RecordReviewRunInput`].
#[derive(Debug, Clone)]
pub struct ReviewFindingInput<'a> {
    pub id: &'a str,
    pub severity: &'a str,
    pub criterion_id: Option<&'a str>,
    pub file_path: Option<&'a str>,
    pub line: Option<i64>,
    pub message: &'a str,
}

/// Input parameters for [`super::record_review_run`].
#[derive(Debug, Clone)]
pub struct RecordReviewRunInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub review_id: &'a str,
    pub expected_revision: i64,
    pub stage_run_id: Option<&'a str>,
    pub thread_id: Option<&'a str>,
    pub snapshot_digest: Option<&'a str>,
    pub verdict: &'a str,
    pub summary: Option<&'a str>,
    pub findings: &'a [ReviewFindingInput<'a>],
}

/// Input parameters for [`super::resolve_finding`].
#[derive(Debug, Clone)]
pub struct ResolveFindingInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub review_id: Option<&'a str>,
    pub finding_id: &'a str,
    pub disposition: &'a str,
    pub reason: Option<&'a str>,
    pub resolved_by: &'a str,
}

/// Input parameters for [`super::pause_card`].
#[derive(Debug, Clone)]
pub struct PauseCardInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub reason: Option<&'a str>,
}

/// Input parameters for [`super::resume_card`].
#[derive(Debug, Clone)]
pub struct ResumeCardInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub note: Option<&'a str>,
}

/// Input parameters for [`super::retry_step`].
#[derive(Debug, Clone)]
pub struct RetryStepInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub step_kind: &'a str,
    pub target_id: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// Input parameters for [`super::propose_amendment`].
#[derive(Debug, Clone)]
pub struct ProposeAmendmentInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub amendment_id: &'a str,
    pub expected_revision: i64,
    pub target_stage: &'a str,
    pub reason: &'a str,
    pub summary: Option<&'a str>,
    pub source_task_id: Option<&'a str>,
    pub unexpected_files: &'a [&'a str],
    pub proposed_changes_json: &'a str,
    pub proposed_by: &'a str,
}

/// Input parameters for [`super::record_external_action`].
#[derive(Debug, Clone)]
pub struct RecordExternalActionInput<'a> {
    pub id: &'a str,
    pub card_id: &'a str,
    pub action_kind: &'a str,
    pub target_id: Option<&'a str>,
    pub stage_run_id: Option<&'a str>,
    pub idempotency_key: &'a str,
    pub status: &'a str,
    pub expected_snapshot_digest: Option<&'a str>,
    pub observed_snapshot_digest: Option<&'a str>,
    pub details_json: &'a str,
}

/// Input parameters for [`super::reconcile_interrupted_action`].
#[derive(Debug, Clone)]
pub struct ReconcileInterruptedActionInput<'a> {
    pub card_id: &'a str,
    pub action_id: &'a str,
    pub status: &'a str,
    pub observed_snapshot_digest: Option<&'a str>,
    pub note: Option<&'a str>,
    pub reconciled_by: &'a str,
}

/// Input parameters for [`super::record_manual_acceptance`].
#[derive(Debug, Clone)]
pub struct RecordManualAcceptanceInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub expected_revision: i64,
    pub criterion_id: &'a str,
    pub verdict: &'a str,
    pub note: Option<&'a str>,
    pub evidence_ref: Option<&'a str>,
    pub evidence_digest: Option<&'a str>,
    pub expected_product_snapshot_digest: Option<&'a str>,
    pub observed_by: &'a str,
}

/// Input parameters for [`super::approve_delivery`].
#[derive(Debug, Clone)]
pub struct ApproveDeliveryInput<'a> {
    pub card_id: &'a str,
    pub revision_id: &'a str,
    pub approval_id: &'a str,
    pub delivery_receipt_id: &'a str,
    pub expected_revision: i64,
    pub expected_product_snapshot_digest: Option<&'a str>,
    pub commit_sha: Option<&'a str>,
    pub diff_digest: Option<&'a str>,
    pub note: Option<&'a str>,
    pub approved_by: &'a str,
}
