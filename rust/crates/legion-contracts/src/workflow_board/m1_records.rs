//! M1 enums and artifact/stage-run/revision record structs for `workflow_board`.

use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, RunId, ThreadId, WorkflowBoardStageRunId, WorkflowCardId};
use crate::orchestration::{ModelSelection, ProviderInteractionMode, WorkspaceStrategy};
use crate::policy::RuntimeMode;

use super::validators::{
    deserialize_artifact_key, deserialize_opt_trimmed_non_empty, deserialize_positive_revision,
    deserialize_present_positive_revision, deserialize_trimmed_non_empty,
};

/// Workflow stage column on the project workflow board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardStage {
    /// Spec authoring, exploration, challenge, and approval.
    Spec,
    /// Design lenses, alternatives, strategy, task DAG, and plan approval.
    Plan,
    /// Task execution, checks, and independent review.
    Execute,
    /// Delivery inspection, criterion observations, and Git actions.
    Ship,
}

/// Stage identifier for stage runs and `workflowBoard.startStage` (includes `review`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardStageRunStage {
    /// Spec stage.
    Spec,
    /// Plan stage.
    Plan,
    /// Execute stage.
    Execute,
    /// Fresh-thread independent review stage.
    Review,
    /// Ship stage.
    Ship,
}

/// Lifecycle status of a workflow board card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardCardStatus {
    /// Card is actively progressing or ready for work.
    Active,
    /// Card is blocked on a failed check, unresolved issue, or missing input.
    Blocked,
    /// Card work is paused by the user.
    Paused,
    /// Card completed delivery.
    Completed,
}

/// Origin source type for a card's initial specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardSpecSourceType {
    /// Started from a freeform idea or prompt.
    Idea,
    /// Started from an existing brief file.
    File,
    /// Started from codebase investigation.
    Codebase,
}

/// Assistance intensity during planning and specification authoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardAssistanceMode {
    /// Interactive guided exploration and challenge.
    Guided,
    /// Balanced assistance with targeted prompts.
    Balanced,
    /// Minimal-friction direct authoring.
    Direct,
}

/// Approval gate identifier bound to a card revision and artifact digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardApprovalGate {
    /// Spec approval gate.
    Spec,
    /// Plan approval gate.
    Plan,
    /// Final Ship delivery approval gate.
    Delivery,
}

/// Validity status of an approval receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardApprovalStatus {
    /// Currently valid and matches latest bound artifact digest.
    Valid,
    /// Invalidated by subsequent edits or revision return.
    Stale,
    /// Replaced by a newer approval receipt.
    Superseded,
    /// Imported historical receipt pending native re-validation.
    Historical,
}

/// Kind of deliverable stored under `.legion/workflows/<card-id>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardArtifactKind {
    /// Card metadata (`card.json`).
    Card,
    /// Specification markdown (`spec.md`).
    Spec,
    /// Plan markdown (`plan.md` / `plan-brief.md`).
    Plan,
    /// Design comparison markdown (`design.md`).
    Design,
    /// Optional HTML wireframe bundle (`wireframes/index.html`).
    Wireframes,
}

/// Execution status of a stage run linked to a board card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowBoardStageRunStatus {
    /// Created and awaiting launch.
    Pending,
    /// Thread launch in progress.
    Launching,
    /// Actively executing.
    Running,
    /// Paused mid-stage.
    Paused,
    /// Finished successfully.
    Completed,
    /// Failed with an error.
    Failed,
    /// Cancelled before completion.
    Cancelled,
}

/// Single acceptance criterion entry inside a card specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardAcceptanceCriterion {
    /// Criterion identifier (e.g. `AC-01`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub id: String,
    /// Acceptance condition description.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub text: String,
}

/// Stage run and thread launch link with distinct card vs run lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardStageRunLink {
    /// Stage run identifier.
    pub id: WorkflowBoardStageRunId,
    /// Workflow stage for this run (including `review`).
    pub stage: WorkflowBoardStageRunStage,
    /// Linked task identifier when launched for a specific Execute task.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub task_id: Option<String>,
    /// Stage attempt counter (`>= 1`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_positive_revision"
    )]
    pub attempt: Option<u64>,
    /// Idempotent launch key preventing duplicate threads on retry/restart.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub stable_launch_key: String,
    /// Current stage run status.
    pub status: WorkflowBoardStageRunStatus,
    /// Card revision at launch time (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub card_revision: u64,
    /// Optional `legion-workflows` run identifier.
    pub run_id: Option<RunId>,
    /// Linked thread identifier once launched.
    pub thread_id: Option<ThreadId>,
    /// Owning project identifier.
    pub project_id: ProjectId,
    /// Workspace launch strategy used for this stage run.
    pub workspace_strategy: WorkspaceStrategy,
    /// Resolved worktree path, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub worktree_path: Option<String>,
    /// Resolved branch name, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub branch: Option<String>,
    /// Explicit model selection used for the stage run.
    pub model_selection: ModelSelection,
    /// Explicit runtime mode used for the stage run.
    pub runtime_mode: RuntimeMode,
    /// Explicit provider interaction mode used for the stage run.
    pub interaction_mode: ProviderInteractionMode,
    /// Error message when failed.
    pub last_error: Option<String>,
    /// Creation ISO-8601 timestamp.
    pub created_at: String,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
    /// Completion/termination ISO-8601 timestamp, if ended.
    pub ended_at: Option<String>,
}

/// Entry in a card's immutable revision history ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardRevisionEntry {
    /// Monotonic card revision (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Stage at this revision.
    pub stage: WorkflowBoardStage,
    /// Revision event kind (e.g. `card_created`, `spec_updated`, `spec_approved`).
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub kind: String,
    /// Short human-readable summary.
    pub summary: String,
    /// Spec digest at this revision, if any.
    #[serde(deserialize_with = "deserialize_opt_trimmed_non_empty")]
    pub spec_digest: Option<String>,
    /// Creation ISO-8601 timestamp.
    pub created_at: String,
    /// Plan digest at this revision, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_trimmed_non_empty"
    )]
    pub plan_digest: Option<String>,
}

/// Deliverable file payload returned on demand by `workflowBoard.getArtifact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBoardArtifact {
    /// Card identifier.
    pub card_id: WorkflowCardId,
    /// Safe relative artifact key.
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub key: String,
    /// Artifact kind.
    pub kind: WorkflowBoardArtifactKind,
    /// Workspace-relative file path (e.g. `.legion/workflows/<card-id>/spec.md`).
    #[serde(deserialize_with = "deserialize_artifact_key")]
    pub relative_path: String,
    /// Content SHA-256 digest.
    #[serde(deserialize_with = "deserialize_trimmed_non_empty")]
    pub digest: String,
    /// File contents.
    pub content: String,
    /// Card revision associated with this artifact (`>= 1`).
    #[serde(deserialize_with = "deserialize_positive_revision")]
    pub revision: u64,
    /// Last update ISO-8601 timestamp.
    pub updated_at: String,
}
