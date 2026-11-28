#![forbid(unsafe_code)]
//! M3 execution, check, diff, amendment, and ship methods on [`ProjectWorkflowBoardService`].

use legion_contracts::workflow_board::{
    WorkflowBoardApplyCliImportInput, WorkflowBoardApplyCliImportResult,
    WorkflowBoardApproveDeliveryInput, WorkflowBoardApproveDeliveryResult,
    WorkflowBoardGetCardDiffInput, WorkflowBoardGetCardDiffResult, WorkflowBoardPauseCardInput,
    WorkflowBoardPauseCardResult, WorkflowBoardPreviewCliImportInput,
    WorkflowBoardPreviewCliImportResult, WorkflowBoardProposeAmendmentInput,
    WorkflowBoardProposeAmendmentResult, WorkflowBoardRecordManualAcceptanceInput,
    WorkflowBoardRecordManualAcceptanceResult, WorkflowBoardRequestError,
    WorkflowBoardResolveFindingInput, WorkflowBoardResolveFindingResult,
    WorkflowBoardResumeCardInput, WorkflowBoardResumeCardResult, WorkflowBoardRetryStepInput,
    WorkflowBoardRetryStepResult, WorkflowCardDetail,
};

use crate::workflow_board::ProjectWorkflowBoardService;
use crate::workflow_board_amend_ops::propose_amendment_impl;
use crate::workflow_board_checks::run_planned_check_impl;
use crate::workflow_board_cli_import::{apply_cli_import_impl, preview_cli_import_impl};
use crate::workflow_board_diff::get_card_diff_impl;
use crate::workflow_board_exec_ops::{pause_card_impl, resume_card_impl, retry_step_impl};
use crate::workflow_board_fs::{resolve_card_workspace_root, validate_safe_card_id};
use crate::workflow_board_hydration_m3::load_hydrated_card_detail;
use crate::workflow_board_ship_ops::{
    approve_delivery_impl, record_manual_acceptance_impl, resolve_finding_impl,
};
use crate::workflow_board_stage_mcp::{
    CompleteTaskFromStageParams, RawReviewFindingTuple, RecordReviewFromStageParams,
    complete_task_from_stage_impl, record_review_from_stage_impl,
};
use crate::workflow_board_turn_observer::{process_settled_board_turns, spawn_board_turn_observer};
use crate::workflow_board_types::db_lock_err;

impl ProjectWorkflowBoardService {
    /// Spawns the event-driven turn-settlement observer for Execute and Review stage runs.
    #[must_use]
    pub fn spawn_turn_observer(&self) -> tokio::task::JoinHandle<()> {
        spawn_board_turn_observer(self.clone())
    }

    /// Runs a catch-up settlement sweep for any finished Execute/Review turns.
    pub async fn process_settled_turns(&self) {
        process_settled_board_turns(self).await;
    }

    /// Pauses a card and cancels any active stage workflow runs.
    pub async fn pause_card(
        &self,
        input: &WorkflowBoardPauseCardInput,
    ) -> Result<WorkflowBoardPauseCardResult, WorkflowBoardRequestError> {
        pause_card_impl(
            self.orchestrator_ref(),
            self.workflow_manager_ref(),
            input,
            |pid| self.notify_project_change(pid),
        )
        .await
    }

    /// Resumes a paused card, optionally reconciling interrupted actions/journals.
    pub async fn resume_card(
        &self,
        input: &WorkflowBoardResumeCardInput,
    ) -> Result<WorkflowBoardResumeCardResult, WorkflowBoardRequestError> {
        resume_card_impl(self.orchestrator_ref(), input, |pid| {
            self.notify_project_change(pid);
        })
        .await
    }

    /// Retries a failed/blocked task, check, review, or stage run.
    pub async fn retry_step(
        &self,
        input: &WorkflowBoardRetryStepInput,
    ) -> Result<WorkflowBoardRetryStepResult, WorkflowBoardRequestError> {
        retry_step_impl(self.orchestrator_ref(), input, |pid| {
            self.notify_project_change(pid);
        })
        .await
    }

    /// Records a bounded plan/spec amendment from Execute/Review discoveries.
    pub async fn propose_amendment(
        &self,
        input: &WorkflowBoardProposeAmendmentInput,
        proposed_by: &str,
    ) -> Result<WorkflowBoardProposeAmendmentResult, WorkflowBoardRequestError> {
        propose_amendment_impl(self.orchestrator_ref(), input, proposed_by, |pid| {
            self.notify_project_change(pid);
        })
        .await
    }

