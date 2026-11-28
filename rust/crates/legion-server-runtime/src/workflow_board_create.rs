#![forbid(unsafe_code)]
//! Card creation (`workflowBoard.createCard`) with intake scan, optional wireframes, and write journal.

use std::path::{Path, PathBuf};

use legion_contracts::orchestration::{PatchValue, WorkspaceStrategy};
use legion_contracts::workflow_board::{
    WorkflowBoardAssistanceMode, WorkflowBoardCreateCardInput, WorkflowBoardRequestError,
    WorkflowBoardRequestErrorCode, WorkflowBoardSpecSourceType, WorkflowCardDetail,
};
use legion_orchestrator::Orchestrator;
use legion_persistence::project_store::{ProjectRecord, find_project_by_id_or_workspace};
use legion_persistence::workflow_board_repo::types::CreateCardInput;
use legion_persistence::workflow_board_repo::{create_card, get_card_planning_detail};
use legion_workflows::sha256_prefixed;
use rusqlite::Connection;
use uuid::Uuid;

use crate::outbox_state::utc_now_iso;
use crate::thread_launcher_workspace::{validate_model_selection, validate_workspace_strategy};
use crate::workflow_board_fs::{
    ensure_confined_card_dir, enumerate_artifact_keys, validate_artifact_key_strict,
};
use crate::workflow_board_hydration_m2::to_card_detail_from_planning;
use crate::workflow_board_intake::prepare_intake;
use crate::workflow_board_journal::{
    JournaledWriteSpec, StagedEntry, finalize_staged_files, remove_confined_staging_file,
    stage_and_write_temp_files,
};
use crate::workflow_board_render::{StoredSpecFields, render_card_json, render_spec_markdown};
use crate::workflow_board_render_plan::render_wireframe_starter_html;
use crate::workflow_board_types::{enum_as_str, internal, map_launch_err, map_repo_err};

struct PreparedCardCreate {
    card_id: String,
    revision_id: String,
    project_id: String,
    brief: String,
    intake_goal: Option<String>,
    brief_digest: Option<String>,
    input_paths_json: String,
    source_type: WorkflowBoardSpecSourceType,
    assistance_mode: WorkflowBoardAssistanceMode,
    worktree_path: Option<String>,
    branch: Option<String>,
    deliverable_root: PathBuf,
    card_dir: PathBuf,
    ws_json: String,
    model_json: Option<String>,
    spec_fields: StoredSpecFields,
    spec_json: String,
    spec_md: String,
    spec_digest: String,
    wireframe_write: Option<(String, String, String)>,
    now_iso: String,
}

pub(crate) fn create_card_impl(
    orchestrator: &Orchestrator,
    input: &WorkflowBoardCreateCardInput,
    notify_project: impl Fn(&str),
) -> Result<WorkflowCardDetail, WorkflowBoardRequestError> {
    if let Some(ref model) = input.model_selection {
        validate_model_selection(model).map_err(map_launch_err)?;
    }
    let workspace_strategy = input
        .workspace_strategy
        .clone()
        .unwrap_or(WorkspaceStrategy::Root { branch: None });
    validate_workspace_strategy(&workspace_strategy).map_err(map_launch_err)?;

    let conn = orchestrator.sink().db().lock().map_err(|_| {
        WorkflowBoardRequestError::new(
            WorkflowBoardRequestErrorCode::InternalError,
            "Database lock poisoned",
        )
    })?;
    let project = find_project_by_id_or_workspace(&*conn, input.project_id.as_str())
        .map_err(|e| internal(e.to_string()))?
        .ok_or_else(|| {
            WorkflowBoardRequestError::new(
                WorkflowBoardRequestErrorCode::ProjectNotFound,
                format!("Project `{}` was not found", input.project_id.as_str()),
            )
        })?;
    let prep = prepare_card_create(&project, input, &workspace_strategy)?;
    let detail = commit_card_and_files(&conn, input, &prep)?;
    let planning = get_card_planning_detail(&conn, &prep.card_id).map_err(map_repo_err)?;
    notify_project(&detail.card.project_id);
    let keys = enumerate_artifact_keys(&prep.deliverable_root, &prep.card_id);
    to_card_detail_from_planning(&planning, keys)
}

