//! M4 RPC request and response payloads for `workflowBoard.previewCliImport` and `workflowBoard.applyCliImport`.

use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, WorkflowCardId};
use crate::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use crate::policy::RuntimeMode;

use super::m2_models::WorkflowBoardTaskContract;
use super::m4_models::{
    WorkflowBoardCliImportHistoricalReceipt, WorkflowBoardCliImportLockEntry,
    WorkflowBoardCliImportMissingLogNote, WorkflowBoardCliImportTaskPreview,
    WorkflowBoardCliImportUnsupportedPolicy,
};
use super::validators::{
    deserialize_artifact_key, deserialize_positive_revision, deserialize_present_input_path,
    deserialize_present_some, deserialize_present_trimmed_non_empty_vec,
    deserialize_trimmed_non_empty, deserialize_trimmed_non_empty_vec,
};
use super::{WorkflowBoardCardStatus, WorkflowBoardStage, WorkflowCardDetail};

/// Input for `workflowBoard.previewCliImport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPreviewCliImportInput {
    /// Target project identifier.
    pub project_id: ProjectId,
    /// Optional workspace-relative CLI directory (defaults to `.legion-cli`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_input_path"
    )]
    pub cli_dir: Option<String>,
}

/// Output for `workflowBoard.previewCliImport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardPreviewCliImportResult {
    /// Target project identifier.
    pub project_id: ProjectId,
    /// Relative `.legion-cli` source directory scanned.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub source_path: String,
    /// Preserved CLI `specId`.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub spec_id: String,
    /// Imported card title.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub title: String,
    /// Initial board stage derived from CLI artifacts.
    pub stage: WorkflowBoardStage,
    /// Initial board status derived from CLI tasks/execution state.
    pub status: WorkflowBoardCardStatus,
    /// Existing card ID if this `(projectId, specId)` was already imported.
    pub existing_card_id: Option<WorkflowCardId>,
    /// `true` when this spec was previously imported for the project.
    pub already_imported: bool,
    /// Active locks refusing takeover (`canApply == false` when non-empty).
    pub active_locks: Vec<WorkflowBoardCliImportLockEntry>,
    /// Stale dead-PID locks observed during scan.
    pub stale_locks: Vec<WorkflowBoardCliImportLockEntry>,
    /// `true` when `applyCliImport` can proceed without active lock conflicts.
    pub can_apply: bool,
    /// Explicit refusal reason when `canApply == false`.
    pub refusal_reason: Option<String>,
    /// Scanned CLI tasks (`TSK-*`).
    pub tasks: Vec<WorkflowBoardCliImportTaskPreview>,
    /// Preserved CLI decision IDs (`D-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub decision_ids: Vec<String>,
    /// Preserved CLI assumption IDs (`ASM-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub assumption_ids: Vec<String>,
    /// Preserved acceptance criterion IDs (`AC-*`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty_vec")]
    pub acceptance_criterion_ids: Vec<String>,
    /// Historical receipts detected from `.legion-cli`.
    pub historical_receipts: Vec<WorkflowBoardCliImportHistoricalReceipt>,
    /// Unsupported adopted policies detected from `config.yaml` / `workflow/assurance*.yaml`.
    pub unsupported_policies: Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    /// Notes for missing gitignored cache log files.
    pub missing_log_notes: Vec<WorkflowBoardCliImportMissingLogNote>,
    /// Informational scan warnings.
    pub warnings: Vec<String>,
}

/// Input for `workflowBoard.applyCliImport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApplyCliImportInput {
    /// Target project identifier.
    pub project_id: ProjectId,
    /// Optional workspace-relative CLI directory (defaults to `.legion-cli`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_input_path"
    )]
    pub cli_dir: Option<String>,
    /// Optional upfront acknowledgement of unsupported adopted policies.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub acknowledge_unsupported_policies: Option<bool>,
    /// Optional list of explicitly acknowledged policy keys.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty_vec"
    )]
    pub acknowledged_policies: Option<Vec<String>>,
    /// Optional default model selection for subsequent native stage launches.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub model_selection: Option<ModelSelection>,
    /// Optional default runtime mode.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub runtime_mode: Option<RuntimeMode>,
    /// Optional default interaction mode.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub interaction_mode: Option<ProviderInteractionMode>,
    /// Optional workspace strategy override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub workspace_strategy: Option<WorkspaceStrategy>,
}

/// Output for `workflowBoard.applyCliImport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardApplyCliImportResult {
    /// Persisted workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Target project identifier.
    pub project_id: ProjectId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// `true` on initial creation; `false` on idempotent re-import.
    pub created: bool,
    /// Preserved CLI `specId`.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub source_spec_id: String,
    /// Imported task contracts.
    pub tasks: Vec<WorkflowBoardTaskContract>,
    /// Imported historical receipts.
    pub historical_receipts: Vec<WorkflowBoardCliImportHistoricalReceipt>,
    /// Unsupported adopted policies detected on the card.
    pub unsupported_policies: Vec<WorkflowBoardCliImportUnsupportedPolicy>,
    /// Notes for missing gitignored cache log files.
    pub missing_log_notes: Vec<WorkflowBoardCliImportMissingLogNote>,
    /// Full hydrated card detail.
    pub card: WorkflowCardDetail,
}