    /// Resolves a review finding or contract violation (human-client gated).
    pub async fn resolve_finding(
        &self,
        input: &WorkflowBoardResolveFindingInput,
        caller_kind: &str,
    ) -> Result<WorkflowBoardResolveFindingResult, WorkflowBoardRequestError> {
        resolve_finding_impl(self.orchestrator_ref(), input, caller_kind, |pid| {
            self.notify_project_change(pid);
        })
        .await
    }

    /// Records manual acceptance evidence for a criterion (human-client gated).
    pub async fn record_manual_acceptance(
        &self,
        input: &WorkflowBoardRecordManualAcceptanceInput,
        caller_kind: &str,
    ) -> Result<WorkflowBoardRecordManualAcceptanceResult, WorkflowBoardRequestError> {
        record_manual_acceptance_impl(self.orchestrator_ref(), input, caller_kind, |pid| {
            self.notify_project_change(pid);
        })
        .await
    }

    /// Evaluates ship gates and records final delivery approval (human-client gated; never auto-commits).
    pub async fn approve_delivery(
        &self,
        input: &WorkflowBoardApproveDeliveryInput,
        caller_kind: &str,
    ) -> Result<WorkflowBoardApproveDeliveryResult, WorkflowBoardRequestError> {
        approve_delivery_impl(self.orchestrator_ref(), input, caller_kind, |pid| {
            self.notify_project_change(pid);
        })
        .await
    }

    /// Computes live worktree diff and task contract status for a card.
    pub fn get_card_diff(
        &self,
        input: &WorkflowBoardGetCardDiffInput,
    ) -> Result<WorkflowBoardGetCardDiffResult, WorkflowBoardRequestError> {
        get_card_diff_impl(self.orchestrator_ref(), input)
    }

    /// Executes a planned `argv`-only check in the card worktree and records a check receipt.
    pub async fn run_planned_check(
        &self,
        card_id: &str,
        check_id: &str,
        expected_revision: Option<u64>,
    ) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
        let cid = validate_safe_card_id(card_id)?;
        let exec = run_planned_check_impl(
            self.orchestrator_ref(),
            cid,
            check_id,
            expected_revision,
            |pid| self.notify_project_change(pid),
        )
        .await?;
        let conn = self
            .orchestrator_ref()
            .sink()
            .db()
            .lock()
            .map_err(|_| db_lock_err())?;
        let ws_root = resolve_card_workspace_root(&conn, &exec.planning.card_detail.card)?;
        load_hydrated_card_detail(&conn, &ws_root, cid)
    }

    /// Completes a bound Execute task from scoped MCP with post-turn contract diff validation.
    pub async fn complete_task_from_stage(
        &self,
        card_id: &str,
        stage_run_id: &str,
        task_id: &str,
        outcome_status: &str,
        error_message: Option<&str>,
    ) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
        complete_task_from_stage_impl(
            self.orchestrator_ref(),
            self.workflow_manager_ref(),
            CompleteTaskFromStageParams {
                card_id_raw: card_id,
                stage_run_id,
                task_id,
                outcome_status,
                error_message,
            },
            |pid| self.notify_project_change(pid),
        )
        .await
    }

    /// Records an independent Review verdict/findings from scoped MCP (`disposition = "unresolved"`).
    pub async fn record_review_from_stage(
        &self,
        card_id: &str,
        stage_run_id: &str,
        verdict: &str,
        summary: Option<&str>,
        findings: &[RawReviewFindingTuple],
    ) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
        record_review_from_stage_impl(
            self.orchestrator_ref(),
            self.workflow_manager_ref(),
            RecordReviewFromStageParams {
                card_id_raw: card_id,
                stage_run_id,
                verdict,
                summary,
                findings,
            },
            |pid| self.notify_project_change(pid),
        )
        .await
    }

    /// Scans a `.legion-cli/` directory read-only and previews import provenance, locks, and policies.
    pub fn preview_cli_import(
        &self,
        input: &WorkflowBoardPreviewCliImportInput,
    ) -> Result<WorkflowBoardPreviewCliImportResult, WorkflowBoardRequestError> {
        preview_cli_import_impl(self.orchestrator_ref(), input)
    }

    /// Idempotently imports a `.legion-cli/` project into SQLite and `.legion/workflows/<card-id>/`.
    pub fn apply_cli_import(
        &self,
        input: &WorkflowBoardApplyCliImportInput,
    ) -> Result<WorkflowBoardApplyCliImportResult, WorkflowBoardRequestError> {
        apply_cli_import_impl(self.orchestrator_ref(), input, |pid| {
            self.notify_project_change(pid);
        })
    }
}