fn prepare_card_create(
    project: &ProjectRecord,
    input: &WorkflowBoardCreateCardInput,
    workspace_strategy: &WorkspaceStrategy,
) -> Result<PreparedCardCreate, WorkflowBoardRequestError> {
    let now_iso = utc_now_iso();
    let card_id = format!("card-{}", Uuid::new_v4().simple());
    let revision_id = format!("rev-{}", Uuid::new_v4().simple());
    let source_type = input
        .source_type
        .unwrap_or(WorkflowBoardSpecSourceType::Idea);
    let assistance_mode = input
        .assistance_mode
        .unwrap_or(WorkflowBoardAssistanceMode::Balanced);
    let project_root = PathBuf::from(&project.workspace_root);

    let intake = prepare_intake(
        &project_root,
        &crate::workflow_board_intake::IntakeRequest {
            title: &input.title,
            explicit_brief: input.brief.as_deref(),
            brief_path: input.brief_path.as_deref(),
            input_paths: input.input_paths.as_deref(),
            source_type,
            intake_goal: input.intake_goal,
            assistance_mode,
        },
    )?;

    let wireframe_key = match &input.wireframes_index {
        PatchValue::Set(key) => Some(validate_artifact_key_strict(key)?.to_owned()),
        PatchValue::Absent if input.wireframes_enabled == Some(true) => {
            Some("wireframes/index.html".to_owned())
        }
        PatchValue::Null | PatchValue::Absent => None,
    };
    let wireframe_write = wireframe_key.map(|key| {
        let html = render_wireframe_starter_html(&card_id, &input.title, &intake.brief);
        let digest = sha256_prefixed(html.as_bytes());
        (key, html, digest)
    });

    let (worktree_path, branch) = match workspace_strategy {
        WorkspaceStrategy::ExistingWorktree {
            worktree_path,
            branch,
        } => (Some(worktree_path.trim().to_owned()), branch.clone()),
        WorkspaceStrategy::Root { branch } | WorkspaceStrategy::Worktree { branch, .. } => {
            (None, branch.clone())
        }
    };
    let deliverable_root = worktree_path.as_deref().map_or(project_root, PathBuf::from);
    let card_dir = ensure_confined_card_dir(&deliverable_root, &card_id)?;

    let spec_fields = StoredSpecFields {
        acceptance_criteria: Vec::new(),
        constraints: Vec::new(),
        source_refs: intake.source_refs,
        wireframes_index: wireframe_write.as_ref().map(|(k, _, _)| k.clone()),
        source_digest: intake.source_digest,
        source_input_digests: intake.source_input_digests,
        intake_analysis: intake.intake_analysis,
    };
    let spec_json = serde_json::to_string(&spec_fields).map_err(|e| internal(e.to_string()))?;
    let spec_md = render_spec_markdown(
        &card_id,
        &input.title,
        &intake.brief,
        source_type,
        assistance_mode,
        &spec_fields,
        &now_iso,
    );
    let spec_digest = sha256_prefixed(spec_md.as_bytes());
    let ws_json = serde_json::to_string(workspace_strategy).map_err(|e| internal(e.to_string()))?;
    let model_json = input
        .model_selection
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| internal(e.to_string()))?;
    let input_paths_json = serde_json::to_string(input.input_paths.as_deref().unwrap_or(&[]))
        .map_err(|e| internal(e.to_string()))?;
    let intake_goal = input.intake_goal.and_then(|g| enum_as_str(&g));

    Ok(PreparedCardCreate {
        card_id,
        revision_id,
        project_id: project.id.clone(),
        brief: intake.brief,
        intake_goal,
        brief_digest: intake.brief_digest,
        input_paths_json,
        source_type,
        assistance_mode,
        worktree_path,
        branch,
        deliverable_root,
        card_dir,
        ws_json,
        model_json,
        spec_fields,
        spec_json,
        spec_md,
        spec_digest,
        wireframe_write,
        now_iso,
    })
}

pub(crate) struct CreateTxGuard<'a> {
    conn: &'a Connection,
    savepoint: bool,
    committed: bool,
}

