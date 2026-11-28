#![forbid(unsafe_code)]
//! Project Workflow Board service (`createCard`, `updateSpec`, `approveSpec`, `exploreStep`,
//! `challengeStep`, `updatePlan`, `approvePlan`, `returnForRevision`, `startStage`, `getCard`,
//! `getArtifact`, `getBoardSnapshot`, `subscribe_changes`, and startup crash recovery).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use legion_contracts::ids::{EnvironmentId, ProjectId, WorkflowCardId};
use legion_contracts::workflow_board::{
    ProjectWorkflowBoardSnapshot, WorkflowBoardApprovePlanInput, WorkflowBoardApprovePlanResult,
    WorkflowBoardApproveSpecInput, WorkflowBoardApproveSpecResult, WorkflowBoardArtifact,
    WorkflowBoardArtifactKind, WorkflowBoardChallengeStepInput, WorkflowBoardChallengeStepResult,
    WorkflowBoardCreateCardInput, WorkflowBoardExploreStepInput, WorkflowBoardExploreStepResult,
    WorkflowBoardGetArtifactInput, WorkflowBoardGetCardInput, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardReturnForRevisionInput,
    WorkflowBoardReturnForRevisionResult, WorkflowBoardStartStageInput,
    WorkflowBoardStartStageResult, WorkflowBoardUpdatePlanInput, WorkflowBoardUpdatePlanResult,
    WorkflowBoardUpdateSpecInput, WorkflowBoardUpdateSpecResult, WorkflowCardDetail,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::workflow_board_repo::{get_board_snapshot, get_card_detail};
use legion_workflows::definitions::WorkflowDefinitionsManager;
use legion_workflows::sha256_prefixed;
use tokio::sync::broadcast;

use crate::outbox_state::utc_now_iso;
use crate::workflow_board_challenge::challenge_step_impl;
use crate::workflow_board_create::create_card_impl;
use crate::workflow_board_explore_challenge::explore_step_impl;
use crate::workflow_board_fs::{
    resolve_card_workspace_root, resolve_confined_artifact_path, validate_artifact_key_strict,
    validate_safe_card_id,
};
use crate::workflow_board_hydration::to_board_snapshot;
use crate::workflow_board_hydration_m3::get_card_detail_impl;
use crate::workflow_board_mutations::update_spec_impl;
use crate::workflow_board_plan::{approve_plan_impl, update_plan_impl};
use crate::workflow_board_recovery::{
    WriteJournalRecoveryReport, reconcile_incomplete_write_journal,
};
use crate::workflow_board_render::parse_stored_spec_fields;
use crate::workflow_board_spec_approve::{approve_spec_impl, return_for_revision_impl};
use crate::workflow_board_stage::start_stage_impl;
use crate::workflow_board_stage_prep::StageLaunchEnv;
use crate::workflow_board_types::{infer_artifact_kind, map_repo_err};

/// Durable Project Workflow Board service backed by SQLite and `.legion/workflows/<card-id>/`.
#[derive(Clone)]
pub struct ProjectWorkflowBoardService {
    orchestrator: Arc<Orchestrator>,
    worktrees_dir: PathBuf,
    settings_path: Option<PathBuf>,
    environment_id: EnvironmentId,
    workflow_manager: Option<Arc<WorkflowDefinitionsManager>>,
    changes: broadcast::Sender<String>,
}

impl ProjectWorkflowBoardService {
    /// Constructs a new [`ProjectWorkflowBoardService`].
    #[must_use]
    pub fn new(
        orchestrator: Arc<Orchestrator>,
        worktrees_dir: PathBuf,
        settings_path: Option<PathBuf>,
        environment_id: EnvironmentId,
        workflow_manager: Option<Arc<WorkflowDefinitionsManager>>,
    ) -> Self {
        let (changes, _) = broadcast::channel(256);
        Self {
            orchestrator,
            worktrees_dir,
            settings_path,
            environment_id,
            workflow_manager,
            changes,
        }
    }

    /// Subscribes to project IDs whenever board cards or stage runs change.
    #[must_use]
    pub fn subscribe_changes(&self) -> broadcast::Receiver<String> {
        self.changes.subscribe()
    }

    fn notify_project(&self, project_id: &str) {
        let _ = self.changes.send(project_id.to_owned());
    }

    /// Reconciles incomplete write-journal entries and orphaned `.tmp` files on startup.
    pub fn reconcile_startup(
        &self,
    ) -> Result<WriteJournalRecoveryReport, WorkflowBoardRequestError> {
        let conn = self.orchestrator.sink().db().lock().map_err(|_| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                "Database lock poisoned",
            )
        })?;
        reconcile_incomplete_write_journal(&conn, &utc_now_iso())
    }

    /// Returns a compact [`ProjectWorkflowBoardSnapshot`] for `project_id`.
    pub fn get_board_snapshot(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectWorkflowBoardSnapshot, WorkflowBoardRequestError> {
        let conn = self.orchestrator.sink().db().lock().map_err(|_| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                "Database lock poisoned",
            )
        })?;
        let project_exists: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM projection_projects WHERE project_id = ?1 AND deleted_at IS NULL)",
                [project_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|err| crate::workflow_board_types::internal(err.to_string()))?;
        if !project_exists {
            return Err(WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ProjectNotFound,
                format!("Project `{}` was not found", project_id.as_str()),
            ));
        }
        let snap =
            get_board_snapshot(&conn, project_id.as_str(), &utc_now_iso()).map_err(map_repo_err)?;
        to_board_snapshot(&snap)
    }

    /// Loads full card detail, reconciling external edits on disk first.
    pub fn get_card_detail(
        &self,
        input: &WorkflowBoardGetCardInput,
    ) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
        get_card_detail_impl(&self.orchestrator, input, |pid| self.notify_project(pid))
    }

    /// Reads an on-demand deliverable artifact from `<card-workspace>/.legion/workflows/<card-id>/`.
    pub fn get_artifact(
        &self,
        input: &WorkflowBoardGetArtifactInput,
    ) -> Result<WorkflowBoardArtifact, WorkflowBoardRequestError> {
        let card_id = validate_safe_card_id(input.card_id.as_str())?;
        let conn = self.orchestrator.sink().db().lock().map_err(|_| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                "Database lock poisoned",
            )
        })?;
        let detail = get_card_detail(&conn, card_id).map_err(map_repo_err)?;
        let ws_root = resolve_card_workspace_root(&conn, &detail.card)?;
        drop(conn);

        let fields = parse_stored_spec_fields(&detail.card.spec_json);
        let key = match (&input.key, input.kind) {
            (Some(k), _) => validate_artifact_key_strict(k)?.to_owned(),
            (None, Some(WorkflowBoardArtifactKind::Card)) => "card.json".to_owned(),
            (None, Some(WorkflowBoardArtifactKind::Spec)) => "spec.md".to_owned(),
            (None, Some(WorkflowBoardArtifactKind::Plan)) => {
                if resolve_confined_artifact_path(&ws_root, card_id, "plan.md").is_ok() {
                    "plan.md".to_owned()
                } else {
                    "plan-brief.md".to_owned()
                }
            }
            (None, Some(WorkflowBoardArtifactKind::Design)) => "design.md".to_owned(),
            (None, Some(WorkflowBoardArtifactKind::Wireframes)) => fields
                .wireframes_index
                .unwrap_or_else(|| "wireframes/index.html".to_owned()),
            (None, None) => {
                return Err(WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InvalidInput,
                    "Either `key` or `kind` must be provided",
                ));
            }
        };

        let path = resolve_confined_artifact_path(&ws_root, card_id, &key)?;
        let bytes = fs::read(&path).map_err(|e| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InternalError,
                format!("Failed to read artifact `{key}`: {e}"),
            )
        })?;
        let digest = sha256_prefixed(&bytes);
        let content = String::from_utf8(bytes).map_err(|_| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::InvalidInput,
                format!("Artifact `{key}` is not valid UTF-8"),
            )
        })?;
        let kind = input.kind.unwrap_or_else(|| infer_artifact_kind(&key));

        Ok(WorkflowBoardArtifact {
            card_id: WorkflowCardId::new(card_id).map_err(|e| {
                WorkflowBoardRequestError::new(
                    WorkflowBoardRequestErrorCode::InternalError,
                    e.to_string(),
                )
            })?,
            relative_path: format!(".legion/workflows/{card_id}/{key}"),
            key,
            kind,
            digest,
            content,
            revision: u64::try_from(detail.card.revision.max(1)).unwrap_or(1),
            updated_at: detail.card.updated_at,
        })
    }

    /// Creates a new workflow card and writes initial `card.json` + `spec.md` without auto-committing.
    pub fn create_card(
        &self,
        input: &WorkflowBoardCreateCardInput,
    ) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
        create_card_impl(&self.orchestrator, input, |pid| self.notify_project(pid))
    }

    /// Updates card spec fields with strict `expectedRevision` CAS and write-journaled deliverables.
    pub async fn update_spec(
        &self,
        input: &WorkflowBoardUpdateSpecInput,
    ) -> Result<WorkflowBoardUpdateSpecResult, WorkflowBoardRequestError> {
        update_spec_impl(&self.orchestrator, input, |pid| self.notify_project(pid)).await
    }

    /// Records a client-authenticated Spec approval bound to `(revision, spec_digest)`.
    pub async fn approve_spec(
        &self,
        input: &WorkflowBoardApproveSpecInput,
        approved_by: &str,
    ) -> Result<WorkflowBoardApproveSpecResult, WorkflowBoardRequestError> {
        approve_spec_impl(&self.orchestrator, input, approved_by, |pid| {
            self.notify_project(pid);
        })
        .await
    }

    /// Runs a resumable guided exploration step (`<= 3` decisions/round).
    pub async fn explore_step(
        &self,
        input: &WorkflowBoardExploreStepInput,
    ) -> Result<WorkflowBoardExploreStepResult, WorkflowBoardRequestError> {
        explore_step_impl(&self.orchestrator, input, |pid| self.notify_project(pid)).await
    }

    /// Runs a bounded challenge step (`0..=3` concerns, max 1 synthesis pass, or manual fallback).
    pub async fn challenge_step(
        &self,
        input: &WorkflowBoardChallengeStepInput,
    ) -> Result<WorkflowBoardChallengeStepResult, WorkflowBoardRequestError> {
        challenge_step_impl(&self.orchestrator, input, |pid| self.notify_project(pid)).await
    }

    /// Updates M2 Plan lenses, alternatives, strategy, granularity, task DAG, checks, and AC mappings.
    pub async fn update_plan(
        &self,
        input: &WorkflowBoardUpdatePlanInput,
    ) -> Result<WorkflowBoardUpdatePlanResult, WorkflowBoardRequestError> {
        update_plan_impl(&self.orchestrator, input, |pid| self.notify_project(pid)).await
    }

    /// Records a client-authenticated Plan approval bound to `(spec_digest, plan_digest, tasks_digest)`.
    pub async fn approve_plan(
        &self,
        input: &WorkflowBoardApprovePlanInput,
        approved_by: &str,
    ) -> Result<WorkflowBoardApprovePlanResult, WorkflowBoardRequestError> {
        approve_plan_impl(&self.orchestrator, input, approved_by, |pid| {
            self.notify_project(pid);
        })
        .await
    }

    /// Returns a card to an earlier stage (`spec` | `plan` | `execute`) with targeted invalidation.
    pub async fn return_for_revision(
        &self,
        input: &WorkflowBoardReturnForRevisionInput,
    ) -> Result<WorkflowBoardReturnForRevisionResult, WorkflowBoardRequestError> {
        return_for_revision_impl(&self.orchestrator, input, |pid| self.notify_project(pid)).await
    }

    /// Launches or idempotently resumes a built-in `Spec`, `Plan`, `Execute`, or `Review` stage job.
    pub async fn start_stage(
        &self,
        input: &WorkflowBoardStartStageInput,
    ) -> Result<WorkflowBoardStartStageResult, WorkflowBoardRequestError> {
        let env = StageLaunchEnv {
            orchestrator: &self.orchestrator,
            worktrees_dir: &self.worktrees_dir,
            settings_path: self.settings_path.as_ref(),
            environment_id: self.environment_id.as_str(),
            workflow_manager: self.workflow_manager.as_ref(),
        };
        start_stage_impl(&env, input, |pid| self.notify_project(pid)).await
    }

    pub(crate) fn orchestrator_ref(&self) -> &Arc<Orchestrator> {
        &self.orchestrator
    }

    pub(crate) fn workflow_manager_ref(&self) -> Option<&Arc<WorkflowDefinitionsManager>> {
        self.workflow_manager.as_ref()
    }

    pub(crate) fn notify_project_change(&self, project_id: &str) {
        self.notify_project(project_id);
    }
}
