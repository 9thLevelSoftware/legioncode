//! RPC request, response, and error payloads for `workflowBoard.*` methods.

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::ids::{ProjectId, RunId, ThreadId, WorkflowBoardStageRunId, WorkflowCardId};
use crate::orchestration::{
    ModelSelection, PatchValue, ProviderInteractionMode, WorkspaceStrategy,
};
use crate::policy::RuntimeMode;

use super::m2_models::WorkflowBoardIntakeGoal;
use super::validators::{
    deserialize_opt_trimmed_non_empty, deserialize_patch_artifact_key,
    deserialize_positive_revision, deserialize_present_artifact_key,
    deserialize_present_input_path, deserialize_present_input_paths, deserialize_present_some,
    deserialize_present_trimmed_non_empty, deserialize_present_trimmed_non_empty_vec,
    deserialize_trimmed_non_empty,
};
use super::{
    WorkflowBoardArtifactKind, WorkflowBoardAssistanceMode, WorkflowBoardSpecSourceType,
    WorkflowBoardStageRunStage,
};

pub use super::error::{WorkflowBoardRequestError, WorkflowBoardRequestErrorCode};
pub use super::m2_rpc::*;
pub use super::m3_rpc::*;
pub use super::m4_rpc::*;
pub use super::rpc_spec::*;

/// Input for `workflowBoard.subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardSubscribeInput {
    /// Target project identifier.
    pub project_id: ProjectId,
}

/// Input for `workflowBoard.getCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardGetCardInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Optional project scope check.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub project_id: Option<ProjectId>,
}

/// Input for `workflowBoard.getArtifact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardGetArtifactInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Safe relative artifact key (e.g. `spec.md` or `wireframes/index.html`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Optional artifact kind selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkflowBoardArtifactKind>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGetArtifactInput {
    card_id: WorkflowCardId,
    #[serde(default, deserialize_with = "deserialize_present_artifact_key")]
    key: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_some")]
    kind: Option<WorkflowBoardArtifactKind>,
}

impl<'de> Deserialize<'de> for WorkflowBoardGetArtifactInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawGetArtifactInput::deserialize(deserializer)?;
        if raw.key.is_none() && raw.kind.is_none() {
            return Err(de::Error::custom(
                "at least one of `key` or `kind` must be provided",
            ));
        }
        Ok(Self {
            card_id: raw.card_id,
            key: raw.key,
            kind: raw.kind,
        })
    }
}

/// Input for `workflowBoard.createCard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardCreateCardInput {
    /// Target project identifier.
    pub project_id: ProjectId,
    /// Human-readable card title.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub title: String,
    /// Optional initial brief text (cannot be whitespace-only if provided).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub brief: Option<String>,
    /// Optional workspace-relative brief file path.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_input_path"
    )]
    pub brief_path: Option<String>,
    /// Optional workspace-relative input file paths (non-credential).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_input_paths"
    )]
    pub input_paths: Option<Vec<String>>,
    /// Optional intake goal (`greenfield`, `change`, `audit`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub intake_goal: Option<WorkflowBoardIntakeGoal>,
    /// Optional flag indicating whether HTML wireframes should be generated.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub wireframes_enabled: Option<bool>,
    /// Optional explicit wireframe entry key (`null` for non-UI cards).
    #[serde(
        default,
        skip_serializing_if = "PatchValue::is_absent",
        deserialize_with = "deserialize_patch_artifact_key"
    )]
    pub wireframes_index: PatchValue<String>,
    /// Spec intake source type.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub source_type: Option<WorkflowBoardSpecSourceType>,
    /// Spec assistance mode.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub assistance_mode: Option<WorkflowBoardAssistanceMode>,
    /// Explicit card workspace strategy.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub workspace_strategy: Option<WorkspaceStrategy>,
    /// Selected model for card stage launches.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub model_selection: Option<ModelSelection>,
    /// Runtime permission mode for card stage launches.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub runtime_mode: Option<RuntimeMode>,
    /// Provider interaction mode for card stage launches.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub interaction_mode: Option<ProviderInteractionMode>,
}

/// Input for `workflowBoard.startStage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardStartStageInput {
    /// Target workflow card identifier.
    pub card_id: WorkflowCardId,
    /// Expected current card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub expected_revision: u64,
    /// Target stage to launch (`spec`, `plan`, `execute`, `review`, `ship`).
    pub stage: WorkflowBoardStageRunStage,
    /// Optional task identifier when starting an Execute task run.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub task_id: Option<String>,
    /// Optional caller-supplied idempotent launch key.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty"
    )]
    pub stable_launch_key: Option<String>,
    /// Explicit model selection override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub model_selection: Option<ModelSelection>,
    /// Explicit runtime mode override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub runtime_mode: Option<RuntimeMode>,
    /// Explicit interaction mode override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub interaction_mode: Option<ProviderInteractionMode>,
    /// Explicit workspace strategy override.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub workspace_strategy: Option<WorkspaceStrategy>,
    /// Acknowledge unsupported adopted `.legion-cli` policies when starting Execute.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_some"
    )]
    pub acknowledge_unsupported_policies: Option<bool>,
    /// Explicitly acknowledged unsupported policy keys.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_trimmed_non_empty_vec"
    )]
    pub acknowledged_policies: Option<Vec<String>>,
}

/// Output for `workflowBoard.startStage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardStartStageResult {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Resulting card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Stage started or resumed.
    pub stage: WorkflowBoardStageRunStage,
    /// Optional task identifier when launched for a specific task.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub task_id: Option<String>,
    /// Stage run record identifier.
    pub stage_run_id: WorkflowBoardStageRunId,
    /// Workflow run identifier if backed by a workflow run.
    pub run_id: Option<RunId>,
    /// Linked thread identifier.
    pub thread_id: ThreadId,
    /// `true` when an existing thread launch for the stable launch key was reused.
    pub resumed: bool,
}