impl<'a> CreateTxGuard<'a> {
    pub(crate) fn begin(conn: &'a Connection) -> Result<Self, WorkflowBoardRequestError> {
        let savepoint = !conn.is_autocommit();
        let sql = if savepoint {
            "SAVEPOINT legion_workflow_board_create;"
        } else {
            "BEGIN IMMEDIATE;"
        };
        conn.execute_batch(sql)
            .map_err(|e| internal(format!("Failed to begin createCard transaction: {e}")))?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    pub(crate) fn commit(mut self) -> Result<(), WorkflowBoardRequestError> {
        let sql = if self.savepoint {
            "RELEASE SAVEPOINT legion_workflow_board_create;"
        } else {
            "COMMIT;"
        };
        self.conn
            .execute_batch(sql)
            .map_err(|e| internal(format!("Failed to commit createCard transaction: {e}")))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for CreateTxGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if self.savepoint {
                let _ = self.conn.execute_batch(
                    "ROLLBACK TO SAVEPOINT legion_workflow_board_create; RELEASE SAVEPOINT legion_workflow_board_create;",
                );
            } else if !self.conn.is_autocommit() {
                let _ = self.conn.execute_batch("ROLLBACK;");
            }
        }
    }
}

fn commit_card_and_files(
    conn: &Connection,
    input: &WorkflowBoardCreateCardInput,
    prep: &PreparedCardCreate,
) -> Result<legion_persistence::workflow_board_repo::types::CardDetail, WorkflowBoardRequestError> {
    let src_str = enum_as_str(&prep.source_type).unwrap_or_else(|| "idea".to_owned());
    let assist_str = enum_as_str(&prep.assistance_mode).unwrap_or_else(|| "balanced".to_owned());
    let rt_str = input.runtime_mode.and_then(|r| enum_as_str(&r));
    let im_str = input.interaction_mode.and_then(|m| enum_as_str(&m));
    let wf_digest = prep.wireframe_write.as_ref().map(|(_, _, d)| d.as_str());

    let (detail, staged) = {
        let tx = CreateTxGuard::begin(conn)?;
        let detail = create_card(
            conn,
            &CreateCardInput {
                card_id: &prep.card_id,
                revision_id: &prep.revision_id,
                project_id: &prep.project_id,
                title: &input.title,
                brief: &prep.brief,
                intake_goal: prep.intake_goal.as_deref(),
                brief_path: input.brief_path.as_deref(),
                brief_digest: prep.brief_digest.as_deref(),
                input_paths_json: Some(&prep.input_paths_json),
                source_type: &src_str,
                assistance_mode: &assist_str,
                workspace_strategy_json: &prep.ws_json,
                worktree_path: prep.worktree_path.as_deref(),
                branch: prep.branch.as_deref(),
                model_selection_json: prep.model_json.as_deref(),
                runtime_mode: rt_str.as_deref(),
                interaction_mode: im_str.as_deref(),
                spec_json: &prep.spec_json,
                spec_digest: Some(&prep.spec_digest),
                wireframes_digest: wf_digest,
            },
            &prep.now_iso,
        )
        .map_err(map_repo_err)?;
        let staged = stage_create_deliverables(conn, &detail.card, &prep.card_dir, prep)?;
        if let Err(err) = tx.commit() {
            for entry in &staged {
                remove_confined_staging_file(&entry.card_dir, &entry.staging_path);
            }
            return Err(err);
        }
        (detail, staged)
    };
    finalize_staged_files(conn, &staged, &prep.now_iso)?;
    Ok(detail)
}

fn stage_create_deliverables(
    conn: &Connection,
    card: &legion_persistence::workflow_board_repo::types::CardRow,
    card_dir: &Path,
    prep: &PreparedCardCreate,
) -> Result<Vec<StagedEntry>, WorkflowBoardRequestError> {
    let card_json = render_card_json(card, &prep.spec_fields)?;
    let mut writes = vec![
        JournaledWriteSpec {
            key: "spec.md".to_owned(),
            bytes: prep.spec_md.as_bytes().to_vec(),
            expected_digest: prep.spec_digest.clone(),
            revision: Some(card.revision),
        },
        JournaledWriteSpec {
            key: "card.json".to_owned(),
            bytes: card_json.as_bytes().to_vec(),
            expected_digest: sha256_prefixed(card_json.as_bytes()),
            revision: Some(card.revision),
        },
    ];
    if let Some((ref key, ref html, ref digest)) = prep.wireframe_write {
        writes.push(JournaledWriteSpec {
            key: key.clone(),
            bytes: html.as_bytes().to_vec(),
            expected_digest: digest.clone(),
            revision: Some(card.revision),
        });
    }
    stage_and_write_temp_files(conn, &prep.card_id, card_dir, &writes, &prep.now_iso)
}
