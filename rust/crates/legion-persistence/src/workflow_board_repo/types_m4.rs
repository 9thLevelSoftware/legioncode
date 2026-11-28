//! Milestone 4 `.legion-cli` import persisted row and upsert DTOs for [`crate::workflow_board_repo`].

use super::types::CardDetail;

/// Row representation from `workflow_board_cli_imports`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliImportRow {
    pub card_id: String,
    pub project_id: String,
    pub source_cli_spec_id: String,
    pub source_cli_dir: String,
    pub source_digest: String,
    pub last_imported_revision: i64,
    pub ownership_owner: String,
    pub unsupported_policies_json: String,
    pub acknowledged_policies_json: String,
    pub policies_acknowledged_at: Option<String>,
    pub assurance_policy_json: Option<String>,
    pub import_provenance_json: String,
    pub imported_at: String,
    pub last_synced_at: String,
}

/// Decision seed extracted from `.legion-cli/decisions/*.md`.
#[derive(Debug, Clone)]
pub struct CliImportDecisionSeed<'a> {
    pub decision_id: &'a str,
    pub prompt: &'a str,
    pub status: &'a str,
    pub answer: Option<&'a str>,
    pub rationale: Option<&'a str>,
}

/// Task seed extracted from `.legion-cli/tasks/TSK-*.md`.
#[derive(Debug, Clone)]
pub struct CliImportTaskSeed<'a> {
    pub task_id: &'a str,
    pub ordinal: i64,
    pub title: &'a str,
    pub status: &'a str,
    pub priority: &'a str,
    pub parent_id: Option<&'a str>,
    pub blocked_by_json: &'a str,
    pub blocks_json: &'a str,
    pub files_allowed_json: &'a str,
    pub files_forbidden_json: &'a str,
    pub expected_artifacts_json: &'a str,
    pub verification_commands_json: &'a str,
    pub testing_method: &'a str,
    pub acceptance_ids_json: &'a str,
    pub blocker_reason: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub max_files_touched: Option<i64>,
    pub contract_digest: Option<&'a str>,
    pub last_error: Option<&'a str>,
}

/// Planned check seed derived from imported task/plan verification commands.
#[derive(Debug, Clone)]
pub struct CliImportPlannedCheckSeed<'a> {
    pub check_id: &'a str,
    pub owner_task_id: Option<&'a str>,
    pub argv_json: &'a str,
    pub label: Option<&'a str>,
    pub last_exit_code: Option<i64>,
    pub last_log_ref: Option<&'a str>,
    pub stale_reason: Option<&'a str>,
    pub last_receipt_id: Option<&'a str>,
    pub last_run_at: Option<&'a str>,
    pub last_output_excerpt: Option<&'a str>,
}

/// Historical check receipt seed derived from `.legion-cli/workflow/execution.yaml`.
#[derive(Debug, Clone)]
pub struct CliImportCheckReceiptSeed<'a> {
    pub id: &'a str,
    pub check_id: &'a str,
    pub criterion_id: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub argv_json: &'a str,
    pub exit_code: Option<i64>,
    pub log_ref: Option<&'a str>,
    pub stderr_excerpt: Option<&'a str>,
    pub stale_reason: Option<&'a str>,
    pub executed_at: Option<&'a str>,
}

/// Historical AC evidence seed derived from `.legion-cli/specs/*/SPEC.md` and `plan-approval.yaml`.
#[derive(Debug, Clone)]
pub struct CliImportAcEvidenceSeed<'a> {
    pub criterion_id: &'a str,
    pub method: &'a str,
    pub check_ids_json: &'a str,
    pub command_argv_json: Option<&'a str>,
    pub procedure: Option<&'a str>,
    pub observation_note: Option<&'a str>,
    pub external_source: Option<&'a str>,
}

/// Historical approval seed derived from `spec-approval.yaml` / `plan-approval.yaml`.
#[derive(Debug, Clone)]
pub struct CliImportApprovalSeed<'a> {
    pub id: &'a str,
    pub gate: &'a str,
    pub artifact_digest: &'a str,
    pub spec_digest: Option<&'a str>,
    pub plan_digest: Option<&'a str>,
    pub tasks_digest: Option<&'a str>,
    pub approved_by: &'a str,
    pub approved_at: &'a str,
    pub metadata_json: &'a str,
}

/// Full transactional input for [`super::cli_import::apply_cli_import_tx`].
#[derive(Debug, Clone)]
pub struct ApplyCliImportInput<'a> {
    pub candidate_card_id: &'a str,
    pub revision_id: &'a str,
    pub project_id: &'a str,
    pub source_cli_spec_id: &'a str,
    pub source_cli_dir: &'a str,
    pub source_digest: &'a str,
    pub title: &'a str,
    pub stage: &'a str,
    pub status: &'a str,
    pub brief: &'a str,
    pub workspace_strategy_json: &'a str,
    pub worktree_path: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub model_selection_json: Option<&'a str>,
    pub runtime_mode: Option<&'a str>,
    pub interaction_mode: Option<&'a str>,
    pub spec_digest: &'a str,
    pub design_digest: Option<&'a str>,
    pub plan_digest: Option<&'a str>,
    pub tasks_digest: Option<&'a str>,
    pub current_task_id: Option<&'a str>,
    pub blocker_reason: Option<&'a str>,
    pub next_action: Option<&'a str>,
    pub spec_json: &'a str,
    pub plan_json: Option<&'a str>,
    pub unsupported_policies_json: &'a str,
    pub acknowledged_policies_json: &'a str,
    pub policies_acknowledged_at: Option<&'a str>,
    pub assurance_policy_json: Option<&'a str>,
    pub import_provenance_json: &'a str,
    pub decisions: &'a [CliImportDecisionSeed<'a>],
    pub tasks: &'a [CliImportTaskSeed<'a>],
    pub planned_checks: &'a [CliImportPlannedCheckSeed<'a>],
    pub check_receipts: &'a [CliImportCheckReceiptSeed<'a>],
    pub ac_evidence: &'a [CliImportAcEvidenceSeed<'a>],
    pub historical_approvals: &'a [CliImportApprovalSeed<'a>],
    pub historical_review_summary: Option<&'a str>,
    pub historical_review_at: Option<&'a str>,
}

/// Outcome returned by [`super::cli_import::apply_cli_import_tx`].
#[expect(
    clippy::struct_excessive_bools,
    reason = "Distinct CLI import outcome flags: created, reused, ownership_preserved, policy_ack_only"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyCliImportOutcome {
    pub card_id: String,
    pub revision: i64,
    pub created: bool,
    pub reused: bool,
    pub ownership_preserved: bool,
    pub policy_ack_only: bool,
    pub cli_import: CliImportRow,
    pub card_detail: CardDetail,
}
